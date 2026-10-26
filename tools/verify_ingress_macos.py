#!/usr/bin/env python3
"""Verify native admission and read-only execution through the installed Reap services."""

import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import platform
import plistlib
import pwd
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time


def command(*arguments):
    return subprocess.run(arguments, capture_output=True, text=True, timeout=5)


def launchctl(*arguments):
    return command("/bin/launchctl", *arguments)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def service_installer():
    """Import the installer for its read-only ownership and native observation methods."""
    source = pathlib.Path(__file__).with_name("install_services_macos.py")
    spec = importlib.util.spec_from_file_location("reap_service_installer", source)
    module = importlib.util.module_from_spec(spec)
    previous = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module


def managed_snapshot(installer, roles, probe=False):
    """Require the complete installed cohort without adoption or installer metadata writes."""
    plan = installer.preflight()
    state = plan["state"]
    assert set(state["roles"]) == set(roles), "the complete managed cohort is required"
    assert sha256(installer.binary) == installer.sha256, "installed binary changed during ownership observation"
    assert installer.json_command(installer.binary, "__service-profile") == installer.profile, "installed profile changed"
    observed = installer.inspect(state)
    for argument in roles:
        entry = state["roles"][argument]
        assert entry["phase"] == "installed" and "replacement" not in entry, "managed role installation is incomplete"
        assert entry["binary_sha256"] == installer.sha256, "managed role has a different installed binary"
        assert entry["profile"] == installer.profile, "managed role has a different installed profile"
        assert observed[argument] is not None, "managed role is not registered"
        assert observed[argument]["intent"] == entry["intent"], "managed registration has a different creation intent"
    if probe:
        for argument in roles:
            installer.probe(argument)
        return managed_snapshot(installer, roles)
    assert all(observed[argument]["identity"] is not None for argument in roles), "managed role process is absent"
    # only exact role evidence leaves this function; the production ownership record is not copied.
    return {"domain": installer.domain, "profile": installer.profile, "installed_sha256": installer.sha256,
            "roles": [{**installer.role(argument), "intent": state["roles"][argument]["intent"],
                       "identity": observed[argument]["identity"]} for argument in roles],
            "ownership_state_sha256": hashlib.sha256(json.dumps(state, sort_keys=True).encode()).hexdigest()}


def job_filename(key):
    """Match journal.rs job_name after validation of the complete wire key."""
    assert isinstance(key, dict) and set(key) == {"generation", "epoch", "nonce"}, "invalid fixture journal key"
    assert all(isinstance(key[name], str) and re.fullmatch(r"[0-9a-f]{32}", key[name])
               for name in ("generation", "nonce")), "invalid fixture journal token"
    assert type(key["epoch"]) is int and 0 < key["epoch"] <= 0xffffffffffffffff, "invalid fixture journal epoch"
    return f"j{key['epoch']:016x}-{key['nonce']}.json"


def execution_record(root, evidence, managed=False):
    """Read one bounded fixture record; only isolated mode enumerates its private journal."""
    if managed:
        name = job_filename(evidence["key"])
    else:
        records = list(root.glob("j*.json"))
        assert len(records) == 1, "only one exact durable intent may exist"
        name = records[0].name
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        parent = os.fstat(directory)
        assert parent.st_uid == os.geteuid() and not parent.st_mode & 0o022, "unsafe fixture journal directory"
        descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(descriptor, "rb") as source:
            before = os.fstat(source.fileno())
            assert stat.S_ISREG(before.st_mode) and before.st_uid == os.geteuid() \
                and before.st_nlink == 1 and not before.st_mode & 0o022, "unsafe fixture journal record"
            data = source.read(2 * 1024 * 1024 + 1)
            after = os.fstat(source.fileno())
            assert len(data) <= 2 * 1024 * 1024 and len(data) == before.st_size, "fixture journal record exceeds its bound"
            assert (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) == (
                after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns), "fixture journal record changed"
            return data, name
    finally:
        os.close(directory)


