"""Negative controls for installed execution evidence and fixture ownership."""

import copy
import hashlib
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ingress_verifier", ROOT / "tools/verify_ingress_macos.py")
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


def response(code=0, output=""):
    return subprocess.CompletedProcess([], code, output, "")


def execution_fixture():
    key = {"generation": "a" * 32, "epoch": 1, "nonce": "b" * 32}
    result = {"exit_code": 0, "effects": 0, "detail": "complete read-only assessment"}
    view = {"key": key, "created_at": 1, "completed_at": 2, "result": result}
    job = {"document": "Job", "view": view,
           "execution": {"phase": "result", "terminal": result},
           "launch": {"resources": {"slot": "bulk"}, "helpers": [None, None],
                      "worker": {"state": {"phase": "exited"}}}}
    evidence = {"key": key, "prepared_key": key, "metadata_only": False,
                "receipt": {"key": key},
                "read_only_execution": True, "cleanup_granted": False,
                "raw_admission_refused": True, "expired_ticket_exact_replay": True,
                "admission_elapsed_ns": 100,
                "terminal": {"state": "completed", "key": key, "job": view}}
    evidence["job_cli"] = [copy.deepcopy(evidence["terminal"]), copy.deepcopy(evidence["terminal"])]
    evidence["unknown_job_cli"] = {"state": "unknown", "job": None}
    return evidence, job


def frame(job):
    body = json.dumps(job).encode()
    return b"REJ1" + len(body).to_bytes(4, "big") + hashlib.sha256(body).digest() + body


class Ownership(unittest.TestCase):
    def test_ambiguous_bootstrap_is_recorded_and_reconciled_before_runtime_removal(self):
        target = "user/501/dev.micro.reap.owned.fixture"
        arguments = ["/owned/reap", "__service", "control"]
        registered = False
        actions = []
        owned = []
        receipt = {"labels_absent": []}
        base = ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="ownership-regression-", dir=base) as directory:
            output = pathlib.Path(directory)

            def launch(*args):
                nonlocal registered
                actions.append(args)
                if args[0] == "bootstrap":
                    self.assertEqual(json.loads((output / "ownership.json").read_text())[0]["target"], target)
                    registered = True
                    raise subprocess.TimeoutExpired("launchctl bootstrap", 5)
                if args[0] == "bootout":
                    self.assertEqual(args[1], target)
                    registered = False
                    return response()
                if not registered:
                    return response(113)
                return response(output="arguments = {\n" + "\n".join(arguments) + "\n}\n")

            with mock.patch.object(VERIFIER, "launchctl", side_effect=launch):
                with self.assertRaises(subprocess.TimeoutExpired):
                    VERIFIER.bootstrap_owned(owned, receipt, output, "user/501", target, arguments, output / "owned.plist")
                self.assertEqual(owned, [(target, arguments)])
                self.assertTrue(VERIFIER.cleanup_owned(owned, receipt))
            self.assertEqual(receipt["labels_absent"], [target])
            self.assertIn(("bootout", target), actions)

    def test_foreign_or_unobservable_registration_is_not_stopped_or_reported_absent(self):
        owned = [("user/501/owned.fixture", ["/owned/reap", "__service", "control"])]
        for reply in [response(output="arguments = {\n/foreign/program\n}\n"),
                      subprocess.TimeoutExpired("launchctl print", 5)]:
            receipt = {"labels_absent": []}
            with mock.patch.object(VERIFIER, "launchctl", side_effect=[reply]) as launch:
                self.assertFalse(VERIFIER.cleanup_owned(owned, receipt))
            self.assertEqual(launch.call_args_list, [mock.call("print", owned[0][0])])
            self.assertEqual(receipt["labels_absent"], [])
            self.assertEqual(receipt["unresolved_ownership"][0]["target"], owned[0][0])


class Evidence(unittest.TestCase):
    def test_terminal_and_preparation_must_match_the_original_key(self):
        evidence, job = execution_fixture()
        self.assertEqual(VERIFIER.validate_execution(evidence, frame(job), b"installed protected survivor")["state"], "completed")
        for field in ["key", "prepared_key"]:
            wrong = copy.deepcopy(evidence)
            wrong[field] = {**wrong[field], "nonce": "c" * 32}
            with self.assertRaises(AssertionError):
                VERIFIER.validate_execution(wrong, frame(job), b"installed protected survivor")
        wrong = copy.deepcopy(evidence)
        wrong["receipt"]["key"] = {**wrong["receipt"]["key"], "nonce": "c" * 32}
        with self.assertRaises(AssertionError):
            VERIFIER.validate_execution(wrong, frame(job), b"installed protected survivor")

    def test_unreleased_or_changed_result_cannot_pass_durable_execution_proof(self):
        evidence, job = execution_fixture()
        for changed in range(4):
            invalid = copy.deepcopy(job)
            if changed == 0:
                invalid["launch"]["worker"]["state"]["phase"] = "granted"
            elif changed == 1:
                invalid["launch"]["helpers"][0] = {"state": {"phase": "intent"}}
            elif changed == 2:
                invalid["execution"]["terminal"]["detail"] = "different result"
            else:
                invalid["view"]["completed_at"] = 0
            with self.assertRaises(AssertionError):
                VERIFIER.validate_execution(evidence, frame(invalid), b"installed protected survivor")


if __name__ == "__main__":
    unittest.main()
