#!/usr/bin/env python3
"""Install the fixed on-demand Reap roles without control of foreign processes."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import pwd
import re
import stat
import subprocess
import sys
import tempfile
import time
import uuid

MAX_BYTES = 65536
ROLES = ("control", "context-resolver", "state-writer")
TOKEN = "REAP_SERVICE_INSTALLATION_ID"


class Conflict(RuntimeError):
    """Ownership or native absence could not be proved."""


def command(arguments, home):
    """Bound each native command and keep the broker namespace independent of the caller."""
    environment = dict(os.environ, HOME=str(home), XDG_STATE_HOME=str(home / ".local/state"))
    result = subprocess.run(arguments, env=environment, capture_output=True, text=True, timeout=5)
    if len(result.stdout.encode()) + len(result.stderr.encode()) > MAX_BYTES:
        raise Conflict("native response exceeds its bound")
    return result


def read_file(path):
    """Read one bounded, owner-controlled regular file without a final symlink."""
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid() or before.st_nlink != 1:
            raise Conflict("installation evidence is not an owned regular file")
        if before.st_mode & 0o022:
            raise Conflict("installation evidence is writable by another user")
        data = source.read(MAX_BYTES + 1)
        after = os.fstat(source.fileno())
        if len(data) > MAX_BYTES or (before.st_ino, before.st_size, before.st_mtime_ns) != (
                after.st_ino, after.st_size, after.st_mtime_ns):
            raise Conflict("installation evidence changed or exceeds its bound")
        return data


def atomic_file(path, data):
    """Publish durable evidence in the same directory before a native lifecycle action."""
    descriptor, temporary = tempfile.mkstemp(prefix=".reap-service-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            value.update(chunk)
    return value.hexdigest()


def scalar(text, name, optional=False):
    values = re.findall(r"^\s*" + re.escape(name) + r" = (.*)$", text, re.M)
    if not values and optional:
        return None
    if len(values) != 1:
        raise Conflict("native service status lacks an unambiguous " + name)
    value = values[0].strip()
    return json.loads(value) if value.startswith('"') else value


def block(text, name):
    matches = list(re.finditer(r"^\s*" + re.escape(name) + r" = \{\n", text, re.M))
    if len(matches) != 1:
        raise Conflict("native service status lacks an unambiguous " + name)
    start = matches[0].end()
    depth = 1
    consumed = 0
    for line in text[start:].splitlines(keepends=True):
        depth += line.count("{") - line.count("}")
        if depth == 0:
            return text[start:start + consumed].strip()
        consumed += len(line)
    raise Conflict("native service status has an incomplete " + name)


class Installer:
    """Own only exact registrations whose durable creation records match native evidence."""

    def __init__(self, home, uid, binary, candidate):
        self.home = Path(home)
        self.uid = uid
        self.binary = Path(binary).absolute()
        self.candidate = Path(candidate).absolute()
        self.domain = f"user/{uid}"
        self.root = self.home / ".local/state/reap/service-installation"
        self.state_path = self.root / "ownership.json"
        self.agents = self.home / "Library/LaunchAgents"
        self.profile = self.json_command(self.candidate, "__service-profile")
        self.validate_profile(self.profile)
        self.sha256 = digest(self.candidate)

    def run(self, *arguments):
        return command(list(map(str, arguments)), self.home)

    def json_command(self, executable, *arguments):
        result = self.run(executable, *arguments)
        if result.returncode:
            raise Conflict("native " + arguments[0] + " did not provide complete evidence")
        value = json.loads(result.stdout)
        if not isinstance(value, dict):
            raise Conflict("native response is not a complete JSON object")
        return value

    def validate_profile(self, value):
        if not isinstance(value, dict) or value.get("schema") != 1 or value.get("protocol") != 2 \
                or value.get("uid") != self.uid or value.get("role_profile") != "fixed-agentless-v1" \
                or value.get("agent_hosts") is not False \
                or value.get("abandon_process_group") is not True:
            raise Conflict("binary lacks the fixed agentless, preserved-group profile")
        roles = value.get("roles")
        if not isinstance(roles, list) or len(roles) != 3:
            raise Conflict("binary does not advertise the three fixed roles")
        endpoints = set()
        for role in roles:
            if set(role) != {"argument", "label", "endpoint"} or role["argument"] not in ROLES \
                    or role["label"] != "dev.micro.reap." + role["argument"] \
                    or not re.fullmatch(r"[a-zA-Z0-9_.-]{1,160}", role["endpoint"]):
                raise Conflict("binary has an invalid fixed-role descriptor")
            endpoints.add(role["endpoint"])
        if {role["argument"] for role in roles} != set(ROLES) or len(endpoints) != 3:
            raise Conflict("binary has duplicate fixed roles or endpoints")

    def role(self, argument, profile=None):
        return next(role for role in (profile or self.profile)["roles"] if role["argument"] == argument)

    def plist_path(self, argument):
        return self.agents / (self.role(argument)["label"] + ".plist")

    def plist(self, argument, token, profile=None):
        role = self.role(argument, profile)
        return {"Label": role["label"], "ProgramArguments": [str(self.binary), "__service", argument],
                "MachServices": {role["endpoint"]: True}, "AbandonProcessGroup": True,
                "LimitLoadToSessionType": "Background", "WorkingDirectory": str(self.home),
                "EnvironmentVariables": {"HOME": str(self.home),
                                         "XDG_STATE_HOME": str(self.home / ".local/state"), TOKEN: token}}

    def creation(self, argument):
        token = uuid.uuid4().hex
        return {"intent": token, "binary_sha256": self.sha256, "profile": self.profile,
                "plist": self.plist(argument, token)}

    def load(self):
        for directory in (self.root, self.agents):
            self.directory_chain(directory)
        try:
            state = json.loads(read_file(self.state_path))
        except FileNotFoundError:
            if self.root.exists():
                raise Conflict("installation directory has no ownership record")
            return {"schema": 1, "uid": self.uid, "home": str(self.home),
                    "binary": str(self.binary), "roles": {}}
        if not isinstance(state, dict) \
                or set(state) != {"schema", "uid", "home", "binary", "roles"} or state["schema"] != 1 \
                or state["uid"] != self.uid or state["home"] != str(self.home) \
                or state["binary"] != str(self.binary) or not isinstance(state["roles"], dict) \
                or not set(state["roles"]).issubset(ROLES):
            raise Conflict("installation ownership record does not match this host user and binary")
        for argument, entry in state["roles"].items():
            if not isinstance(entry, dict) or entry.get("phase") not in ("creating", "installed", "retiring"):
                raise Conflict("installation ownership phase is unknown")
            for creation in [entry] + ([entry["replacement"]] if "replacement" in entry else []):
                self.validate_profile(creation["profile"])
                if not re.fullmatch(r"[0-9a-f]{32}", creation["intent"]) \
                        or not re.fullmatch(r"[0-9a-f]{64}", creation["binary_sha256"]) \
                        or creation["plist"] != self.plist(argument, creation["intent"], creation["profile"]):
                    raise Conflict("creation record lacks exact agentless registration ownership")
            for name in ("old_process", "last_process"):
                if entry.get(name) is not None:
                    saved = entry[name]
                    self.validate_identity(saved, saved.get("pid") if isinstance(saved, dict) else None)
        return state

    def directory_chain(self, directory, create=False):
        """Refuse unsafe ancestry; make new owned directories durable before native registration."""
        for parent in reversed([directory, *directory.parents]):
            if parent != self.home and self.home not in parent.parents:
                continue
            try:
                info = parent.lstat()
            except FileNotFoundError:
                if not create:
                    continue
                parent.mkdir(mode=0o700)
                sync_directory(parent.parent)
                info = parent.lstat()
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != self.uid or info.st_mode & 0o022:
                raise Conflict("installation directory is not exclusively controlled by the host user")

    def save(self, state):
        data = (json.dumps(state, sort_keys=True, indent=2) + "\n").encode()
        if len(data) > MAX_BYTES:
            raise Conflict("installation ownership record exceeds its bound")
        atomic_file(self.state_path, data)

    def status(self, argument, domain=None):
        target = (domain or self.domain) + "/" + self.role(argument)["label"]
        result = self.run("/bin/launchctl", "print", target)
        if result.returncode == 113:
            return None
        if result.returncode:
            raise Conflict("native registration state is unknown: " + target)
        return result.stdout

    def validate_registration(self, argument, text, creation):
        expected = creation["plist"]
        actual = [line.strip() for line in block(text, "arguments").splitlines()]
        actual = [json.loads(line) if line.startswith('"') else line for line in actual]
        environment = {}
        for line in block(text, "environment").splitlines():
            key, separator, value = line.strip().partition(" => ")
            if not separator or key in environment:
                raise Conflict("registered service environment is ambiguous")
            environment[key] = value
        if environment.get("XPC_SERVICE_NAME") == expected["Label"]:
            del environment["XPC_SERVICE_NAME"]
        if environment.get("OSLogRateLimit") == "64":
            del environment["OSLogRateLimit"]
        endpoints = re.findall(r'^\s*"?([a-zA-Z0-9_.-]+)"? = \{', block(text, "endpoints"), re.M)
        if actual != expected["ProgramArguments"] or scalar(text, "program") != str(self.binary) \
                or scalar(text, "path") != str(self.plist_path(argument)) \
                or scalar(text, "working directory") != str(self.home) \
                or environment != expected["EnvironmentVariables"] \
                or endpoints != list(expected["MachServices"]) \
                or not re.fullmatch(re.escape(self.domain) + r"(?: \[[0-9]+\])?", scalar(text, "domain")) \
                or "abandon process group" not in scalar(text, "properties").split(" | "):
            raise Conflict("registered role does not match its exact creation intent: " + argument)
        pid = scalar(text, "pid", optional=True)
        if pid is not None and (not pid.isdecimal() or not 0 < int(pid) <= 2147483647):
            raise Conflict("registered role has an invalid process identifier")
        return int(pid) if pid is not None else None

    def identity(self, pid):
        value = self.json_command(self.candidate, "__process-identity", str(pid))
        if value.get("state") == "absent":
            return None
        identity = value.get("identity")
        if value.get("state") != "present":
            raise Conflict("native process identity is unknown")
        self.validate_identity(identity, pid)
        return identity

    def validate_identity(self, identity, pid):
        if not isinstance(identity, dict) \
                or set(identity) != {"pid", "uid", "boot", "start_seconds", "start_fraction"} \
                or type(pid) is not int or not 0 < pid <= 2147483647 \
                or identity["pid"] != pid or identity["uid"] != self.uid \
                or not isinstance(identity["boot"], str) \
                or not re.fullmatch(r"[a-fA-F0-9-]{1,64}", identity["boot"]) \
                or type(identity["start_seconds"]) is not int or identity["start_seconds"] <= 0 \
                or type(identity["start_fraction"]) is not int or identity["start_fraction"] < 0:
            raise Conflict("native process identity is unknown")

    def old_absent(self, identity):
        return identity is None or self.identity(identity["pid"]) != identity

    def owned_plist(self, argument, creations):
        try:
            contents = plistlib.loads(read_file(self.plist_path(argument)))
        except FileNotFoundError:
            return None
        if not any(contents == creation["plist"] for creation in creations):
            raise Conflict("existing plist has no matching creation ownership: " + argument)
        return contents

    def inspect(self, state):
        """Validate the entire cohort before the first registration or plist mutation."""
        observed = {}
        for argument in ROLES:
            if self.status(argument, f"gui/{self.uid}") is not None:
                raise Conflict("fixed role exists outside the single user cohort: " + argument)
            text = self.status(argument)
            entry = state["roles"].get(argument)
            creations = [] if entry is None else [entry] + ([entry["replacement"]] if "replacement" in entry else [])
            contents = self.owned_plist(argument, creations)
            if text is None:
                observed[argument] = None
                continue
            if contents is None or not creations:
                raise Conflict("existing registration has no exact creation ownership: " + argument)
            matched = None
            for creation in creations:
                try:
                    pid = self.validate_registration(argument, text, creation)
                except Conflict:
                    continue
                matched = {"intent": creation["intent"], "identity": self.identity(pid) if pid else None}
                if pid and matched["identity"] is None:
                    raise Conflict("registered process changed during ownership observation")
                break
            if matched is None:
                raise Conflict("existing registration differs from every owned intent: " + argument)
            observed[argument] = matched
        return observed

    def preflight(self):
        state = self.load()
        observed = self.inspect(state)
        for argument, entry in state["roles"].items():
            if "replacement" in entry and entry["replacement"]["binary_sha256"] != self.sha256:
                raise Conflict("an incomplete refresh requires its recorded binary before another upgrade")
            if entry.get("old_process") is not None and not self.old_absent(entry["old_process"]):
                current = observed[argument]
                if current is None or current["identity"] != entry["old_process"]:
                    raise Conflict("an old role process still has unresolved ownership")
            previous = entry.get("last_process")
            current = observed[argument]
            if previous is not None and (current is None or current["identity"] != previous) \
                    and not self.old_absent(previous):
                raise Conflict("a previously observed role process remains present outside its registration")
        return {"schema": 1, "uid": self.uid, "home": str(self.home), "binary": str(self.binary),
                "candidate_sha256": self.sha256, "profile": self.profile, "state": state}

    def probe(self, argument, require_ready=True):
        deadline = time.monotonic() + 3
        while True:
            value = self.json_command(self.binary, "__service-probe", argument)
            if value.get("protocol") != 2 or value.get("via") != argument.replace("-", "_") \
                    or value.get("outcome") != {"result": "pong"}:
                raise Conflict("native role probe did not confirm protocol and role: " + argument)
            snapshot = value.get("snapshot")
            phase = snapshot.get("phase") if isinstance(snapshot, dict) else None
            # control Ping returns cached writer state; its initial snapshot does not prove forwarding health.
            if not require_ready or argument == "control" or phase in ("ready", "busy"):
                return
            if phase != "starting" or time.monotonic() >= deadline:
                raise Conflict("native role snapshot is not available: " + argument)
            time.sleep(0.05)

    def retire(self, argument, entry, state):
        text = self.status(argument)
        if text is not None:
            self.validate_registration(argument, text, entry)
            self.probe(argument, require_ready=False)
            text = self.status(argument)
            if text is None:
                raise Conflict("role disappeared before its native identity could be retained")
            pid = self.validate_registration(argument, text, entry)
            if pid is None:
                raise Conflict("owned role did not expose its native process identity")
            identity = self.identity(pid)
            if identity is None:
                raise Conflict("owned process disappeared before refresh")
            entry["old_process"] = identity
            self.save(state)
            current = self.status(argument)
            if current is None or self.validate_registration(argument, current, entry) != pid \
                    or self.identity(pid) != identity:
                raise Conflict("owned role changed before scoped bootout")
            # only this recorded agentless registration can receive bootout; its process group is preserved.
            try:
                self.run("/bin/launchctl", "bootout", self.domain + "/" + self.role(argument)["label"])
            except subprocess.TimeoutExpired:
                pass
        deadline = time.monotonic() + 3
        while True:
            present = self.status(argument)
            absent = self.old_absent(entry.get("old_process"))
            if present is None and absent:
                return
            if present is not None:
                self.validate_registration(argument, present, entry)
            if time.monotonic() >= deadline:
                raise Conflict("refresh incomplete: exact old registration/process absence is not proved")
            time.sleep(0.05)

    def apply(self, plan):
        if plan != self.preflight():
            raise Conflict("preflight evidence changed before apply")
        if not self.root.exists():
            self.directory_chain(self.root.parent, create=True)
            self.root.mkdir(mode=0o700)
            sync_directory(self.root.parent)
            self.save(plan["state"])
        descriptor = os.open(self.root / "install.lock",
                             os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
        with os.fdopen(descriptor, "r+") as lock:
            info = os.fstat(lock.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != self.uid \
                    or info.st_nlink != 1 or info.st_mode & 0o022:
                raise Conflict("installation lock is not an owned regular file")
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as reason:
                raise Conflict("another service installation owns the lifecycle lock") from reason
            if plan != self.preflight():
                raise Conflict("preflight evidence changed before the lifecycle lock")
            state = self.load()
            self.directory_chain(self.agents, create=True)
            for argument in ROLES:
                entry = state["roles"].get(argument)
                if entry is None:
                    entry = dict(self.creation(argument), phase="creating")
                    state["roles"][argument] = entry
                    self.save(state)
                elif entry["binary_sha256"] != self.sha256 or entry["profile"] != self.profile:
                    if "replacement" not in entry:
                        entry["replacement"] = self.creation(argument)
                        entry["phase"] = "retiring"
                        self.save(state)
                    text = self.status(argument)
                    replacement = entry["replacement"]
                    already_created = False
                    if text is not None:
                        try:
                            self.validate_registration(argument, text, replacement)
                            already_created = True
                        except Conflict:
                            pass
                    if not already_created:
                        self.retire(argument, entry, state)
                    elif not self.old_absent(entry.get("old_process")):
                        raise Conflict("old role absence remains unresolved after replacement bootstrap")
                    # the old record retains both exact plists until the new plist has durable publication.
                    self.owned_plist(argument, [entry, replacement])
                    atomic_file(self.plist_path(argument), plistlib.dumps(replacement["plist"], sort_keys=True))
                    entry = dict(replacement, phase="creating")
                    state["roles"][argument] = entry
                    self.save(state)
                text = self.status(argument)
                if text is None:
                    if not self.old_absent(entry.get("last_process")):
                        raise Conflict("unregistered old process remains present")
                    entry["phase"] = "creating"
                    self.save(state)
                    self.owned_plist(argument, [entry])
                    atomic_file(self.plist_path(argument), plistlib.dumps(entry["plist"], sort_keys=True))
                    # the durable intent survives timeout, partial bootstrap, and failed confirmation.
                    try:
                        self.run("/bin/launchctl", "bootstrap", self.domain, self.plist_path(argument))
                    except subprocess.TimeoutExpired:
                        pass
                    text = self.status(argument)
                    if text is None:
                        raise Conflict("bootstrap incomplete; exact creation intent is retained: " + argument)
                self.validate_registration(argument, text, entry)
                self.probe(argument)
                text = self.status(argument)
                if text is None:
                    raise Conflict("role is absent after its native probe")
                pid = self.validate_registration(argument, text, entry)
                if pid is None:
                    raise Conflict("role has no process after its native probe")
                entry["last_process"] = self.identity(pid)
                if entry["last_process"] is None:
                    raise Conflict("role process disappeared after its native probe")
                entry["phase"] = "installed"
                self.save(state)
            self.inspect(state)
        return {"schema": 1, "state": "installed", "domain": self.domain,
                "roles": [self.role(argument)["label"] for argument in ROLES]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "apply"))
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if sys.platform != "darwin" or os.getuid() != os.geteuid():
            raise Conflict("service installation requires the ordinary macOS host user")
        home = Path(pwd.getpwuid(os.getuid()).pw_dir)
        installer = Installer(home, os.getuid(), arguments.binary, arguments.candidate or arguments.binary)
        if arguments.action == "preflight":
            encoded = (json.dumps(installer.preflight()) + "\n").encode()
            if len(encoded) > MAX_BYTES:
                raise Conflict("complete installation plan exceeds its bound")
            atomic_file(arguments.plan, encoded)
            print(json.dumps({"schema": 1, "state": "preflight-complete"}))
        else:
            print(json.dumps(installer.apply(json.loads(read_file(arguments.plan)))))
        return 0
    except (Conflict, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as reason:
        print("service installation conflict/incomplete: " + str(reason), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
