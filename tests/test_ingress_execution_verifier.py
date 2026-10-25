"""Negative controls for installed execution evidence and fixture ownership."""

import copy
import contextlib
import hashlib
import importlib.util
import io
import json
import os
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
    result = {"exit_code": 0, "effects": 0, "detail": json.dumps({"command": "check"})}
    payload = {"request": json.dumps({"argv": ["check", "/owned/project"]})}
    view = {"key": key, "created_at": 1, "completed_at": 2, "result": result,
            "invocation": {"operation": "check", "mode": "read"}}
    job = {"document": "Job", "view": view, "input": payload,
           "execution": {"phase": "result", "terminal": result},
           "launch": {"resources": {"slot": "bulk"}, "helpers": [None, None],
                      "worker": {"state": {"phase": "exited"}}}}
    evidence = {"key": key, "prepared_key": key, "metadata_only": False,
                "receipt": {"key": key, "input": payload},
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
    def test_cancellation_requires_a_request_durable_failure_and_proven_child_exit(self):
        evidence, job = execution_fixture()
        result = job["view"]["result"]
        result.update(exit_code=1, detail=json.dumps({"outcome": "cancelled"}))
        job["view"]["cancel_requested_at"] = 1
        evidence["terminal"]["state"] = "failed"
        acknowledged = copy.deepcopy(job["view"])
        acknowledged.update(result=None, completed_at=None)
        evidence["cancellation"] = {"protocol": 2, "key": copy.deepcopy(evidence["key"]),
                                    "state": "pending", "job": acknowledged}
        evidence["job_cli"] = [copy.deepcopy(evidence["terminal"]), copy.deepcopy(evidence["terminal"])]
        verify = lambda e, j: VERIFIER.validate_execution(e, frame(j), b"installed protected survivor", "check", True)
        verify(evidence, job)
        no_launch = copy.deepcopy(job)
        del no_launch["launch"]
        verify(evidence, no_launch)
        lost = copy.deepcopy(evidence)
        lost["cancellation"].update(state="acceptance_unknown", job=None)
        verify(lost, job)
        for change in ("missing_request", "boolean_time", "wrong_key", "missing_ack", "ack_time",
                       "success", "result", "worker", "helper", "boolean_exit", "boolean_effects", "pending_result"):
            bad_evidence, bad_job = copy.deepcopy((evidence, job))
            if change == "missing_request":
                del bad_job["view"]["cancel_requested_at"]
            elif change == "boolean_time":
                bad_job["view"]["cancel_requested_at"] = True
            elif change == "wrong_key":
                bad_evidence["cancellation"]["key"]["nonce"] = "c" * 32
            elif change == "missing_ack":
                bad_evidence["cancellation"] = None
            elif change == "ack_time":
                bad_evidence["cancellation"]["job"]["cancel_requested_at"] = 0
            elif change == "success":
                bad_evidence["terminal"]["state"] = "completed"
            elif change == "result":
                bad_job["view"]["result"]["detail"] = json.dumps({"outcome": "complete"})
            elif change == "worker":
                bad_job["launch"]["worker"]["state"]["phase"] = "granted"
            elif change == "helper":
                bad_job["launch"]["helpers"][0] = {"state": {"phase": "intent"}}
            elif change == "boolean_exit":
                bad_job["view"]["result"]["exit_code"] = True
            elif change == "boolean_effects":
                bad_job["view"]["result"]["effects"] = False
            else:
                bad_evidence["cancellation"]["job"]["result"] = bad_job["view"]["result"]
            with self.subTest(change=change), self.assertRaises(AssertionError):
                verify(bad_evidence, bad_job)

    def test_files_proof_requires_the_exact_operation_and_complete_read_only_report(self):
        evidence, job = execution_fixture()
        payload = {"request": json.dumps({"argv": ["files", "/owned/project"]})}
        job["input"] = payload
        evidence["receipt"]["input"] = payload
        job["view"]["invocation"] = {"operation": "files", "mode": "read"}
        detail = {"command": "files", "requested_roots": 1, "present_roots": 1,
                  "reports": [{"exit_code": 0, "coverage_complete": True,
                               "roots": 1, "declarations": 0, "honored": 0, "invalid": 0,
                               "undeclared": 2, "planned_units": 0, "blocked_units": 0,
                               "planned_bytes": 0, "completed_bytes": 0,
                               "completed_units": 0, "unknown_residual_units": 0}]}
        job["view"]["result"]["detail"] = json.dumps(detail)
        evidence["terminal"]["job"] = job["view"]
        evidence["job_cli"] = [copy.deepcopy(evidence["terminal"]), copy.deepcopy(evidence["terminal"])]
        VERIFIER.validate_execution(evidence, frame(job), b"installed protected survivor", "files")
        for change in ("operation", "coverage", "effects", "roots", "bytes", "discovery", "missing", "boolean"):
            bad_evidence, bad_job = copy.deepcopy((evidence, job))
            if change == "operation":
                bad_job["input"]["request"] = json.dumps({"argv": ["check", "/owned/project"]})
            else:
                bad_detail = json.loads(bad_job["view"]["result"]["detail"])
                report = bad_detail["reports"][0]
                if change == "missing":
                    del report["planned_bytes"]
                else:
                    name, value = {"coverage": ("coverage_complete", False),
                                   "effects": ("unknown_residual_units", 1), "roots": ("roots", 0),
                                   "bytes": ("completed_bytes", 99), "discovery": ("undeclared", 0),
                                   "boolean": ("roots", True)}[change]
                    report[name] = value
                bad_job["view"]["result"]["detail"] = json.dumps(bad_detail)
                bad_evidence["job_cli"] = [copy.deepcopy(bad_evidence["terminal"]), copy.deepcopy(bad_evidence["terminal"])]
            with self.subTest(change=change), self.assertRaises(AssertionError):
                VERIFIER.validate_execution(bad_evidence, frame(bad_job), b"installed protected survivor", "files")

    def test_scoped_files_requires_ancestor_discovery_and_exact_scope_binding(self):
        evidence, job = execution_fixture()
        payload = {"request": json.dumps({"argv": ["files", "/owned/project/workspace"]})}
        job["input"] = payload
        evidence["receipt"]["input"] = payload
        job["view"]["invocation"] = {"operation": "files", "mode": "read"}
        detail = {"command": "files", "requested_roots": 1, "present_roots": 1,
                  "selections": [{"path": "/owned/project/workspace", "governed_root": "/owned/project"}],
                  "reports": [{"exit_code": 0, "coverage_complete": True,
                               "roots": 1, "declarations": 2, "honored": 2, "invalid": 0,
                               "undeclared": 0, "planned_units": 0, "blocked_units": 0,
                               "planned_bytes": 0, "completed_bytes": 0,
                               "completed_units": 0, "unknown_residual_units": 0}]}
        job["view"]["result"]["detail"] = json.dumps(detail)
        evidence["terminal"]["job"] = job["view"]
        evidence["job_cli"] = [copy.deepcopy(evidence["terminal"]), copy.deepcopy(evidence["terminal"])]
        verify = lambda e, j: VERIFIER.validate_execution(e, frame(j), b"installed protected survivor", "files", False, True)
        verify(evidence, job)
        for change in ("empty", "coverage", "authority", "scope"):
            bad_evidence, bad_job = copy.deepcopy((evidence, job))
            changed = json.loads(bad_job["view"]["result"]["detail"])
            if change == "empty":
                changed["reports"][0].update(declarations=0, honored=0)
            elif change == "coverage":
                changed["reports"][0]["coverage_complete"] = False
            elif change == "authority":
                changed["selections"][0]["governed_root"] = "/owned/project/workspace"
            else:
                changed["selections"][0]["path"] = "/foreign/workspace"
            bad_job["view"]["result"]["detail"] = json.dumps(changed)
            bad_evidence["job_cli"] = [copy.deepcopy(bad_evidence["terminal"]), copy.deepcopy(bad_evidence["terminal"])]
            with self.subTest(change=change), self.assertRaises(AssertionError):
                verify(bad_evidence, bad_job)

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


class JournalSelection(unittest.TestCase):
    def setUp(self):
        base = ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="managed-ingress-keys-", dir=base)
        self.addCleanup(self.temporary.cleanup)
        self.journal = pathlib.Path(self.temporary.name)
        self.evidence, self.job = execution_fixture()
        self.name = "j0000000000000001-" + "b" * 32 + ".json"
        (self.journal / self.name).write_bytes(frame(self.job))

    def test_managed_record_uses_only_the_exact_key_without_enumeration(self):
        unrelated = self.journal / ("j0000000000000002-" + "c" * 32 + ".json")
        unrelated.write_bytes(b"protected unrelated production record")
        native_open = os.open
        opened = []

        def exact_open(path, *args, **kwargs):
            opened.append(path)
            return native_open(path, *args, **kwargs)

        with mock.patch.object(pathlib.Path, "glob", side_effect=AssertionError("production enumeration")), \
                mock.patch.object(VERIFIER.os, "open", side_effect=exact_open):
            data, name = VERIFIER.execution_record(self.journal, self.evidence, managed=True)
        self.assertEqual(name, self.name)
        self.assertEqual(data, frame(self.job))
        self.assertEqual(opened, [self.journal, self.name])
        self.assertEqual(unrelated.read_bytes(), b"protected unrelated production record")
        VERIFIER.validate_execution(self.evidence, data, b"installed protected survivor")

    def test_isolated_one_key_rule_is_not_used_for_managed_mode(self):
        self.assertEqual(VERIFIER.execution_record(self.journal, self.evidence)[1], self.name)
        (self.journal / "j-other.json").write_bytes(b"not the fixture record")
        with self.assertRaises(AssertionError):
            VERIFIER.execution_record(self.journal, self.evidence)
        self.assertEqual(VERIFIER.execution_record(self.journal, self.evidence, managed=True)[1], self.name)

    def test_filename_matches_native_hex_epoch_and_rejects_invalid_keys(self):
        key = {**self.evidence["key"], "epoch": 0x123abcdef}
        self.assertEqual(VERIFIER.job_filename(key), "j0000000123abcdef-" + "b" * 32 + ".json")
        self.assertEqual(VERIFIER.job_filename({**key, "epoch": 2**64 - 1}), "jffffffffffffffff-" + "b" * 32 + ".json")
        for changed in [{**key, "epoch": 0}, {**key, "epoch": True}, {**key, "epoch": 2**64},
                        {**key, "nonce": "../foreign"}, {**key, "nonce": "B" * 32},
                        {**key, "generation": "a"}, {**key, "extra": "unknown"}]:
            with self.subTest(key=changed), self.assertRaises(AssertionError):
                VERIFIER.job_filename(changed)

    def test_exact_record_refuses_symlinks_oversize_and_wrong_generation(self):
        path = self.journal / self.name
        path.unlink()
        other = self.journal / "unrelated"
        other.write_bytes(frame(self.job))
        path.symlink_to(other)
        with self.assertRaises(OSError):
            VERIFIER.execution_record(self.journal, self.evidence, managed=True)
        path.unlink()
        path.write_bytes(b"x" * (128 * 1024 + 1))
        with self.assertRaises(AssertionError):
            VERIFIER.execution_record(self.journal, self.evidence, managed=True)
        wrong = copy.deepcopy(self.job)
        wrong["view"]["key"]["generation"] = "d" * 32
        path.write_bytes(frame(wrong))
        data, _ = VERIFIER.execution_record(self.journal, self.evidence, managed=True)
        with self.assertRaises(AssertionError):
            VERIFIER.validate_execution(self.evidence, data, b"installed protected survivor")


