#!/usr/bin/env python3
"""Verify native admission through the actual installed Reap service executable."""

import argparse
import hashlib
import json
import os
import pathlib
import platform
import plistlib
import re
import shutil
import stat
import subprocess
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("unit_executable", type=pathlib.Path)
    parser.add_argument("--cwd", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("the native macOS runtime is required")
    installed = pathlib.Path.home() / ".cargo/bin/reap"
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
    receipt = {"host": platform.node(), "runtime": str(runtime), "cwd": str(cwd),
               "labels_absent": [], "installed_binary": str(installed), "native_exit": None}
    result = 1
    try:
        assert launchctl("print", domain).returncode == 0
        namespace = launchctl("print", domain).stdout
        assert not any(name in namespace for name in endpoints.values()), "a fixed endpoint already exists; preserve foreign ownership"
        assert command("/usr/bin/codesign", "--verify", "--strict", str(installed)).returncode == 0
        requirement = command("/usr/bin/codesign", "-d", "-r-", str(installed))
        assert requirement.returncode == 0
        receipt["designated_requirement"] = requirement.stdout + requirement.stderr
        receipt["installed_sha256"] = sha256(installed)
        client = runtime / "reap-ingress-client"
        shutil.copyfile(unit, client)
        client.chmod(0o700)
        receipt["client_sha256"] = sha256(client)
        home = runtime / "caller-home"
        (home / ".config/reap").mkdir(parents=True)
        config = b'{"roots":["~/caller-only"],"governed_roots":[]}'
        (home / ".config/reap/config.json").write_bytes(config)
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
            boot = launchctl("bootstrap", domain, str(plist))
            assert boot.returncode == 0, boot.stderr
            owned.append((target, arguments))
        environment = dict(os.environ)
        for name in ("DYLD_INSERT_LIBRARIES", "REAP_INTERNAL_CONFIG_SNAPSHOT",
                     "REAP_NATIVE_CONTROL_SERVICE", "REAP_NATIVE_CONTROL_PROBE"):
            environment.pop(name, None)
        environment.update({
            "REAP_INSTALLED_INGRESS_PROBE": str(runtime),
            "REAP_INSTALLED_INGRESS_EXPECT_CWD": str(cwd),
            "REAP_INSTALLED_INGRESS_EXPECT_FILE_ID": f"{cwd.stat().st_ino:016x}",
            "REAP_INSTALLED_INGRESS_EXPECT_CONFIG_SHA256": hashlib.sha256(config).hexdigest(),
            "PWD": "/not-the-native-caller-directory",
        })
        with (runtime / "client.log").open("wb") as log:
            run = subprocess.run([str(client), "--exact", "service::tests::native::server_process", "--nocapture"],
                                 env=environment, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, timeout=6)
        receipt["native_exit"] = run.returncode
        if run.returncode != 0:
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
        assert evidence["metadata_only"] and not evidence["cleanup_granted"]
        assert evidence["raw_admission_refused"] and evidence["expired_ticket_exact_replay"]
        assert evidence["admission_elapsed_ns"] < 1_000_000_000
        records = list((runtime / "writer-state/reap/admission").glob("j*.json"))
        assert len(records) == 1, "only one exact durable intent may exist"
        receipt["admission_elapsed_ns"] = evidence["admission_elapsed_ns"]
        receipt["durable_jobs"] = len(records)
        result = 0
    except Exception as reason:
        receipt["failure"] = str(reason)
    finally:
        safe_to_remove = True
        for target, arguments in reversed(owned):
            state = launchctl("print", target)
            if state.returncode == 0:
                match = re.search(r"arguments = \{\n(.*?)\n\s*\}", state.stdout, re.S)
                actual = [line.strip() for line in match[1].splitlines()] if match else []
                if actual != arguments:
                    safe_to_remove = False
                    continue
                launchctl("bootout", target)
            for _ in range(100):
                if launchctl("print", target).returncode == 113:
                    receipt["labels_absent"].append(target)
                    break
                time.sleep(0.01)
            else:
                safe_to_remove = False
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
