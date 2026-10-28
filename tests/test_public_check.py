"""Exercise the ordinary bounded check command through the installed host-user broker."""

import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import time
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
BINARY = os.environ.get("REAP_TEST_BIN") or shutil.which("reap")
PAYLOAD = b"synthetic retained check input\n"


@unittest.skipUnless(BINARY, "install reap or set REAP_TEST_BIN")
class PublicCheck(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run([BINARY, "__service-probe", "control"], capture_output=True, timeout=3)
        if result.returncode:
            raise RuntimeError("the installed on-demand broker is required for public CLI proof")

    def setUp(self):
        base = ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        self.root = pathlib.Path(tempfile.mkdtemp(prefix="public-check-", dir=base)).resolve()
        self.unconfirmed_jobs = 0
        self.addCleanup(self.close_fixture)
        self.user_home = self.root / "home"
        (self.user_home / ".config/reap").mkdir(parents=True)
        self.state = self.root / "state"
        (self.state / "reap").mkdir(parents=True)
        (self.user_home / ".config/reap/config.json").write_text(json.dumps({
            "roots": [str(self.root / "project")], "governed_roots": []}))
        self.project = self.root / "project"
        self.project.mkdir()
        (self.project / "Cargo.toml").write_text("[workspace]\n")
        (self.project / "valuable").write_bytes(PAYLOAD)
        self.environment = dict(os.environ, HOME=str(self.user_home), XDG_STATE_HOME=str(self.state),
                                PWD="/not-the-native-directory")
        for name in ("REAP_INTERNAL_CONFIG_SNAPSHOT", "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
            self.environment.pop(name, None)

    def close_fixture(self):
        if self.unconfirmed_jobs:
            (self.root / ".reap").write_text(json.dumps({
                "version": 1, "keep": {"reason": "unconfirmed public check fixture"},
            }))
            print(f"Retained unconfirmed public check fixture: {self.root}")
        else:
            shutil.rmtree(self.root)

    def run_cli(self, *arguments, environment=None, timeout=32):
        coordinated = arguments and arguments[0] in ("check", "stores")
        if coordinated:
            self.unconfirmed_jobs += 1
        started = time.monotonic()
        result = subprocess.run([BINARY, *map(str, arguments)], cwd=self.root,
                                env=environment or self.environment, capture_output=True, timeout=timeout)
        result.elapsed = time.monotonic() - started
        result.receipt = json.loads(result.stdout)
        self.assertEqual(result.receipt["protocol"], 2, result)
        if coordinated and result.receipt["state"] in ("completed", "not_accepted", "invalid", "unavailable"):
            self.unconfirmed_jobs -= 1
        return result

    def completed(self, *arguments):
        result = self.run_cli("check", *arguments, "--wait", "30")
        self.assertEqual(result.returncode, 0, result)
        receipt = result.receipt
        self.assertEqual(receipt["state"], "completed", result)
        self.assertEqual(receipt["job"]["result"]["effects"], 0)
        self.assertEqual(receipt["invocation"]["operation"], "check")
        self.assertEqual(receipt["invocation"]["mode"], "read")
        self.assertEqual(receipt["job"]["invocation"], receipt["invocation"])
        self.assertEqual((self.project / "valuable").read_bytes(), PAYLOAD)
        return result

    def test_actual_cwd_and_requested_scope_survive_public_lookup(self):
        result = self.completed("project")
        self.assertEqual(result.receipt["invocation"]["requested_scope"], "project")
        detail = json.loads(result.receipt["job"]["result"]["detail"])
        self.assertEqual(pathlib.Path(detail["project"]), self.project)
        source = self.project / ".reap.json"
        os.mkfifo(source)
        shown = self.run_cli("job", "show", result.receipt["job_id"], timeout=2)
        self.assertEqual(shown.returncode, 0, shown)
        self.assertEqual(shown.receipt["job"], result.receipt["job"])
        self.assertLess(shown.elapsed, 1.25)

    def test_cancel_preserves_a_completed_result_without_reading_caller_config(self):
        completed = self.completed(self.project)
        config = self.user_home / ".config/reap/config.json"
        config.unlink()
        os.mkfifo(config)
        cancelled = self.run_cli("job", "cancel", completed.receipt["job_id"], timeout=2)
        self.assertEqual(cancelled.returncode, 0, cancelled)
        self.assertEqual(cancelled.receipt["job"], completed.receipt["job"])
        self.assertLess(cancelled.elapsed, 1.25)
        self.assertEqual((self.project / "valuable").read_bytes(), PAYLOAD)

    def test_cancel_missing_key_does_not_admit_work(self):
        import uuid

        completed = self.completed(self.project)
        key = completed.receipt["key"]
        identifier = f"{key['generation']}:{key['epoch']}:{uuid.uuid4().hex}"
        cancelled = self.run_cli("job", "cancel", identifier, timeout=2)
        self.assertIn(cancelled.receipt["state"], ("not_accepted", "unknown"))
        self.assertNotEqual(cancelled.returncode, 0)
        self.assertIsNone(cancelled.receipt["job"])
        observed = self.run_cli("job", "show", identifier, timeout=2)
        self.assertIn(observed.receipt["state"], ("not_accepted", "unknown"))
        self.assertIsNone(observed.receipt["job"])

    def test_cancel_invalid_key_is_invalid_before_context_preparation(self):
        config = self.user_home / ".config/reap/config.json"
        config.unlink()
        os.mkfifo(config)
        cancelled = self.run_cli("job", "cancel", "../../not-a-job", timeout=2)
        self.assertEqual(cancelled.returncode, 2)
        self.assertEqual(cancelled.receipt["state"], "invalid")
        self.assertIsNone(cancelled.receipt["key"])
        self.assertIsNone(cancelled.receipt["next_command"])

    def test_outside_project_target_is_a_complete_read_only_assessment(self):
        target = self.root / "shared-target"
        target.mkdir()
        (self.project / ".reap.json").write_text(json.dumps({"target": "../shared-target"}))
        result = self.completed(self.project)
        detail = json.loads(result.receipt["job"]["result"]["detail"])
        self.assertEqual(pathlib.Path(detail["target"]).resolve(strict=True), target)
        self.assertTrue(target.is_dir())

    def test_bound_external_store_uses_the_caller_namespace(self):
        store = self.root / "external-store"
        store.mkdir()
        (store / "valuable").write_bytes(PAYLOAD)
        (self.project / ".reap.json").write_text(json.dumps({
            "version": 2, "stores": [{"resource": "logs", "retention": {"max_age_days": 1}}]}))
        bound = self.run_cli("stores", "--bind", "logs", "--to", store, self.project, "--wait", "30")
        self.assertEqual(bound.returncode, 0, bound)
        self.assertEqual(bound.receipt["state"], "completed", bound)
        result = self.completed(self.project)
        detail = json.loads(result.receipt["job"]["result"]["detail"])
        self.assertEqual(detail["stores"][0]["resource"], "logs")
        self.assertEqual(pathlib.Path(detail["stores"][0]["directory"]), store)
        self.assertEqual((store / "valuable").read_bytes(), PAYLOAD)

    def test_special_manifest_returns_promptly_without_completed_success(self):
        os.mkfifo(self.project / ".reap.json")
        result = self.run_cli("check", self.project, timeout=2)
        self.assertLess(result.elapsed, 1.25)
        self.assertNotEqual(result.returncode, 0, result)
        self.assertNotEqual(result.receipt["state"], "completed", result)
        self.assertEqual((self.project / "valuable").read_bytes(), PAYLOAD)

    def test_invalid_public_namespace_and_snapshot_fail_before_submission(self):
        for updates in ({"HOME": "relative-home"}, {"REAP_INTERNAL_CONFIG_SNAPSHOT": "{}"}):
            with self.subTest(updates=updates):
                result = self.run_cli("check", self.project,
                                      environment=dict(self.environment, **updates), timeout=2)
                self.assertEqual(result.returncode, 2, result)
                self.assertEqual(result.receipt["state"], "invalid")
                self.assertIsNone(result.receipt["job"])
                self.assertLess(result.elapsed, 1.25)
                self.assertEqual((self.project / "valuable").read_bytes(), PAYLOAD)


if __name__ == "__main__":
    unittest.main()