class ManagedInstaller:
    """Expose only the installer's read-only contract to managed-mode unit cases."""

    roles = ("control", "context-resolver", "state-writer")

    def __init__(self, home, binary):
        self.home = home
        self.binary = binary
        self.sha256 = VERIFIER.sha256(binary)
        self.domain = f"user/{os.getuid()}"
        self.profile = {"schema": 1, "protocol": 2, "uid": os.getuid(),
                        "agent_hosts": False, "abandon_process_group": True,
                        "role_profile": "fixed-agentless-v1", "roles": [
                            {"argument": role, "label": "dev.micro.reap." + role,
                             "endpoint": "fixture." + role} for role in self.roles]}
        self.state = {"roles": {role: {"phase": "installed", "intent": role + "-owned-intent",
                                      "binary_sha256": self.sha256, "profile": copy.deepcopy(self.profile)}
                                for role in self.roles}}
        self.observed = {role: {"intent": role + "-owned-intent", "identity": {"pid": 100 + index}}
                         for index, role in enumerate(self.roles)}
        self.preflight = mock.Mock(side_effect=lambda: {"state": copy.deepcopy(self.state)})
        self.inspect = mock.Mock(side_effect=lambda state: copy.deepcopy(self.observed))
        self.probe = mock.Mock()
        self.json_command = mock.Mock(side_effect=lambda *args: copy.deepcopy(self.profile))
        self.apply = mock.Mock(side_effect=AssertionError("managed verification cannot install"))
        self.save = mock.Mock(side_effect=AssertionError("managed verification cannot write installer metadata"))
        self.retire = mock.Mock(side_effect=AssertionError("managed verification cannot stop a role"))

    def role(self, argument):
        return next(role for role in self.profile["roles"] if role["argument"] == argument)


