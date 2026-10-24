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
        ):
            environment.pop(name, None)
        environment["REAP_NATIVE_CONTROL_FIXTURE_ROOT"] = str(runtime / "fixtures")
        with (output / "native.log").open("wb") as log:
            # this executable contains only the owned transport fixtures, not agent providers.
            run = subprocess.run(
                [str(executable), "control::tests", "--test-threads=1", "--nocapture"],
                env=environment, stdout=log, stderr=subprocess.STDOUT, timeout=20,
            )
        receipt["native_exit"] = run.returncode
        result = run.returncode
    finally:
        safe_to_remove = True
        for path in runtime.glob("fixtures/*/service.plist"):
            data = plistlib.loads(path.read_bytes())
            expected = [str(runtime / "reap-native-test"), "--exact",
                        "control::tests::native::server_process", "--nocapture"]
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
            # only this invocation's executable, empty fixtures, plists, and FIFO remain here.
            shutil.rmtree(runtime)
        receipt["runtime_removed"] = not runtime.exists()
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps(receipt))
        if not receipt["runtime_removed"]:
            result = 1
    return result


if __name__ == "__main__":
    sys.exit(main())