def bootstrap_owned(owned, receipt, output, domain, target, arguments, plist):
    """Record an attempted creation before its result can become ambiguous."""
    owned.append((target, arguments))
    receipt.setdefault("ownership_intents", []).append({
        "target": target, "arguments": arguments, "plist": str(plist)
    })
    with (output / "ownership.json").open("w") as evidence:
        json.dump(receipt["ownership_intents"], evidence, indent=2)
        evidence.flush()
        os.fsync(evidence.fileno())
    boot = launchctl("bootstrap", domain, str(plist))
    assert boot.returncode == 0, boot.stderr


def cleanup_owned(owned, receipt):
    """Reconcile exact attempted labels; unknown ownership remains explicit."""
    safe = True
    for target, expected in reversed(owned):
        try:
            state = launchctl("print", target)
            if state.returncode == 113:
                receipt["labels_absent"].append(target)
                continue
            arguments = re.search(r"arguments = \{\n(.*?)\n\s*\}", state.stdout, re.S)
            actual = [line.strip() for line in arguments[1].splitlines()] if arguments else []
            if state.returncode != 0 or actual != expected:
                raise RuntimeError("registered program ownership is not confirmed")
            launchctl("bootout", target)
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                state = launchctl("print", target)
                if state.returncode == 113:
                    receipt["labels_absent"].append(target)
                    break
                if state.returncode != 0:
                    raise RuntimeError("service absence is not confirmed")
                time.sleep(0.01)
            else:
                raise RuntimeError("owned service remains registered")
        except Exception as reason:
            safe = False
            receipt.setdefault("unresolved_ownership", []).append({"target": target, "reason": str(reason)})
    return safe


def validate_first_seen_rounds(rounds, frames):
    """Require installed publication and identity-bound credit across an actual rename."""
    assert len(rounds) == len(frames) == 2
    initial = None
    for index, (item, data) in enumerate(zip(rounds, frames)):
        terminal = item["terminal"]
        assert terminal["protocol"] == 2 and terminal["state"] == "completed"
        assert terminal["key"] == item["key"] == item["receipt"]["key"]
        assert len(data) <= 2 * 1024 * 1024 and data[:4] == b"REJ1"
        assert int.from_bytes(data[4:8], "big") == len(data) - 40
        assert hashlib.sha256(data[40:]).digest() == data[8:40]
        job = json.loads(data[40:])
        assert job["document"] == "Job" and job["view"] == terminal["job"]
        assert job["view"]["key"] == item["key"] and job["input"] == item["receipt"]["input"]
        result = job["view"]["result"]
        assert type(result["exit_code"]) is type(result["effects"]) is int
        assert result["exit_code"] == result["effects"] == 0
        assert job["execution"] == {"phase": "result", "terminal": result}
        request = json.loads(job["input"]["request"])
        assert request["argv"][0] == "files" and len(request["argv"]) == 2
        path = pathlib.PurePath(request["argv"][1])
        assert path.is_absolute() and path.name == ("lifetime-workspace" if index == 0 else "renamed-workspace")
        identity = {name: {field: item["identity"][name][field] for field in ("volume_uuid", "file_id")}
                    for name in ("directory", "declaration")}
        digest = item["policy_digest"]
        assert re.fullmatch(r"[0-9a-f]{64}", digest)
        key = hashlib.sha256(json.dumps([identity, digest], separators=(",", ":")).encode()).hexdigest()
        observations = job["observations"]
        assert set(observations["first_seen"]) == {key}
        first = observations["first_seen"][key]
        assert type(first) is type(observations["at"]) is int
        assert 0 < first <= observations["at"] <= job["view"]["completed_at"]
        detail = json.loads(result["detail"])
        report = detail["reports"][0]
        assert report["coverage_complete"] is True and type(report["exit_code"]) is int and report["exit_code"] == 0
        assert all(type(report[name]) is int for name in ("declarations", "honored", "invalid", "planned_units", "blocked_units", "completed_units"))
        assert report["declarations"] == 1 and report["honored"] == index and report["invalid"] == 1 - index
        if index == 0:
            assert "new; disposal takes effect in" in detail["stdout"]
        assert report["planned_units"] == index and report["blocked_units"] == report["completed_units"] == 0
        assert job["launch"]["worker"]["state"]["phase"] == "exited"
        assert all(helper is None or helper["state"]["phase"] == "exited" for helper in job["launch"]["helpers"])
        if initial is None:
            assert first == observations["at"] and first >= job["view"]["created_at"]
            initial = (identity, digest, key, first, path)
        else:
            assert (identity, digest, key, first) == initial[:4]
            assert path.parent == initial[4].parent and item["key"] != rounds[0]["key"]
            assert observations["at"] >= first + 2


