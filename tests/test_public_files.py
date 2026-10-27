"""Exercise ordinary Files submission through the installed host-user broker."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REAP_TEST_BIN") or shutil.which("reap")
PAYLOAD = b"retained public Files input\n"


@unittest.skipUnless(BINARY, "install reap or set REAP_TEST_BIN")
class PublicFiles(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run([BINARY, "__service-probe", "control"], capture_output=True, timeout=3)
        if result.returncode:
            raise RuntimeError("the installed on-demand broker is required for public CLI proof")

    def setUp(self):
        base = ROOT / "target/control-t13"
        temporary = tempfile.TemporaryDirectory(prefix="public-files-", dir=base)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.assertEqual(self.root.stat().st_dev, Path("/System/Volumes/Data").stat().st_dev)
        self.home = self.root / "home"
        self.config = self.home / ".config/reap/config.json"
        self.config.parent.mkdir(parents=True)
        self.governed = self.root / "governed"
        self.workspace = self.governed / "project/workspace"
        self.workspace.mkdir(parents=True)
        (self.workspace / "valuable").write_bytes(PAYLOAD)
        (self.workspace / ".reap").write_text(json.dumps({
            "version": 1, "keep": {"reason": "public Files survivor"}}))
        self.config.write_text(json.dumps({"roots": [], "governed_roots": [str(self.governed)]}))
        self.environment = dict(os.environ, HOME=str(self.home),
                                XDG_STATE_HOME=str(self.root / "state"), PWD="/not-native-cwd")
        for name in ("REAP_INTERNAL_CONFIG_SNAPSHOT", "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
            self.environment.pop(name, None)

    def run_cli(self, *arguments, environment=None, timeout=32):
        started = time.monotonic()
        result = subprocess.run([BINARY, *map(str, arguments)], cwd=self.root,
                                env=environment or self.environment, capture_output=True, timeout=timeout)
        result.elapsed = time.monotonic() - started
        result.receipt = json.loads(result.stdout)
        self.assertEqual(result.receipt["protocol"], 2)
        self.assertEqual((self.workspace / "valuable").read_bytes(), PAYLOAD)
        return result

    def completed(self, *arguments):
        result = self.run_cli("files", *arguments, "--wait", "30")
        self.assertEqual(result.returncode, 0, result)
        self.assertEqual(result.receipt["state"], "completed", result)
        self.assertEqual(result.receipt["invocation"]["operation"], "files")
        self.assertEqual(result.receipt["invocation"]["mode"], "read")
        self.assertEqual(result.receipt["job"]["invocation"], result.receipt["invocation"])
        self.assertEqual(result.receipt["job"]["result"]["effects"], 0)
        detail = json.loads(result.receipt["job"]["result"]["detail"])
        self.assertEqual(detail["command"], "files")
        self.assertEqual(detail["reports"][0]["declarations"], 1)
        return result

    def test_scoped_completion_and_lookup_keep_native_cwd_and_source(self):
        result = self.completed("governed/project/workspace")
        self.assertEqual(result.receipt["invocation"]["requested_scope"], "governed/project/workspace")
        self.config.unlink()
        os.mkfifo(self.config)
        shown = self.run_cli("job", "show", result.receipt["job_id"], timeout=2)
        self.assertEqual(shown.returncode, 0)
        self.assertEqual(shown.receipt["job"], result.receipt["job"])
        self.assertLess(shown.elapsed, 1.25)

    def test_no_path_assesses_configured_roots_not_service_cwd(self):
        result = self.completed()
        self.assertEqual(result.receipt["invocation"]["requested_scope"], "configured")

    def test_special_configuration_cannot_block_the_public_response(self):
        self.config.unlink()
        os.mkfifo(self.config)
        result = self.run_cli("files", self.workspace, timeout=2)
        self.assertLess(result.elapsed, 1.25)
        self.assertNotEqual(result.returncode, 0, result)
        self.assertNotEqual(result.receipt["state"], "completed")

    def test_outside_governed_scope_cannot_complete_successfully(self):
        outside = self.root / "outside"
        outside.mkdir()
        result = self.run_cli("files", outside, "--wait", "30")
        self.assertEqual(result.returncode, 1, result)
        self.assertEqual(result.receipt["state"], "failed")
        self.assertEqual(result.receipt["job"]["result"]["effects"], 0)

    def test_unreadable_descendant_is_incomplete_without_source_effects(self):
        child = self.workspace / "unreadable"
        child.mkdir()
        (child / "valuable").write_bytes(PAYLOAD)
        child.chmod(0)
        try:
            try:
                list(child.iterdir())
            except PermissionError:
                pass
            else:
                self.skipTest("the unreadable fixture requires an unprivileged account")
            result = self.run_cli("files", self.workspace, "--wait", "30")
            self.assertEqual(result.returncode, 1, result)
            self.assertEqual(result.receipt["state"], "failed")
            self.assertEqual(result.receipt["job"]["result"]["effects"], 0)
        finally:
            child.chmod(0o700)
        self.assertEqual((child / "valuable").read_bytes(), PAYLOAD)

    def test_invalid_namespace_and_snapshot_never_submit(self):
        for updates in ({"HOME": "relative-home"}, {"REAP_INTERNAL_CONFIG_SNAPSHOT": "{}"}):
            with self.subTest(updates=updates):
                result = self.run_cli("files", self.workspace,
                                      environment=dict(self.environment, **updates), timeout=2)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.receipt["state"], "invalid")
                self.assertIsNone(result.receipt["job"])
                self.assertLess(result.elapsed, 1.25)

    def test_wait_rejects_invalid_deadlines_and_applying_mode(self):
        for arguments in (("--wait", "0"), ("--wait", "nan"), ("--apply", "--wait", "1")):
            with self.subTest(arguments=arguments):
                result = subprocess.run([BINARY, "files", *arguments], cwd=self.root,
                                        env=self.environment, capture_output=True, timeout=2)
                self.assertEqual(result.returncode, 2)
                self.assertEqual((self.workspace / "valuable").read_bytes(), PAYLOAD)


if __name__ == "__main__":
    unittest.main()
