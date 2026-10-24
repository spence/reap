#!/usr/bin/env python3
"""Verify native admission and read-only execution through the installed Reap services."""

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


def validate_execution(evidence, frame, survivor):
    """Bind actual terminal work to its original and prepared request key."""
    assert not evidence["metadata_only"] and evidence["read_only_execution"]
    assert not evidence["cleanup_granted"]
    terminal = evidence["terminal"]
    assert terminal["state"] == "completed"
    assert terminal["key"] == evidence["key"] == evidence["prepared_key"]
    assert evidence["receipt"]["key"] == evidence["key"]
    assert terminal["job"]["key"] == terminal["key"]
    assert terminal["job"]["result"]["exit_code"] == 0
    assert terminal["job"]["result"]["effects"] == 0
    assert evidence["raw_admission_refused"] and evidence["expired_ticket_exact_replay"]
    assert evidence["admission_elapsed_ns"] < 1_000_000_000
    assert len(frame) <= 128 * 1024 and frame[:4] == b"REJ1"
    assert int.from_bytes(frame[4:8], "big") == len(frame) - 40
    assert hashlib.sha256(frame[40:]).digest() == frame[8:40]
    job = json.loads(frame[40:])
    assert job["document"] == "Job" and job["view"]["key"] == terminal["key"]
    assert job["view"] == terminal["job"]
    assert job["view"]["completed_at"] >= job["view"]["created_at"]
    assert job["execution"]["phase"] == "result"
    assert job["execution"]["terminal"] == job["view"]["result"]
    launch = job["launch"]
    assert launch["resources"]["slot"] == "bulk"
    assert launch["worker"]["state"]["phase"] in ("exited", "boot_ended")
    assert all(helper is None or helper["state"]["phase"] in ("exited", "boot_ended")
               for helper in launch["helpers"])
    assert survivor == b"installed protected survivor"
    assert len(evidence["job_cli"]) == 2
    assert all(item["state"] == "completed" and item["key"] == evidence["key"]
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
            bootstrap_owned(owned, receipt, output, domain, target, arguments, plist)
        environment = dict(os.environ)
        for name in ("DYLD_INSERT_LIBRARIES", "REAP_INTERNAL_CONFIG_SNAPSHOT",
                     "REAP_NATIVE_CONTROL_SERVICE", "REAP_NATIVE_CONTROL_PROBE"):
            environment.pop(name, None)
        environment.update({
            "REAP_INSTALLED_INGRESS_PROBE": str(runtime),
            "REAP_INSTALLED_INGRESS_BINARY": str(installed),
            "REAP_INSTALLED_INGRESS_EXPECT_CWD": str(cwd),
            "REAP_INSTALLED_INGRESS_EXPECT_FILE_ID": f"{cwd.stat().st_ino:016x}",
            "REAP_INSTALLED_INGRESS_EXPECT_CONFIG_SHA256": hashlib.sha256(config).hexdigest(),
            "PWD": "/not-the-native-caller-directory",
        })
        with (runtime / "client.log").open("wb") as log:
            run = subprocess.run([str(client), "--exact", "service::tests::native::server_process", "--nocapture"],
                                 env=environment, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, timeout=20)
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
        records = list((runtime / "writer-state/reap/admission").glob("j*.json"))
        assert len(records) == 1, "only one exact durable intent may exist"
        terminal = validate_execution(evidence, records[0].read_bytes(),
                                      (runtime / "check-project/valuable").read_bytes())
        receipt["admission_elapsed_ns"] = evidence["admission_elapsed_ns"]
        receipt["durable_jobs"] = len(records)
        receipt["read_only_execution"] = True
        receipt["terminal_exit_code"] = terminal["job"]["result"]["exit_code"]
        receipt["terminal_effects"] = terminal["job"]["result"]["effects"]
        receipt["released_bulk_launch"] = True
        receipt["protected_survivor"] = True
        receipt["job_cli_completed"] = True
        receipt["unknown_job_cli_not_success"] = True
        result = 0
    except Exception as reason:
        receipt["failure"] = str(reason)
    finally:
        # a failed execution proof can retain an owned child and its recovery records.
        safe_to_remove = result == 0
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