def validate_execution(evidence, frame, survivor, operation=None, cancel=False, workspace_scope=False, result_pages=False):
    """Bind actual terminal work to its original and prepared request key."""
    assert not evidence["metadata_only"] and evidence["read_only_execution"]
    assert not evidence["cleanup_granted"]
    terminal = evidence["terminal"]
    expected_state = "failed" if cancel else "completed"
    assert terminal["state"] == expected_state
    assert terminal["key"] == evidence["key"] == evidence["prepared_key"]
    assert evidence["receipt"]["key"] == evidence["key"]
    assert terminal["job"]["key"] == terminal["key"]
    assert type(terminal["job"]["result"]["exit_code"]) is int
    assert type(terminal["job"]["result"]["effects"]) is int
    assert terminal["job"]["result"]["exit_code"] == (1 if cancel else 0)
    assert terminal["job"]["result"]["effects"] == 0
    assert evidence["raw_admission_refused"] and evidence["expired_ticket_exact_replay"]
    assert evidence["admission_elapsed_ns"] < 1_000_000_000
    assert len(frame) <= 2 * 1024 * 1024 and frame[:4] == b"REJ1"
    assert int.from_bytes(frame[4:8], "big") == len(frame) - 40
    assert hashlib.sha256(frame[40:]).digest() == frame[8:40]
    job = json.loads(frame[40:])
    paged_profile = job.get("execution") is not None and job["view"].get("invocation", {}).get("operation") == "files"
    assert len(frame) <= (2 * 1024 * 1024 if paged_profile else 128 * 1024)
    assert job["document"] == "Job" and job["view"]["key"] == terminal["key"]
    assert type(job["view"]["created_at"]) is int and type(job["view"]["completed_at"]) is int
    assert job["view"] == terminal["job"]
    assert job["view"]["completed_at"] >= job["view"]["created_at"]
    assert job["execution"]["phase"] == "result"
    assert job["execution"]["terminal"] == job["view"]["result"]
    if operation is not None:
        assert job["input"] == evidence["receipt"]["input"]
        request = json.loads(job["input"]["request"])
        assert request["argv"][0] == operation
        assert job["view"]["invocation"]["operation"] == operation
        assert job["view"]["invocation"]["mode"] == "read"
        detail = json.loads(terminal["job"]["result"]["detail"])
        if result_pages:
            assert set(detail) == {"paged_result"}
            manifest = detail["paged_result"]
            assert set(manifest) == {"bytes", "pages", "sha256"}
            assert type(manifest["bytes"]) is int and 4096 < manifest["bytes"] <= 512 * 1024
            assert type(manifest["pages"]) is int and 1 <= manifest["pages"] <= 65
            assert re.fullmatch(r"[0-9a-f]{64}", manifest["sha256"])
            receipts = evidence["result_pages_cli"]
            assert len(receipts) == manifest["pages"]
            texts = []
            for index, item in enumerate(receipts):
                assert item["protocol"] == 2 and item["state"] == "completed"
                assert item["key"] == terminal["key"] and item["job"] == terminal["job"]
                page = item["page"]
                assert type(page["index"]) is int and page["index"] == index
                assert page["manifest"] == manifest
                assert isinstance(page["text"], str) and 0 < len(page["text"].encode()) <= 8192
                texts.append(page["text"])
            assert job["pages"] == texts
            missing = evidence["missing_page_cli"]
            assert missing["key"] == terminal["key"] and missing["state"] != "completed" and missing.get("page") is None
            unknown = evidence["unknown_page_cli"]
            assert unknown["state"] == "unknown" and unknown["job"] is None and unknown.get("page") is None
            assert unknown["key"] == evidence["unknown_job_cli"]["key"] != terminal["key"]
            body = "".join(texts).encode()
            assert len(body) == manifest["bytes"] and hashlib.sha256(body).hexdigest() == manifest["sha256"]
            detail = json.loads(body)
        if not cancel:
            assert detail["command"] == operation
        if operation == "files" and not cancel:
            assert type(detail["requested_roots"]) is int and type(detail["present_roots"]) is int
            roots = 24 if result_pages else 1
            assert detail["requested_roots"] == detail["present_roots"] == roots
            assert len(detail["reports"]) == roots
            expected = {"exit_code": 0, "roots": 1, "declarations": 0, "honored": 0,
                        "invalid": 0, "undeclared": 2, "planned_units": 0, "blocked_units": 0,
                        "completed_units": 0, "unknown_residual_units": 0, "planned_bytes": 0,
                        "completed_bytes": 0, "coverage_complete": True}
            if workspace_scope:
                expected.update(declarations=2, honored=2, undeclared=0)
                selected = pathlib.PurePath(request["argv"][1])
                assert selected.is_absolute() and selected.name == "workspace"
                assert detail["selections"] == [{"path": str(selected), "governed_root": str(selected.parent)}]
            if result_pages:
                # a governed-root policy does not declare its loose sentinel
                expected.update(declarations=1, honored=1, undeclared=1)
                base = pathlib.PurePath(request["namespace"]["home"]).parent
                targets = {str(base / f"page-root-{index:02}") for index in range(24)}
                assert len(detail["selections"]) == roots
                assert {item["path"] for item in detail["selections"]} == targets
                assert all(item["path"] == item["governed_root"] for item in detail["selections"])
            for report in detail["reports"]:
                assert report.keys() == expected.keys()
                assert all(type(report[name]) is type(value) and report[name] == value
                           for name, value in expected.items())
    launch = job.get("launch")
    if cancel:
        at = terminal["job"].get("cancel_requested_at")
        assert type(at) is int and terminal["job"]["created_at"] <= at <= terminal["job"]["completed_at"]
        assert json.loads(terminal["job"]["result"]["detail"]) == {"outcome": "cancelled"}
        cancellation = evidence.get("cancellation")
        assert isinstance(cancellation, dict) and cancellation["protocol"] == 2
        assert cancellation["key"] == evidence["key"]
        assert cancellation["state"] in ("pending", "failed", "acceptance_unknown")
        acknowledged = cancellation.get("job")
        if cancellation["state"] != "acceptance_unknown":
            assert isinstance(acknowledged, dict)
            assert acknowledged["key"] == evidence["key"]
            assert acknowledged["cancel_requested_at"] == at
            if cancellation["state"] == "failed":
                assert acknowledged["result"] == terminal["job"]["result"]
            else:
                assert acknowledged["result"] is None
        else:
            assert acknowledged is None
    else:
        assert launch["resources"]["slot"] == "bulk"
    if launch is not None:
        assert launch["resources"]["slot"] in ("bulk", "resolver")
        assert launch["worker"]["state"]["phase"] in ("exited", "boot_ended")
        assert all(helper is None or helper["state"]["phase"] in ("exited", "boot_ended")
                   for helper in launch["helpers"])
    assert survivor == b"installed protected survivor"
    assert len(evidence["job_cli"]) == 2
    assert all(item["state"] == expected_state and item["key"] == evidence["key"]
               and item["job"]["result"] == terminal["job"]["result"]
               for item in evidence["job_cli"])
    assert evidence["unknown_job_cli"]["state"] == "unknown"
    assert evidence["unknown_job_cli"]["job"] is None
    return terminal


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("unit_executable", type=pathlib.Path)
    parser.add_argument("--cwd", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--managed-services", action="store_true",
                        help="use the exact installed owned cohort without registration or shutdown")
    parser.add_argument("--operation", choices=["check", "files"], default="check")
    parser.add_argument("--cancel", action="store_true",
                        help="require actual installed cancellation and its durable terminal result")
    parser.add_argument("--workspace-scope", action="store_true",
                        help="require scoped Files discovery with ancestor policy and an unrelated FIFO")
    parser.add_argument("--result-pages", action="store_true",
                        help="require complete multi-root Files output through the installed page command")
    parser.add_argument("--first-seen", action="store_true",
                        help="require installed first-seen publication and rename continuity")
    args = parser.parse_args()
    if args.workspace_scope and (args.operation != "files" or args.cancel):
        parser.error("--workspace-scope requires --operation files without --cancel")
    if args.result_pages and (args.operation != "files" or args.cancel or args.workspace_scope):
        parser.error("--result-pages requires whole-root Files without cancellation")
    if args.first_seen and (not args.managed_services or args.operation != "files" or args.cancel or args.workspace_scope or args.result_pages):
        parser.error("--first-seen requires managed whole-root Files without cancellation or paging")
    if platform.system() != "Darwin":
        parser.error("the native macOS runtime is required")
    host_home = pathlib.Path(pwd.getpwuid(os.getuid()).pw_dir) if args.managed_services else pathlib.Path.home()
    installed = host_home / ".cargo/bin/reap"
    unit = args.unit_executable.resolve(strict=True)
    cwd = args.cwd.resolve(strict=True)
    output = args.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    runtime = pathlib.Path(tempfile.mkdtemp(prefix="reap-ingress-runtime-", dir="/private/tmp"))
    uid = os.geteuid()
    domain = f"user/{uid}"
    stem = f"dev.micro.reap.proof.ingress.{os.getpid()}.{time.time_ns()}"
    endpoints = {
        "state-writer": f"dev.micro.reap.writer.v1.{uid}",
        "context-resolver": f"dev.micro.reap.resolver.v1.{uid}",
        "control": f"dev.micro.reap.control.v1.{uid}",
    }
    owned = []
    helper = None
    managed_before = None
    receipt = {"host": platform.node(), "runtime": str(runtime), "cwd": str(cwd), "operation": args.operation,
               "cancellation": args.cancel,
               "workspace_scope": args.workspace_scope,
               "first_seen": args.first_seen,
               "installed_binary": str(installed), "native_exit": None,
               "service_mode": "managed" if args.managed_services else "isolated"}
    if args.managed_services:
        receipt.update({"managed_services_preserved": False, "service_cleanup": "none_managed",
                        "production_services_stopped": False})
    else:
        receipt["labels_absent"] = []
        receipt["service_cleanup"] = "isolated_owned_attempts"
    result = 1
    try:
        if args.managed_services:
            assert os.getuid() == uid, "managed verification requires the ordinary host user"
            helper = service_installer()
            managed_before = managed_snapshot(helper.Installer(host_home, uid, installed, installed),
                                              helper.ROLES, probe=True)
            receipt["managed_services"] = managed_before
        else:
            assert launchctl("print", domain).returncode == 0
            namespace = launchctl("print", domain).stdout
            assert not any(name in namespace for name in endpoints.values()), "a fixed endpoint already exists; preserve foreign ownership"
        assert command("/usr/bin/codesign", "--verify", "--strict", str(installed)).returncode == 0
        requirement = command("/usr/bin/codesign", "-d", "-r-", str(installed))
        assert requirement.returncode == 0
        receipt["designated_requirement"] = requirement.stdout + requirement.stderr
        receipt["installed_sha256"] = sha256(installed)
        if args.managed_services:
            assert receipt["installed_sha256"] == managed_before["installed_sha256"], "installed binary changed before fixture"
        client = runtime / "reap-ingress-client"
        shutil.copyfile(unit, client)
        client.chmod(0o700)
        receipt["client_sha256"] = sha256(client)
        home = runtime / "caller-home"
        (home / ".config/reap").mkdir(parents=True)
        governed = ([str(runtime / f"page-root-{index:02}") for index in range(24)] if args.result_pages
                    else [str(runtime / "check-project")] if args.operation == "files" else [])
        config = json.dumps({"roots": ["~/caller-only"], "governed_roots": governed,
                             "reap_file_grace_hours": 0}).encode()
        (home / ".config/reap/config.json").write_bytes(config)
        if not args.managed_services:
            service_home = runtime / "service-home"
            (service_home / ".config/reap").mkdir(parents=True)
            os.mkfifo(service_home / ".config/reap/config.json", 0o600)
            for role, endpoint in endpoints.items():
                label = stem + "." + role
                target = domain + "/" + label
                assert launchctl("print", target).returncode == 113
                arguments = [str(installed), "__service", role]
                spec = {"Label": label, "LimitLoadToSessionType": "Background",
                        "ProgramArguments": arguments, "MachServices": {endpoint: True},
                        "EnvironmentVariables": {"HOME": str(service_home),
                                                 "XDG_STATE_HOME": str(runtime / "writer-state")},
                        "StandardOutPath": str(runtime / (role + ".log")),
                        "StandardErrorPath": str(runtime / (role + ".log"))}
                plist = runtime / (role + ".plist")
                plist.write_bytes(plistlib.dumps(spec))
                bootstrap_owned(owned, receipt, output, domain, target, arguments, plist)
        environment = dict(os.environ)
        if args.managed_services:
            environment.update(HOME=str(host_home), XDG_STATE_HOME=str(host_home / ".local/state"))
        for name in ("DYLD_INSERT_LIBRARIES", "REAP_INTERNAL_CONFIG_SNAPSHOT",
                     "REAP_NATIVE_CONTROL_SERVICE", "REAP_NATIVE_CONTROL_PROBE",
                     "REAP_INSTALLED_INGRESS_CANCEL", "REAP_INSTALLED_INGRESS_FILES_SCOPE",
                     "REAP_INSTALLED_INGRESS_RESULT_PAGES"):
            environment.pop(name, None)
        environment.update({
            "REAP_INSTALLED_INGRESS_PROBE": str(runtime),
            "REAP_INSTALLED_INGRESS_BINARY": str(installed),
            "REAP_INSTALLED_INGRESS_OPERATION": args.operation,
            "REAP_INSTALLED_INGRESS_EXPECT_CWD": str(cwd),
            "REAP_INSTALLED_INGRESS_EXPECT_FILE_ID": f"{cwd.stat().st_ino:016x}",
            "REAP_INSTALLED_INGRESS_EXPECT_CONFIG_SHA256": hashlib.sha256(config).hexdigest(),
            "PWD": "/not-the-native-caller-directory",
        })
        if args.cancel:
            environment["REAP_INSTALLED_INGRESS_CANCEL"] = "1"
        if args.workspace_scope:
            environment["REAP_INSTALLED_INGRESS_FILES_SCOPE"] = "1"
        if args.result_pages:
            environment["REAP_INSTALLED_INGRESS_RESULT_PAGES"] = "1"
        environment.pop("REAP_INSTALLED_INGRESS_FIRST_SEEN", None)
        if args.first_seen:
            environment["REAP_INSTALLED_INGRESS_FIRST_SEEN"] = "1"
        with (runtime / "client.log").open("wb") as log:
            run = subprocess.run([str(client), "--exact", "service::tests::native::server_process", "--nocapture"],
                                 env=environment, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, timeout=40 if args.first_seen else 20)
        receipt["native_exit"] = run.returncode
        if run.returncode != 0 and not args.managed_services:
            # inspect only this verifier's still-registered, agentless resolver before cleanup.
            target, expected = next(item for item in owned if item[1][-1] == "context-resolver")
            current = launchctl("print", target)
            arguments = re.search(r"\n\s*arguments = \{\n(.*?)\n\s*\}", current.stdout, re.S)
            actual = [line.strip() for line in arguments[1].splitlines()] if arguments else []
            pid = re.search(r"\n\s*pid = ([0-9]+)\n", current.stdout)
            if current.returncode == 0 and actual == expected and pid:
                sample = command("/usr/bin/sample", pid[1], "1", "1", "-file", str(runtime / "resolver.sample.txt"))
                receipt["resolver_sample_exit"] = sample.returncode
        assert run.returncode == 0, "installed ingress client failed"
        evidence = json.loads((runtime / "client-receipt.json").read_text())
        journal = (host_home / ".local/state/reap/admission" if args.managed_services
                   else runtime / "writer-state/reap/admission")
        frame, record_name = execution_record(journal, evidence, managed=args.managed_services)
        if args.managed_services:
            receipt.update({"journal_scope": "exact_fixture_key", "journal_key": evidence["key"],
                            "journal_record": record_name, "journal_record_sha256": hashlib.sha256(frame).hexdigest()})
        terminal = validate_execution(evidence, frame,
                                      (runtime / "check-project/valuable").read_bytes(), args.operation, args.cancel,
                                      args.workspace_scope, args.result_pages)
        if args.first_seen:
            rounds = evidence["first_seen_rounds"]
            frames = [execution_record(journal, item, managed=True)[0] for item in rounds]
            validate_first_seen_rounds(rounds, frames)
            for index, data in enumerate(frames):
                (output / f"first-seen-job-{index}.json").write_bytes(data)
            assert (runtime / "check-project/renamed-workspace/valuable").read_bytes() == b"installed grace survivor"
            receipt["first_seen_rounds"] = len(rounds)
        if args.result_pages:
            for index in range(24):
                assert (runtime / f"page-root-{index:02}/valuable").read_bytes() == b"installed paged survivor"
            receipt["result_pages"] = len(evidence["result_pages_cli"])
        if args.workspace_scope:
            project = runtime / "check-project"
            assert (project / "workspace/valuable").read_bytes() == b"scoped protected survivor"
            assert stat.S_ISFIFO((project / "unrelated/fifo").lstat().st_mode)
        if args.managed_services:
            (output / "verified-job.json").write_bytes(frame)
            receipt["verified_job_records"] = 1
        else:
            receipt["durable_jobs"] = 1
        receipt["admission_elapsed_ns"] = evidence["admission_elapsed_ns"]
        receipt["read_only_execution"] = True
        receipt["terminal_exit_code"] = terminal["job"]["result"]["exit_code"]
        receipt["terminal_effects"] = terminal["job"]["result"]["effects"]
        launch = json.loads(frame[40:]).get("launch")
        receipt["released_bulk_launch"] = bool(launch and launch["resources"]["slot"] == "bulk")
        receipt["protected_survivor"] = True
        receipt["job_cli_completed"] = True
        receipt["unknown_job_cli_not_success"] = True
        result = 0
    except Exception as reason:
        receipt["failure"] = str(reason)
    finally:
        # a failed execution proof can retain an owned child and its recovery records.
        safe_to_remove = result == 0
        if args.managed_services:
            if managed_before is not None:
                try:
                    after = managed_snapshot(helper.Installer(host_home, uid, installed, installed), helper.ROLES)
                    assert after == managed_before, "managed cohort, installed binary, or ownership metadata changed"
                    receipt["managed_services_preserved"] = True
                except Exception as reason:
                    receipt["managed_preservation_failure"] = str(reason)
                    safe_to_remove = False
        else:
            safe_to_remove = cleanup_owned(owned, receipt) and safe_to_remove
        for parent, directories, files in os.walk(runtime, followlinks=False):
            for name in files:
                path = pathlib.Path(parent) / name
                if path.name == "reap-ingress-client" or not stat.S_ISREG(path.lstat().st_mode):
                    continue
                saved = output / path.relative_to(runtime)
                saved.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, saved)
        if safe_to_remove:
            shutil.rmtree(runtime)
        else:
            result = 1
        receipt["runtime_removed"] = not runtime.exists()
        receipt["verification_exit"] = result
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps(receipt))
    return result


if __name__ == "__main__":
    raise SystemExit(main())
