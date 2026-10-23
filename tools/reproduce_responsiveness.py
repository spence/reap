#!/usr/bin/env python3
"""Reproduce blocking boundaries with isolated read-only Reap commands."""

import argparse
import fcntl
import hashlib
import json
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path


def observe(binary, args, env, directory, deadline):
    """Bound only this harness's process group and preserve the command's output."""
    started = time.monotonic()
    process = subprocess.Popen(
        [str(binary), *args], env=env, cwd=directory,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
    )
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=deadline)
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(process.pid, signal.SIGTERM)
        try:
            stdout, stderr = process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate(timeout=2)
    return {
        "command": [str(binary), *args], "pid": process.pid, "pgid": process.pid,
        "deadline_seconds": deadline, "deadline_reached": timed_out,
        "elapsed_seconds": time.monotonic() - started, "exit_code": process.returncode,
        "stdout": stdout.decode(errors="replace"), "stderr": stderr.decode(errors="replace"),
    }


def overlap(binary, args, env, directory):
    """Demonstrate two simultaneous discovery requests without live project work."""
    processes = []
    started = time.monotonic()
    try:
        for _ in range(2):
            processes.append(subprocess.Popen(
                [str(binary), *args], env=env, cwd=directory,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
            ))
        time.sleep(0.75)
        alive = sum(process.poll() is None for process in processes)
        assert alive == 2, "One request unexpectedly suppressed the duplicate"
    finally:
        for process in processes:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
        for process in processes:
            process.communicate(timeout=2)
    return {"pgids": [process.pid for process in processes], "simultaneously_alive": alive,
            "elapsed_seconds": time.monotonic() - started,
            "exit_code": [process.returncode for process in processes], "deadline_reached": True}


def unreadable_keep(binary, env, destination, governed):
    """Preserve a hidden KEEP and observe only the installed binary's dry-run proposal."""
    parent = governed / "unreadable-example" / "expired"
    child = parent / "keep-child"
    child.mkdir(parents=True)
    (parent / ".reap").write_text(json.dumps({"version": 1, "expires": "2020-01-01T00:00:00Z"}))
    (child / ".reap").write_text(json.dumps({"version": 1, "keep": {"reason": "must survive"}}))
    sentinel = child / "sentinel"
    sentinel.write_text("must survive")
    for path in sorted(parent.rglob("*"), key=lambda path: len(path.parts), reverse=True):
        os.utime(path, (1500000000, 1500000000))
    os.utime(parent, (1500000000, 1500000000))
    config = json.loads(env["REAP_INTERNAL_CONFIG_SNAPSHOT"])
    config["reap_file_min_age_minutes"] = 0
    env = env | {"REAP_INTERNAL_CONFIG_SNAPSHOT": json.dumps(config)}
    try:
        child.chmod(0)
        try:
            list(child.iterdir())
        except PermissionError:
            pass
        else:
            raise AssertionError("The fixture must be unreadable to this account")
        result = observe(binary, ["files", str(governed)], env, destination, 12)
        assert not result["deadline_reached"] and result["exit_code"] == 0
        assert "would quarantine " + str(parent) in result["stdout"]
        assert not result["stderr"]
    finally:
        child.chmod(0o755)
    assert sentinel.read_text() == "must survive"
    result["hidden_keep_preserved"] = True
    return result


def failed_tracked_probe(binary, env, destination, governed):
    """Distinguish a tracked declaration from an unavailable Git answer."""
    workspace = governed / "git-failure" / "workspace"
    workspace.mkdir(parents=True)
    declaration = workspace / ".reap"
    declaration.write_text(json.dumps({
        "version": 1, "expires": "2020-01-01T00:00:00Z", "scratch": True,
    }))
    git = shutil.which("git")
    assert git is not None
    subprocess.run([git, "init", "-q", str(workspace)], check=True)
    subprocess.run([git, "-C", str(workspace), "add", ".reap"], check=True)
    for path in sorted(workspace.rglob("*"), key=lambda path: len(path.parts), reverse=True):
        os.utime(path, (1500000000, 1500000000))
    os.utime(workspace, (1500000000, 1500000000))
    config = json.loads(env["REAP_INTERNAL_CONFIG_SNAPSHOT"])
    config["reap_file_min_age_minutes"] = 0
    env = env | {"REAP_INTERNAL_CONFIG_SNAPSHOT": json.dumps(config)}
    control = observe(binary, ["files", str(governed)], env, destination, 12)
    assert "tracked by git" in control["stdout"]
    failed_bin = destination / "failed-git-bin"
    failed_bin.mkdir()
    failed_git = failed_bin / "git"
    failed_git.write_text("#!/bin/sh\nprintf '%s\\n' 'fatal: diagnostic Git failure' >&2\nexit 128\n")
    failed_git.chmod(0o755)
    failed_env = env | {"PATH": str(failed_bin) + os.pathsep + env.get("PATH", "")}
    result = observe(binary, ["files", str(governed)], failed_env, destination, 12)
    assert not result["deadline_reached"] and result["exit_code"] == 0
    assert "would quarantine " + str(workspace) in result["stdout"]
    assert declaration.is_file() and (workspace / ".git").is_dir()
    result["tracked_control_stdout"] = control["stdout"]
    result["tracked_workspace_preserved"] = True
    return result


