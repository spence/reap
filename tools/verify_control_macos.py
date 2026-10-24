#!/usr/bin/env python3
"""Run the owned native transport fixtures from the internal runtime volume."""

import argparse
import hashlib
import json
import os
import pathlib
import platform
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile


def launchctl(*arguments):
    return subprocess.run(
        ["/bin/launchctl", *arguments], capture_output=True, text=True, timeout=5
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("test_executable", type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--filter", choices=["control::tests", "service::tests"], default="control::tests")
    parser.add_argument("--fault-library", type=pathlib.Path)
    arguments = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("the native macOS runtime is required")

    source = arguments.test_executable.resolve(strict=True)
    output = arguments.output.absolute()
    output.mkdir(parents=True, exist_ok=False)
    runtime = pathlib.Path(tempfile.mkdtemp(prefix="reap-control-runtime-", dir="/private/tmp"))
    receipt = {"runtime": str(runtime), "labels_absent": [], "native_exit": None}
    result = 1
    try:
        # launchd and dyld must not open the test executable on an external volume.
        assert runtime.stat().st_dev == pathlib.Path("/System/Volumes/Data").stat().st_dev
        executable = runtime / "reap-native-test"
        shutil.copyfile(source, executable)
        executable.chmod(0o700)
        digest = hashlib.sha256()
        with executable.open("rb") as binary:
            for chunk in iter(lambda: binary.read(65536), b""):
                digest.update(chunk)
        receipt["binary_sha256"] = digest.hexdigest()
        environment = dict(os.environ)
        for name in (
            "REAP_NATIVE_CONTROL_PROBE", "REAP_NATIVE_CONTROL_SERVICE",
            "REAP_INTERNAL_CONFIG_SNAPSHOT",
            "DYLD_INSERT_LIBRARIES",
        ):
            environment.pop(name, None)
        environment["REAP_NATIVE_CONTROL_FIXTURE_ROOT"] = str(runtime / "fixtures")
        selection = [str(executable), arguments.filter, "--test-threads=1", "--nocapture"]
        if arguments.fault_library:
            library = runtime / "fault_fullsync.dylib"
            shutil.copyfile(arguments.fault_library.resolve(strict=True), library)
            environment["REAP_NATIVE_CONTROL_FAULT_LIBRARY"] = str(library)
            receipt["fault_library_sha256"] = hashlib.sha256(library.read_bytes()).hexdigest()
            selection.append("--include-ignored")
        with (output / "native.log").open("wb") as log:
            # this executable contains only the owned transport fixtures, not agent providers.
            run = subprocess.run(
                selection, env=environment, stdout=log, stderr=subprocess.STDOUT, timeout=60,
            )
        receipt["native_exit"] = run.returncode
        result = run.returncode
        content = (output / "native.log").read_text()
        required = (["native_control_survives_a_real_filesystem_worker_stall"]
                    if arguments.filter == "control::tests" else [
                        "native_admission_survives_control_restart_and_writer_process_death",
                        "native_alias_writer_cannot_replace_the_locked_owner",
                        "malformed_native_requests_do_not_poison_the_writer"])
        if not all(case in content for case in required) or "test result: ok." not in content:
            result = 1
        if arguments.fault_library and (
                "native_blocked_sync_does_not_ack_early_or_replace_the_writer" not in content or
                "native_recovery_flushes_an_unacknowledged_pending_record_before_publication" not in content or
                "ignored;" not in content or "0 ignored;" not in content):
            result = 1
    finally:
        safe_to_remove = True
        for path in runtime.glob("fixtures/*/*.plist"):
            data = plistlib.loads(path.read_bytes())
            expected = [str(runtime / "reap-native-test"), "--exact",
                        arguments.filter + "::native::server_process", "--nocapture"]
            if (data["ProgramArguments"] != expected or
                    data["EnvironmentVariables"]["REAP_NATIVE_CONTROL_SERVICE"] != data["Label"]):
                safe_to_remove = False
                continue
            target = f"user/{os.getuid()}/{data['Label']}"
            if launchctl("print", target).returncode == 0:
                launchctl("bootout", target)
            if launchctl("print", target).returncode != 113:
                safe_to_remove = False
            else:
                receipt["labels_absent"].append(target)
            (output / f"{data['Label']}.plist").write_bytes(path.read_bytes())
        if safe_to_remove:
            # retain regular fixture evidence, including journal records, before runtime removal.
            fixtures = runtime / "fixtures"
            for parent, directories, files in os.walk(fixtures, followlinks=False):
                for name in files:
                    path = pathlib.Path(parent) / name
                    if not stat.S_ISREG(path.lstat().st_mode):
                        continue
                    saved = output / "fixtures" / path.relative_to(fixtures)
                    saved.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, saved)
            shutil.rmtree(runtime)
        receipt["runtime_removed"] = not runtime.exists()
        if not receipt["runtime_removed"]:
            result = 1
        receipt["verification_exit"] = result
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps(receipt))
    return result


if __name__ == "__main__":
    sys.exit(main())
