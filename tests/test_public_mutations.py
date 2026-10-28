"""Require prompt mutation receipts and real effects through the installed broker."""

import fcntl
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REAP_TEST_BIN") or shutil.which("reap")
PAYLOAD = b"mutation bridge source fixture\n"


@unittest.skipUnless(BINARY, "install reap or set REAP_TEST_BIN")
class PublicMutations(unittest.TestCase):
    def setUp(self):
        base = ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(prefix="public-mutations-", dir=base)).resolve()
        self.unconfirmed = 0
        self.addCleanup(self.close_fixture)
        self.assertEqual(self.root.stat().st_dev, Path("/System/Volumes/Data").stat().st_dev)
        self.home = self.root / "home"
        self.config = self.home / ".config/reap/config.json"
        self.config.parent.mkdir(parents=True)
        self.governed = self.home / "projects"
        self.project = self.governed / "fixture"
        self.project.mkdir(parents=True)
        self.quarantine = self.root / "state/reap/quarantine"
        self.quarantine.mkdir(parents=True)
        self.config.write_text(json.dumps({
            "roots": [str(self.project)], "governed_roots": [str(self.governed)],
            "reap_file_grace_hours": 0, "reap_file_min_age_minutes": 0,
            "quarantine": {"auto_purge": False},
        }))
        self.kept = self.project / "kept"
        self.kept.mkdir()
        (self.kept / "valuable").write_bytes(PAYLOAD)
        (self.kept / ".reap").write_text(json.dumps({
            "version": 1, "keep": {"reason": "must survive applying work"}}))
        self.environment = dict(os.environ, HOME=str(self.home),
                                XDG_STATE_HOME=str(self.root / "state"), PWD="/not-native-cwd")
        for name in ("REAP_INTERNAL_CONFIG_SNAPSHOT", "REAP_OWNER", "REAP_SESSION",
                     "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
            self.environment.pop(name, None)

    def close_fixture(self):
        if self.unconfirmed:
            (self.root / ".reap").write_text(json.dumps({
                "version": 1, "keep": {"reason": "unconfirmed public mutation fixture"},
            }))
            print(f"Retained unconfirmed mutation fixture: {self.root}")
        else:
            shutil.rmtree(self.root)

    def run_cli(self, *arguments, timeout=2):
        started = time.monotonic()
        self.unconfirmed += 1
        result = subprocess.run([BINARY, *map(str, arguments)], cwd=self.root,
                                env=self.environment, capture_output=True, timeout=timeout)
        result.elapsed = time.monotonic() - started
        self.assertLess(result.elapsed, 1.25)
        try:
            result.receipt = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            self.fail(f"mutation returned no protocol receipt: {error}; stdout={result.stdout!r}")
        self.assertEqual(result.receipt["protocol"], 2)
        if result.receipt["state"] not in ("pending", "acceptance_unknown", "unknown", "capacity", "failed"):
            self.unconfirmed -= 1
        self.assertEqual((self.kept / "valuable").read_bytes(), PAYLOAD)
        return result

    def complete(self, *arguments):
        result = self.run_cli(*arguments)
        if result.returncode == 75:
            receipt = result.receipt
            self.assertIsNotNone(receipt["key"])
            self.assertIsNotNone(receipt["job_id"])
            result = subprocess.run([BINARY, "job", "wait", receipt["job_id"], "--timeout", "30"],
                                    cwd=self.root, env=self.environment, capture_output=True, timeout=32)
            result.receipt = json.loads(result.stdout)
            if result.receipt["state"] == "completed":
                self.unconfirmed -= 1
            self.assertEqual(result.receipt["key"], receipt["key"])
        self.assertEqual(result.returncode, 0, result)
        self.assertEqual(result.receipt["state"], "completed")
        self.assertGreater(result.receipt["job"]["result"]["effects"], 0)
        self.assertEqual((self.kept / "valuable").read_bytes(), PAYLOAD)
        return result

    def test_files_apply_does_not_wait_for_configuration_fifo(self):
        self.config.unlink()
        os.mkfifo(self.config)
        result = self.run_cli("files", self.governed, "--apply")
        self.assertIn(result.returncode, (1, 75))
        self.assertNotEqual(result.receipt["state"], "completed")
        self.assertEqual(result.receipt["invocation"]["operation"], "files")
        self.assertEqual(result.receipt["invocation"]["mode"], "apply")
        self.assertTrue(self.kept.exists())

    def test_lease_commands_do_not_wait_for_a_busy_legacy_state_lock(self):
        state = self.root / "state/reap"
        index = state / "leases.json"
        original = b'{"version":1,"leases":[]}\n'
        index.write_bytes(original)
        with (state / "state.lock").open("wb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            commands = [
                (("lease",), "read"),
                (("lease", "list"), "read"),
                (("lease", "add", self.project, "--ttl", "24h"), "apply"),
                (("lease", "renew", self.project), "apply"),
                (("lease", "release", self.project), "apply"),
            ]
            for command, mode in commands:
                with self.subTest(command=command[:2]):
                    result = self.run_cli(*command)
                    self.assertIn(result.returncode, (0, 1, 75) if mode == "read" else (1, 75))
                    self.assertEqual(result.receipt["invocation"]["operation"], "lease")
                    self.assertEqual(result.receipt["invocation"]["mode"], mode)
                    self.assertEqual(index.read_bytes(), original)
                    self.assertFalse((self.project / ".reap-lease").exists())
                    print(json.dumps({"command": list(map(str, command)),
                                      "elapsed_seconds": result.elapsed,
                                      "state": result.receipt["state"],
                                      "exit_code": result.returncode}))

    def test_store_commands_do_not_wait_for_manifest_fifo(self):
        (self.project / "Cargo.toml").write_text("[workspace]\n")
        manifest = self.project / ".reap.json"
        os.mkfifo(manifest)
        external = self.root / "external-store"
        external.mkdir()
        commands = [
            (("stores",), "read"),
            (("stores", self.project), "read"),
            (("stores", self.project, "--apply"), "apply"),
            (("stores", self.project, "--init"), "apply"),
            (("stores", self.project, "--bind", "runs", "--to", external), "apply"),
        ]
        for command, mode in commands:
            with self.subTest(command=command):
                result = self.run_cli(*command)
                self.assertIn(result.returncode, (1, 75))
                self.assertNotEqual(result.receipt["state"], "completed")
                self.assertEqual(result.receipt["invocation"]["operation"], "stores")
                self.assertEqual(result.receipt["invocation"]["mode"], mode)
                self.assertTrue(stat.S_ISFIFO(manifest.lstat().st_mode))
                self.assertEqual(list(external.iterdir()), [])
                print(json.dumps({"command": list(map(str, command)),
                                  "elapsed_seconds": result.elapsed,
                                  "state": result.receipt["state"],
                                  "exit_code": result.returncode}))

    def test_store_apply_does_not_wait_for_configuration_fifo(self):
        (self.project / "Cargo.toml").write_text("[workspace]\n")
        (self.project / ".reap.json").write_text(json.dumps({
            "version": 2, "stores": [{"path": "output", "disposition": "quarantine",
                "retention": {"keep_last": 0, "min_age_hours": 0, "max_age_days": 0}}],
        }))
        store = self.project / "output"
        store.mkdir()
        (store / "REAP-STORE.TAG").write_text("Signature: reap-store-marker-v1\n")
        payload = store / "old-output"
        payload.write_bytes(PAYLOAD)
        stamp = time.time() - 86400
        os.utime(payload, (stamp, stamp))
        self.config.unlink()
        os.mkfifo(self.config)
        result = self.run_cli("stores", self.project, "--apply")
        self.assertIn(result.returncode, (1, 75))
        self.assertNotEqual(result.receipt["state"], "completed")
        self.assertEqual(payload.read_bytes(), PAYLOAD)
        self.assertTrue(stat.S_ISFIFO(self.config.lstat().st_mode))

    def test_remaining_applying_commands_do_not_wait_for_configuration_fifo(self):
        self.config.unlink()
        os.mkfifo(self.config)
        commands = [
            ("stores", self.project, "--apply"),
            ("retire", self.project, "--now", "--apply", "--min-age-minutes", "0"),
        ]
        for command in commands:
            with self.subTest(command=command[0]):
                result = self.run_cli(*command)
                self.assertIn(result.returncode, (1, 75))
                self.assertNotEqual(result.receipt["state"], "completed")
                self.assertTrue(self.kept.exists())

    def test_cargo_commands_do_not_wait_for_target_marker_fifo(self):
        (self.project / "Cargo.toml").write_text(
            '[package]\nname="reap-cargo-client-fixture"\nversion="0.1.0"\n')
        target = self.project / "target"
        target.mkdir()
        (target / "CACHEDIR.TAG").write_text(
            "Signature: 8a477f597d28d172789f06886806bc55\n")
        os.mkfifo(self.project / "CACHEDIR.TAG")
        commands = [
            ("plan", self.project),
            ("plan", self.project, "--apply"),
            ("clean", self.project),
            ("clean", self.project, "--apply"),
            ("sweep",),
            ("sweep", "--apply"),
            (),
        ]
        for command in commands:
            with self.subTest(command=command):
                result = self.run_cli(*command)
                print(json.dumps({"command": list(map(str, command)), "elapsed_seconds": result.elapsed,
                                  "exit_code": result.returncode, "state": result.receipt["state"],
                                  "invocation": result.receipt.get("invocation")}))
                self.assertIn(result.returncode, (1, 75))
                self.assertNotEqual(result.receipt["state"], "completed")
                self.assertEqual(result.receipt["invocation"]["operation"],
                                 command[0] if command else "sweep")
                self.assertEqual(result.receipt["invocation"]["mode"],
                                 "apply" if "clean" in command or "--apply" in command else "read")
                self.assertTrue(self.kept.exists())

    def test_completed_files_apply_retires_only_the_authorized_unit(self):
        disposable = self.project / "disposable"
        disposable.mkdir()
        (disposable / "valuable").write_bytes(PAYLOAD)
        (disposable / ".reap").write_text(json.dumps({
            "version": 1, "expires": "2020-01-01T00:00:00Z", "scratch": True,
        }))
        past = time.time() - 86400
        for path in [*disposable.iterdir(), disposable]:
            os.utime(path, (past, past))
        result = self.complete("files", self.governed, "--apply")
        self.assertEqual(result.receipt["invocation"]["operation"], "files")
        self.assertEqual(result.receipt["invocation"]["mode"], "apply")
        self.assertFalse(disposable.exists())
        payloads = list(self.quarantine.rglob("valuable"))
        self.assertEqual(len(payloads), 1)
        self.assertEqual(payloads[0].read_bytes(), PAYLOAD)

    def test_creation_without_an_expiry_is_colocated_keep_not_a_lease(self):
        target = self.project / "new-work"
        self.complete("create", "scratch", target, "--owner", "mutation-fixture",
                      "--purpose", "active work has no disposal deadline")
        policy = json.loads((target / ".reap").read_text())
        self.assertEqual(policy["version"], 2)
        self.assertIsNone(policy.get("expires"))
        self.assertIsNone(policy.get("work"))
        self.assertFalse(policy.get("children"))
        leases = self.root / "state/reap/leases.json"
        if leases.exists():
            self.assertFalse(any(record["path"] == str(target)
                                 for record in json.loads(leases.read_text())["leases"]))


if __name__ == "__main__":
    unittest.main()