def reproduce(binary, destination):
    """Demonstrate subprocess, state-read, and read-only lock waits without live state."""
    destination.mkdir(parents=True, exist_ok=False)
    governed = destination / "projects"
    kept = governed / "example" / "workspace"
    kept.mkdir(parents=True)
    declaration = kept / ".reap"
    declaration.write_text(json.dumps({"version": 1, "keep": {"reason": "owned diagnostic fixture"}}))
    sentinel = kept / "retained.txt"
    sentinel.write_text("must survive every read-only probe\n")
    config = {"roots": [], "governed_roots": [str(governed)], "reap_file_grace_hours": 0}
    base_env = dict(os.environ)
    base_env.pop("REAP_PROFILE_READ_ONLY", None)
    base_env["REAP_INTERNAL_CONFIG_SNAPSHOT"] = json.dumps(config)

    def environment(name):
        state_home = destination / name / "state"
        state = state_home / "reap"
        state.mkdir(parents=True)
        return base_env | {"XDG_STATE_HOME": str(state_home)}, state

    results = {}
    env, _ = environment("control")
    results["healthy_files"] = observe(binary, ["files", str(governed)], env, destination, 5)
    assert results["healthy_files"]["exit_code"] == 0

    fake_bin = destination / "fake-bin"
    fake_bin.mkdir()
    git = fake_bin / "git"
    git.write_text("#!/bin/sh\nexec /bin/sleep 60\n")
    git.chmod(0o755)
    env, _ = environment("stalled-git")
    env["PATH"] = str(fake_bin) + os.pathsep + env.get("PATH", "")
    results["stalled_git"] = observe(binary, ["files", str(governed)], env, destination, 0.75)
    assert results["stalled_git"]["deadline_reached"]
    assert not results["stalled_git"]["stdout"]
    results["duplicate_scans"] = overlap(binary, ["files", str(governed)], env, destination)

    env, state = environment("stalled-discovery-state")
    os.mkfifo(state / "reap-files-seen.json")
    results["stalled_seen_read"] = observe(binary, ["files", str(governed)], env, destination, 0.75)
    assert results["stalled_seen_read"]["deadline_reached"]
    assert not results["stalled_seen_read"]["stdout"]

    env, state = environment("stalled-status-state")
    os.mkfifo(state / "maintenance-last.json")
    results["stalled_status_read"] = observe(binary, ["status"], env, destination, 0.75)
    assert results["stalled_status_read"]["deadline_reached"]
    assert "read-only" in results["stalled_status_read"]["stdout"]

    env, state = environment("locked-read-only-list")
    with (state / "state.lock").open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        results["locked_lease_list"] = observe(binary, ["lease", "list"], env, destination, 33)
    locked = results["locked_lease_list"]
    assert not locked["deadline_reached"] and locked["exit_code"] == 1
    assert locked["elapsed_seconds"] >= 29
    assert "busy" in locked["stderr"] and not locked["stdout"]
    env, _ = environment("unreadable-keep")
    results["unreadable_keep"] = unreadable_keep(binary, env, destination, governed)
    env, _ = environment("failed-tracked-probe")
    results["failed_tracked_probe"] = failed_tracked_probe(binary, env, destination, governed)

    assert sentinel.read_text() == "must survive every read-only probe\n"
    assert declaration.is_file()
    assert not list(destination.rglob("index.json"))
    groups = {value["pgid"] for value in results.values() if "pgid" in value}
    groups.update(results["duplicate_scans"]["pgids"])
    processes = subprocess.check_output(["/bin/ps", "-axo", "pid=,pgid="], text=True)
    remaining = [line.strip() for line in processes.splitlines() if len(line.split()) == 2 and int(line.split()[1]) in groups]
    assert not remaining, remaining
    receipt = {
        "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "hostname": os.uname().nodename, "results": results,
        "preserved": True, "remaining_processes_in_owned_groups": remaining,
        "scope": "Synthetic blocking controls, not a simulation of the Mini filesystem or proof of a new execution design.",
    }
    (destination / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({name: {k: value[k] for k in ["elapsed_seconds", "exit_code", "deadline_reached"]} for name, value in results.items()}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path.home() / ".cargo/bin/reap")
    parser.add_argument("--destination", type=Path, required=True)
    options = parser.parse_args()
    reproduce(options.binary.resolve(), options.destination.resolve())
