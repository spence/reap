#!/usr/bin/env python3
"""Freeze and verify Micro-signed native Reap proof binaries on internal storage."""

import argparse
import hashlib
import json
import pathlib
import platform
import re
import shutil
import subprocess
import tempfile


def internal(path):
    path = path.resolve(strict=True)
    if path.stat().st_dev != pathlib.Path("/System/Volumes/Data").stat().st_dev:
        raise ValueError(f"native proof input is not on internal storage: {path}")
    return path


def command(*arguments):
    return subprocess.run(arguments, check=True, capture_output=True, text=True, timeout=15)


def inspect(path, expected):
    path = internal(path)
    command("codesign", "--verify", "--strict", str(path))
    with tempfile.TemporaryDirectory(prefix="reap-native-cert-", dir="/private/tmp") as temporary:
        prefix = pathlib.Path(temporary) / "cert"
        command("codesign", "-d", "--extract-certificates=" + str(prefix), str(path))
        leaf = hashlib.sha1(prefix.with_name("cert0").read_bytes()).hexdigest().upper()
    requirement = command("codesign", "-d", "-r-", str(path))
    designated = [line for line in (requirement.stdout + requirement.stderr).splitlines()
                  if line.startswith("designated =>")]
    if leaf != expected["micro_leaf_sha1"] or designated != [expected["designated_requirement"]]:
        raise ValueError(f"native identity differs from its retained Micro reference: {path}")
    return {
        "path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "micro_leaf_sha1": leaf, "designated_requirement": designated[0],
        "identifier": expected["identifier"], "bytes": path.stat().st_size,
        "storage_device": path.stat().st_dev,
    }


def freeze(arguments):
    reference = json.loads(internal(arguments.reference).read_text())
    sources = {}
    for item in arguments.input:
        name, separator, raw = item.partition("=")
        if not separator or name not in reference or name in sources or not re.fullmatch(r"[a-z-]+", name):
            raise ValueError("each input needs a unique reference name and source path")
        if reference[name]["micro_leaf_sha1"] != arguments.identity:
            raise ValueError("the selected identity differs from the retained Micro certificate")
        sources[name] = internal(pathlib.Path(raw))
        if not sources[name].is_file():
            raise ValueError("native proof sources must be files")
    output = internal(arguments.output.parent) / arguments.output.name
    output.mkdir(exist_ok=False)
    records = {}
    for name, source in sources.items():
        destination = output / name
        shutil.copyfile(source, destination)
        destination.chmod(0o700)
        expected = reference[name]
        command("codesign", "--force", "--sign", arguments.identity,
                "--identifier", expected["identifier"], "--timestamp=none", str(destination))
        records[name] = inspect(destination, expected)
        records[name]["source"] = str(source)
    manifest = {"artifacts": records}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def verify(arguments):
    expected = json.loads(internal(arguments.manifest).read_text())["artifacts"]
    if not expected:
        raise ValueError("native manifest has no artifacts")
    directory = internal(arguments.directory)
    records = {}
    for name, reference in expected.items():
        if not re.fullmatch(r"[a-z-]+", name):
            raise ValueError("native manifest has an invalid artifact name")
        records[name] = inspect(directory / name, reference)
        if records[name]["sha256"] != reference["sha256"]:
            raise ValueError(f"native bytes differ from the frozen manifest: {name}")
    return {"artifacts": records}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    create = actions.add_parser("freeze")
    create.add_argument("--reference", type=pathlib.Path, required=True)
    create.add_argument("--output", type=pathlib.Path, required=True)
    create.add_argument("--identity", required=True)
    create.add_argument("--input", action="append", required=True)
    observe = actions.add_parser("verify")
    observe.add_argument("--manifest", type=pathlib.Path, required=True)
    observe.add_argument("--directory", type=pathlib.Path, required=True)
    arguments = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("native macOS code signatures are required")
    if arguments.action == "freeze" and not re.fullmatch(r"[0-9A-F]{40}", arguments.identity):
        parser.error("freeze requires an explicit certificate hash, not ad-hoc signing")
    result = freeze(arguments) if arguments.action == "freeze" else verify(arguments)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
