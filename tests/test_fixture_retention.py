"""Keep synthetic fixture data after a failed job and a later successful job."""

import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]


def load(name):
    path = ROOT / "tests" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def receipt(state):
    detail = {"stdout": "", "stderr": "", "worker_observation": {"unknown_effects": state == "failed"}}
    body = {"protocol": 2, "state": state, "job": {"result": {"detail": json.dumps(detail)}}}
    return subprocess.CompletedProcess([], 0 if state == "completed" else 1, json.dumps(body).encode(), b"")


@unittest.skipUnless(Path("/System/Volumes/Data").is_dir(), "requires the macOS internal fixture volume")
class Retention(unittest.TestCase):
    def test_successful_fixture_jobs_allow_owned_cleanup(self):
        module = load("test_preservation")
        with tempfile.TemporaryDirectory(prefix="reap-fixture-retention-", dir="/private/tmp") as base:
            fixture = module.Fixture(Path(base))
            with patch.object(module.subprocess, "run", return_value=receipt("completed")):
                fixture.run("files", "--apply")
            fixture.close()
            self.assertFalse(fixture.root.exists())
        module = load("test_public_mutations")
        fixture = module.PublicMutations()
        fixture.setUp()
        with patch.object(module.subprocess, "run", return_value=receipt("completed")):
            fixture.run_cli("files", "--apply")
        self.assertTrue(fixture.doCleanups())
        self.assertFalse(fixture.root.exists())

    def test_preservation_fixture_keeps_failed_job_data_after_a_later_success(self):
        module = load("test_preservation")
        with tempfile.TemporaryDirectory(prefix="reap-fixture-retention-", dir="/private/tmp") as base:
            fixture = module.Fixture(Path(base))
            payload = fixture.payload(fixture.projects / "owned")
            with patch.object(module.subprocess, "run", side_effect=[receipt("failed"), receipt("completed")]):
                fixture.run("files", "--apply")
                fixture.run("files", "--apply")
            fixture.close()
            self.assertEqual(payload.read_bytes(), module.PAYLOAD)
            self.assertIn("keep", json.loads((fixture.root / ".reap").read_text()))

    def test_public_mutation_fixture_keeps_failed_job_data_after_a_later_success(self):
        module = load("test_public_mutations")
        fixture = module.PublicMutations()
        fixture.setUp()
        try:
            with patch.object(module.subprocess, "run", side_effect=[receipt("failed"), receipt("completed")]):
                fixture.run_cli("files", "--apply")
                fixture.run_cli("files", "--apply")
            self.assertTrue(fixture.doCleanups())
            self.assertEqual((fixture.kept / "valuable").read_bytes(), module.PAYLOAD)
            self.assertIn("keep", json.loads((fixture.root / ".reap").read_text()))
        finally:
            # the subprocess is mocked; no native job or reservation exists.
            fixture.doCleanups()
            if fixture.root.exists():
                shutil.rmtree(fixture.root)


if __name__ == "__main__":
    unittest.main()