class ManagedMode(unittest.TestCase):
    def setUp(self):
        base = ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="managed-ingress-mode-", dir=base)
        self.addCleanup(self.temporary.cleanup)
        self.base = pathlib.Path(self.temporary.name)
        self.home = self.base / "ordinary-home"
        self.binary = self.home / ".cargo/bin/reap"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(b"installed fixture binary")
        self.unit = self.base / "unit"
        self.unit.write_bytes(b"fixture native client")
        self.cwd = self.base / "native-cwd"
        self.cwd.mkdir()
        self.runtime = self.base / "runtime"
        self.output = self.base / "output"
        self.installer = ManagedInstaller(self.home, self.binary)
        self.helper = mock.Mock(ROLES=self.installer.roles)
        self.helper.Installer.return_value = self.installer
        self.evidence, self.job = execution_fixture()
        self.client_exit = 0
        self.after_client = lambda: None
        self.journal = self.home / ".local/state/reap/admission"
        self.journal.mkdir(parents=True)
        self.unrelated = self.journal / "j-production-unrelated.json"
        self.unrelated.write_bytes(b"protected production history")

    def run_main(self, managed=True):
        arguments = ["verify_ingress_macos.py", str(self.unit), "--cwd", str(self.cwd), "--output", str(self.output)]
        if managed:
            arguments.append("--managed-services")

        def runtime(*args, **kwargs):
            self.runtime.mkdir()
            return str(self.runtime)

        def client(*args, **kwargs):
            environment = kwargs["env"]
            self.assertEqual(environment["REAP_INSTALLED_INGRESS_EXPECT_CWD"], str(self.cwd))
            self.assertEqual(environment["REAP_INSTALLED_INGRESS_EXPECT_FILE_ID"], f"{self.cwd.stat().st_ino:016x}")
            self.assertEqual(environment["PWD"], "/not-the-native-caller-directory")
            config = (self.runtime / "caller-home/.config/reap/config.json").read_bytes()
            self.assertEqual(environment["REAP_INSTALLED_INGRESS_EXPECT_CONFIG_SHA256"], hashlib.sha256(config).hexdigest())
            if managed:
                self.assertEqual(environment["HOME"], str(self.home))
                self.assertEqual(environment["XDG_STATE_HOME"], str(self.home / ".local/state"))
            (self.runtime / "client-receipt.json").write_text(json.dumps(self.evidence))
            (self.runtime / "check-project").mkdir()
            (self.runtime / "check-project/valuable").write_bytes(b"installed protected survivor")
            journal = self.journal if managed else self.runtime / "writer-state/reap/admission"
            journal.mkdir(parents=True, exist_ok=True)
            (journal / VERIFIER.job_filename(self.evidence["key"])).write_bytes(frame(self.job))
            self.after_client()
            return response(self.client_exit)

        patches = [mock.patch.object(VERIFIER.sys, "argv", arguments),
                   mock.patch.object(VERIFIER.platform, "system", return_value="Darwin"),
                   mock.patch.object(VERIFIER.pwd, "getpwuid", return_value=mock.Mock(pw_dir=str(self.home))),
                   mock.patch.object(VERIFIER.tempfile, "mkdtemp", side_effect=runtime),
                   mock.patch.object(VERIFIER, "service_installer", return_value=self.helper),
                   mock.patch.object(VERIFIER, "command", return_value=response(output="fixture designated requirement")),
                   mock.patch.object(VERIFIER.subprocess, "run", side_effect=client),
                   mock.patch.object(VERIFIER.sys, "stdout", io.StringIO()),
                   mock.patch.dict(os.environ, {"HOME": "/foreign/caller/home", "XDG_STATE_HOME": "/foreign/caller/state"})]
        if managed:
            patches += [mock.patch.object(VERIFIER, "launchctl", side_effect=AssertionError("managed launchctl mutation or domain scan")),
                        mock.patch.object(VERIFIER, "bootstrap_owned", side_effect=AssertionError("managed bootstrap")),
                        mock.patch.object(VERIFIER, "cleanup_owned", side_effect=AssertionError("managed cleanup")),
                        mock.patch.object(pathlib.Path, "home", side_effect=AssertionError("caller HOME chose the broker"))]
        else:
            patches += [mock.patch.object(pathlib.Path, "home", return_value=self.home),
                        mock.patch.object(VERIFIER, "launchctl", side_effect=lambda *args: response(0 if args[1] == f"user/{os.getuid()}" else 113)),
                        mock.patch.object(VERIFIER, "bootstrap_owned"), mock.patch.object(VERIFIER, "cleanup_owned", return_value=True)]
        with contextlib.ExitStack() as stack:
            for patch in patches:
                stack.enter_context(patch)
            result = VERIFIER.main()
        self.installer.apply.assert_not_called()
        self.installer.save.assert_not_called()
        self.installer.retire.assert_not_called()
        return result, json.loads((self.output / "receipt.json").read_text())

    def test_managed_mode_preserves_services_and_reads_only_its_production_key(self):
        result, receipt = self.run_main()
        self.assertEqual(result, 0, receipt)
        self.assertEqual(receipt["service_mode"], "managed")
        self.assertTrue(receipt["managed_services_preserved"])
        self.assertFalse(receipt["production_services_stopped"])
        self.assertEqual(receipt["service_cleanup"], "none_managed")
        self.assertNotIn("durable_jobs", receipt)
        self.assertNotIn("labels_absent", receipt)
        self.assertEqual(receipt["verified_job_records"], 1)
        self.assertEqual(receipt["journal_scope"], "exact_fixture_key")
        self.assertEqual(self.unrelated.read_bytes(), b"protected production history")
        self.assertFalse((self.output / self.unrelated.name).exists())
        self.assertEqual((self.output / "verified-job.json").read_bytes(), frame(self.job))
        self.assertEqual(receipt["journal_record_sha256"], hashlib.sha256(frame(self.job)).hexdigest())
        self.assertEqual(self.installer.probe.call_args_list, [mock.call(role) for role in self.installer.roles])
        self.assertTrue(all(call == mock.call(self.home, os.getuid(), self.binary, self.binary)
                            for call in self.helper.Installer.call_args_list))
        self.assertTrue(receipt["runtime_removed"])

    def test_failed_managed_client_retains_evidence_without_sampling_or_cleanup(self):
        self.client_exit = 1
        result, receipt = self.run_main()
        self.assertEqual(result, 1)
        self.assertTrue(receipt["managed_services_preserved"])
        self.assertFalse(receipt["runtime_removed"])
        self.assertNotIn("resolver_sample_exit", receipt)
        self.assertTrue((self.output / "client.log").exists())
        self.assertNotIn("verified_job_records", receipt)

    def test_foreign_preflight_failure_never_runs_the_native_fixture(self):
        self.installer.preflight.side_effect = RuntimeError("ownership is not confirmed")
        result, receipt = self.run_main()
        self.assertEqual(result, 1)
        self.assertIsNone(receipt["native_exit"])
        self.assertFalse(receipt["managed_services_preserved"])
        self.assertFalse(receipt["runtime_removed"])
        self.installer.probe.assert_not_called()

    def test_changed_managed_cohort_fails_preservation_and_retains_fixture(self):
        self.after_client = lambda: self.installer.observed["control"]["identity"].update(pid=999)
        result, receipt = self.run_main()
        self.assertEqual(result, 1)
        self.assertFalse(receipt["managed_services_preserved"])
        self.assertIn("managed_preservation_failure", receipt)
        self.assertFalse(receipt["runtime_removed"])

    def test_partial_wrong_digest_profile_or_intent_cannot_pass_managed_preflight(self):
        state = copy.deepcopy(self.installer.state)
        observed = copy.deepcopy(self.installer.observed)
        for invalid in range(6):
            self.installer.state = copy.deepcopy(state)
            self.installer.observed = copy.deepcopy(observed)
            entry = self.installer.state["roles"]["control"]
            if invalid == 0:
                self.installer.state["roles"].pop("state-writer")
            elif invalid == 1:
                entry["phase"] = "creating"
            elif invalid == 2:
                entry["replacement"] = {}
            elif invalid == 3:
                entry["binary_sha256"] = "0" * 64
            elif invalid == 4:
                entry["profile"]["protocol"] = 1
            else:
                self.installer.observed["control"]["intent"] = "unowned"
            with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                VERIFIER.managed_snapshot(self.installer, self.installer.roles, probe=True)
        self.installer.probe.assert_not_called()

    def test_default_isolated_mode_keeps_private_journal_and_cleanup(self):
        result, receipt = self.run_main(managed=False)
        self.assertEqual(result, 0, receipt)
        self.assertEqual(receipt["service_mode"], "isolated")
        self.assertEqual(receipt["durable_jobs"], 1)
        self.assertEqual(receipt["service_cleanup"], "isolated_owned_attempts")
        self.assertIn("labels_absent", receipt)
        self.assertNotIn("managed_services", receipt)
        self.helper.Installer.assert_not_called()

    def test_helper_import_has_no_native_action_or_installer_side_effect(self):
        before = VERIFIER.sys.dont_write_bytecode
        with mock.patch.object(VERIFIER.subprocess, "run", side_effect=AssertionError("native action during import")):
            helper = VERIFIER.service_installer()
        self.assertEqual(set(helper.ROLES), set(self.installer.roles))
        self.assertTrue(callable(helper.Installer.preflight))
        self.assertTrue(callable(helper.Installer.inspect))
        self.assertEqual(VERIFIER.sys.dont_write_bytecode, before)


if __name__ == "__main__":
    unittest.main()
