"""Require an active executable survivor and independent installed Cargo reclamation."""

import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REAP_TEST_BIN") or shutil.which("reap")
DRIVER = os.environ.get("REAP_TEST_NATIVE_DRIVER")
FAULT_LIBRARY = os.environ.get("REAP_TEST_ACTIVITY_FAULT_LIBRARY")


@unittest.skipUnless(platform.system() == "Darwin" and BINARY and DRIVER,
                     "provide the installed binary and signed native driver on macOS")
class ActiveExecutables(unittest.TestCase):
    def setUp(self):
        parent = ROOT / "target/control-t13"
        parent.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix="active-executable-", dir=parent)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.assertEqual(self.root.stat().st_dev, Path("/System/Volumes/Data").stat().st_dev)
        self.home = self.root / "home"
        self.home.mkdir()
        self.project = self.root / "project"
        self.target = self.project / "target"
        self.deps = self.target / "debug/deps"
        self.deps.mkdir(parents=True)
        (self.project / "Cargo.toml").write_text('[package]\nname="reap-active-fixture"\nversion="0.1.0"\n')
        (self.target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\n")
        self.environment = dict(os.environ, HOME=str(self.home),
                                XDG_STATE_HOME=str(self.root / "state"), TMPDIR="/private/tmp")
        for key in ("DYLD_INSERT_LIBRARIES", "REAP_INTERNAL_CONFIG_SNAPSHOT",
                    "REAP_OWNER", "REAP_SESSION"):
            self.environment.pop(key, None)
        self.child = None
        self.addCleanup(self.stop_child)

    def helper(self, path, age):
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(DRIVER, path)
        path.chmod(0o700)
        stamp = time.time() - age
        os.utime(path, (stamp, stamp))
        return path

    def start_child(self, path):
        environment = dict(self.environment, REAP_ACTIVE_EXECUTABLE_FIXTURE=str(self.root))
        self.child = subprocess.Popen(
            [str(path), "--exact", "activity::tests::executable_fixture_entry", "--nocapture"],
            cwd=self.root, env=environment, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 5
        while not (self.root / "ready").exists():
            self.assertIsNone(self.child.poll(), "native fixture exited before readiness")
            self.assertLess(time.monotonic(), deadline)
            time.sleep(.01)

    def stop_child(self):
        if self.child is None:
            return
        (self.root / "release").write_bytes(b"finish owned fixture")
        try:
            self.child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.child.terminate()
            self.child.wait(timeout=3)
        self.child.stderr.close()
        self.child = None

    def apply(self, project=None, fault=None, activity_pid=None):
        environment = self.environment
        if fault:
            environment = dict(environment, DYLD_INSERT_LIBRARIES=FAULT_LIBRARY,
                               REAP_NATIVE_ACTIVITY_FAULT=fault)
            if activity_pid is not None:
                environment["REAP_NATIVE_ACTIVITY_PID"] = str(activity_pid)
        return subprocess.run([BINARY, "plan", str(project or self.project), "--apply",
                               "--min-age-minutes", "0"], cwd=self.root,
                              env=environment, capture_output=True, timeout=10)

    def test_running_hash_survives_while_inactive_hash_is_removed_then_releases(self):
        active = self.helper(self.deps / "reap_active_fixture-0000000000000001", 86400)
        inactive = self.helper(self.deps / "reap_active_fixture-0000000000000002", 86300)
        newest = self.helper(self.deps / "reap_active_fixture-0000000000000003", 86200)
        self.start_child(active)
        result = self.apply()
        self.assertTrue(active.exists(), result.stdout + result.stderr)
        self.assertIsNone(self.child.poll())
        self.assertFalse(inactive.exists(), "inactive superseded output must remain reclaimable")
        self.assertTrue(newest.exists())
        self.assertEqual(result.returncode, 1, "partial cleanup must not claim complete success")
        self.assertIn(b"removed 1/2 planned item(s)", result.stdout)
        alias = Path("/System/Volumes/Data" + str(self.project))
        self.assertTrue(alias.samefile(self.project))
        inactive = self.helper(inactive, 86300)
        result = self.apply(alias)
        self.assertTrue(active.exists(), result.stdout + result.stderr)
        self.assertIsNone(self.child.poll())
        self.assertFalse(inactive.exists(), result.stdout + result.stderr)
        self.assertEqual(result.returncode, 1, "the Data alias must preserve the same live image")
        self.assertIn(b"removed 1/2 planned item(s)", result.stdout)
        self.stop_child()
        result = self.apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(active.exists(), "native exit releases the superseded executable")
        self.assertTrue(newest.exists())

    def test_intermediate_directory_alias_preserves_the_active_image(self):
        physical = self.target / "real-deps"
        self.deps.rename(physical)
        self.deps.symlink_to(physical, target_is_directory=True)
        active = self.helper(physical / "reap_active_fixture-0000000000000001", 86400)
        inactive = self.helper(physical / "reap_active_fixture-0000000000000002", 86300)
        newest = self.helper(physical / "reap_active_fixture-0000000000000003", 86200)
        self.start_child(active)
        result = self.apply()
        self.assertTrue(active.exists(), result.stdout + result.stderr)
        self.assertFalse(inactive.exists(), result.stdout + result.stderr)
        self.assertTrue(newest.exists())
        self.assertIsNone(self.child.poll())
        self.assertEqual(result.returncode, 1)

    def test_unlinked_owned_image_does_not_block_independent_reclamation(self):
        active = self.helper(self.root / "unlinked-image", 86400)
        self.start_child(active)
        active.unlink()
        older = self.helper(self.deps / "reap_active_fixture-0000000000000001", 86400)
        newest = self.helper(self.deps / "reap_active_fixture-0000000000000002", 86300)
        result = self.apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(older.exists())
        self.assertTrue(newest.exists())
        self.assertIsNone(self.child.poll())

    @unittest.skipUnless(FAULT_LIBRARY, "provide the signed native activity fault library")
    def test_unresolved_image_preserves_linked_files_and_directories_not_symlink_leaves(self):
        original = self.helper(self.root / "original-image", 86400)
        active = self.deps / "reap_active_fixture-0000000000000001"
        os.link(original, active)
        active_dir = self.target / "debug/incremental/active-unit"
        active_dir.mkdir(parents=True)
        os.link(original, active_dir / "image")
        inactive_dir = active_dir.parent / "inactive-unit"
        inactive_dir.mkdir()
        (inactive_dir / "link").symlink_to(active)
        for path in (active_dir, inactive_dir):
            old = time.time() - 86400
            os.utime(path, (old, old))
        inactive = self.helper(self.deps / "reap_active_fixture-0000000000000002", 86300)
        newest = self.helper(self.deps / "reap_active_fixture-0000000000000003", 86200)
        self.start_child(original)
        original.unlink()
        result = self.apply(fault="missing-image", activity_pid=self.child.pid)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertTrue(active.exists(), result.stdout + result.stderr)
        self.assertTrue((active_dir / "image").exists())
        self.assertFalse(inactive.exists(), result.stdout + result.stderr)
        self.assertFalse(inactive_dir.exists(), "candidate walks must not follow symlink leaves")
        self.assertIn(b"removed 2/4 planned item(s)", result.stdout)
        self.assertTrue(newest.exists())
        self.assertIsNone(self.child.poll())
        self.stop_child()
        result = self.apply()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(active.exists())
        self.assertFalse(active_dir.exists())
        self.assertTrue(newest.exists())

    def test_recursive_candidate_preserves_its_active_executable_not_an_inactive_sibling(self):
        active_dir = self.target / "debug/incremental/active-unit"
        active = self.helper(active_dir / "helper", 86400)
        inactive = self.target / "debug/incremental/inactive-unit"
        inactive.mkdir()
        (inactive / "payload").write_bytes(b"regenerable fixture")
        for path in [active_dir, inactive / "payload", inactive]:
            old = time.time() - 86400
            os.utime(path, (old, old))
        self.start_child(active)
        result = self.apply()
        self.assertTrue(active.exists(), result.stdout + result.stderr)
        self.assertIsNone(self.child.poll())
        self.assertFalse(inactive.exists())
        self.assertEqual(result.returncode, 1)

    @unittest.skipUnless(FAULT_LIBRARY, "provide the signed native activity fault library")
    def test_unknown_or_incomplete_native_snapshot_refuses_before_any_removal(self):
        older = self.helper(self.deps / "reap_active_fixture-0000000000000001", 86400)
        newest = self.helper(self.deps / "reap_active_fixture-0000000000000002", 86300)
        before = older.read_bytes()
        for mode in ("capacity", "missing-caller", "denied"):
            with self.subTest(mode=mode):
                result = self.apply(fault=mode)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(b"executable activity is unavailable", result.stderr)
                self.assertEqual(older.read_bytes(), before)
                self.assertTrue(newest.exists())


if __name__ == "__main__":
    unittest.main()
