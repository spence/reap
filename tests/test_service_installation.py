"""Ownership, failure, and recovery controls for the macOS service installer."""

import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("service_installation", ROOT / "tools/install_services_macos.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
NATIVE_COMMAND = MODULE.command


class Native:
    """Model only exact role targets; no mocked call can reach launchd."""

    def __init__(self, home, uid):
        self.home = home
        self.uid = uid
        self.profile = {"schema": 1, "protocol": 2, "uid": uid,
                        "role_profile": "fixed-agentless-v1", "agent_hosts": False,
                        "abandon_process_group": True, "roles": [
                            {"argument": role, "label": "dev.micro.reap." + role,
                             "endpoint": "dev.micro.reap." + {"control": "control", "context-resolver": "resolver",
                                                              "state-writer": "writer"}[role] + ".v1." + str(uid)}
                            for role in MODULE.ROLES]}
        self.jobs = {}
        self.processes = {}
        self.calls = []
        self.next_pid = 12345
        self.bootstrap_fault = None
        self.unknown_status = False
        self.unknown_identity = False
        self.linger = False
        self.probe_failure = False
        self.probe_phase = "ready"
        self.unknown_after_bootout = False
        self.worker = {"pid": 987654, "reservation": "protected target worker"}

    def response(self, args, code=0, value=None, text=None):
        return subprocess.CompletedProcess(args, code, json.dumps(value) if value is not None else (text or ""), "")

    def text(self, target, job):
        spec = job["spec"]
        arguments = "\n".join("\t\t" + argument for argument in spec["ProgramArguments"])
        environment = "\n".join("\t\t" + key + " => " + value for key, value in
                                {"OSLogRateLimit": "64", **spec["EnvironmentVariables"],
                                 "XPC_SERVICE_NAME": spec["Label"]}.items())
        endpoints = "\n".join('\t\t"' + name + '" = {\n\t\t\tactive = 1\n\t\t}' for name in spec["MachServices"])
        pid = "\n\tpid = " + str(job["pid"]) if job.get("pid") else ""
        properties = "abandon process group | inferred program" if spec.get("AbandonProcessGroup") else "inferred program"
        return (f"{target} = {{\n\tpath = {job['path']}\n\tprogram = {spec['ProgramArguments'][0]}\n"
                f"\tworking directory = {spec['WorkingDirectory']}\n"
                f"\targuments = {{\n{arguments}\n\t}}\n\tenvironment = {{\n{environment}\n\t}}\n"
                f"\tendpoints = {{\n{endpoints}\n\t}}\n\tdomain = user/{self.uid} [100001]\n"
                f"\tproperties = {properties}{pid}\n}}\n")

    def __call__(self, args, home):
        assert home == self.home
        self.calls.append(args)
        if args[0] != "/bin/launchctl":
            if args[1] == "__service-profile":
                return self.response(args, value=self.profile)
            if args[1] == "__process-identity":
                pid = int(args[2])
                if self.unknown_identity:
                    return self.response(args, code=1, value={"state": "unknown"})
                identity = self.processes.get(pid)
                return self.response(args, value={"state": "present" if identity else "absent", "identity": identity})
            assert args[1] == "__service-probe", args
            role = args[2]
            target = f"user/{self.uid}/dev.micro.reap.{role}"
            if self.probe_failure or target not in self.jobs:
                return self.response(args, code=1)
            job = self.jobs[target]
            if not job.get("pid"):
                pid = self.next_pid
                self.next_pid += 1
                self.processes[pid] = {"pid": pid, "uid": self.uid, "boot": "ab-cd", "start_seconds": pid, "start_fraction": 1}
                job["pid"] = pid
            return self.response(args, value={"protocol": 2, "via": role.replace("-", "_"),
                                              "snapshot": {"phase": self.probe_phase}, "outcome": {"result": "pong"}})
        action = args[1]
        if action == "print":
            if self.unknown_status:
                return self.response(args, code=5)
            job = self.jobs.get(args[2])
            return self.response(args, text=self.text(args[2], job)) if job else self.response(args, code=113)
        if action == "bootstrap":
            assert args[2] == f"user/{self.uid}"
            path = Path(args[3])
            spec = plistlib.loads(path.read_bytes())
            role = spec["ProgramArguments"][2]
            record = json.loads((self.home / ".local/state/reap/service-installation/ownership.json").read_text())
            entry = record["roles"][role]
            assert entry["intent"] == spec["EnvironmentVariables"][MODULE.TOKEN]
            assert entry["plist"] == spec and entry["phase"] == "creating"
            assert spec["AbandonProcessGroup"] is True
            target = args[2] + "/" + spec["Label"]
            assert target not in self.jobs, "duplicate role bootstrap"
            fault = self.bootstrap_fault
            self.bootstrap_fault = None
            if fault in ("refused", "timeout-absent"):
                if fault == "timeout-absent":
                    raise subprocess.TimeoutExpired(args, 5)
                return self.response(args, code=5)
            self.jobs[target] = {"spec": spec, "path": str(path)}
            if fault == "timeout-unknown":
                self.unknown_status = True
            if fault in ("timeout-present", "timeout-unknown"):
                raise subprocess.TimeoutExpired(args, 5)
            return self.response(args)
        assert action == "bootout" and len(args) == 3, args
        target = args[2]
        job = self.jobs[target]
        role = job["spec"]["ProgramArguments"][2]
        state = json.loads((self.home / ".local/state/reap/service-installation/ownership.json").read_text())
        entry = state["roles"][role]
        assert entry["phase"] == "retiring" and entry["replacement"]
        assert entry["old_process"] == self.processes[job["pid"]]
        assert job["spec"]["AbandonProcessGroup"] is True
        self.jobs.pop(target)
        if not self.linger:
            self.processes.pop(job["pid"])
        if self.unknown_after_bootout:
            self.unknown_identity = True
        return self.response(args)


class InstallationTests(unittest.TestCase):
    def setUp(self):
        base = ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="service-install-", dir=base)
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.home = self.base / "ordinary home"
        self.home.mkdir()
        self.binary = self.home / ".cargo/bin/reap"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(b"old binary")
        self.candidate = self.base / "candidate"
        self.candidate.write_bytes(b"new binary")
        self.native = Native(self.home, os.getuid())
        patcher = mock.patch.object(MODULE, "command", self.native)
        patcher.start()
        self.addCleanup(patcher.stop)

    def installer(self, candidate=None):
        return MODULE.Installer(self.home, os.getuid(), self.binary, candidate or self.candidate)

    def install(self):
        installer = self.installer()
        plan = installer.preflight()
        self.binary.write_bytes(self.candidate.read_bytes())
        return self.installer(self.binary).apply(plan)

    def actions(self, action):
        return [call for call in self.native.calls if call[:2] == ["/bin/launchctl", action]]

    def state(self):
        return json.loads((self.home / ".local/state/reap/service-installation/ownership.json").read_text())

    def files(self):
        return {str(path.relative_to(self.home)): path.read_bytes()
                for path in self.home.rglob("*") if path.is_file()}

    def test_initial_install_and_idempotent_rerun_are_one_cohort_and_on_demand(self):
        result = self.install()
        self.assertEqual(result["state"], "installed")
        self.assertEqual(len(self.actions("bootstrap")), 3)
        self.assertEqual(len(self.actions("bootout")), 0)
        state = self.state()
        for role in MODULE.ROLES:
            spec = state["roles"][role]["plist"]
            self.assertTrue(spec["AbandonProcessGroup"])
            self.assertNotIn("RunAtLoad", spec)
            self.assertNotIn("KeepAlive", spec)
            self.assertNotIn("StartInterval", spec)
            self.assertNotIn("StandardOutPath", spec)
            self.assertNotIn("StandardErrorPath", spec)
            self.assertEqual(spec["EnvironmentVariables"]["HOME"], str(self.home))
            self.assertEqual(spec["EnvironmentVariables"]["XDG_STATE_HOME"], str(self.home / ".local/state"))
        self.install()
        self.assertEqual(len(self.actions("bootstrap")), 3)
        self.assertFalse(self.actions("bootout"))
        self.assertTrue(all(call[2].startswith((f"user/{os.getuid()}/dev.micro.reap.",
                                               f"gui/{os.getuid()}/dev.micro.reap."))
                            for call in self.actions("print")))

    def test_native_logging_default_is_narrow_and_unknown_environment_still_refuses(self):
        self.install()
        installer = self.installer()
        target = installer.domain + "/" + installer.role("control")["label"]
        original = self.native.jobs[target]["spec"]["EnvironmentVariables"].copy()
        for extra in ({"OSLogRateLimit": "65"}, {"UNKNOWN_SERVICE_OVERRIDE": "value"}):
            with self.subTest(extra=extra):
                self.native.jobs[target]["spec"]["EnvironmentVariables"] = dict(original, **extra)
                before = self.files()
                with self.assertRaises(MODULE.Conflict):
                    installer.preflight()
                self.assertEqual(before, self.files())
        self.native.jobs[target]["spec"]["EnvironmentVariables"] = original
        self.assertFalse(self.actions("bootout"))

    def test_foreign_registration_and_plist_preflight_preserve_all_files(self):
        installer = self.installer()
        role = MODULE.ROLES[-1]
        self.native.jobs[installer.domain + "/" + installer.role(role)["label"]] = {
            "spec": installer.plist(role, "foreign"), "path": str(installer.plist_path(role))}
        before = self.files()
        with self.assertRaises(MODULE.Conflict):
            installer.preflight()
        self.assertEqual(before, self.files())
        self.assertFalse(self.actions("bootstrap") + self.actions("bootout"))
        self.native.jobs.clear()
        installer.agents.mkdir(parents=True)
        installer.plist_path(role).write_bytes(plistlib.dumps(installer.plist(role, "unknown")))
        before = self.files()
        with self.assertRaises(MODULE.Conflict):
            installer.preflight()
        self.assertEqual(before, self.files())

    def test_owned_registration_mutations_refuse_without_bootout(self):
        self.install()
        installer = self.installer()
        target = installer.domain + "/" + installer.role("control")["label"]
        original = copy.deepcopy(self.native.jobs[target])
        for field in ("ProgramArguments", "MachServices", "EnvironmentVariables", "AbandonProcessGroup"):
            with self.subTest(field=field):
                self.native.jobs[target] = copy.deepcopy(original)
                spec = self.native.jobs[target]["spec"]
                if field == "ProgramArguments":
                    spec[field] = ["/Applications/Zed.app/foreign", "__service", "control"]
                elif field == "MachServices":
                    spec[field]["foreign.endpoint"] = True
                elif field == "EnvironmentVariables":
                    spec[field][MODULE.TOKEN] = "not-the-recorded-creation"
                else:
                    spec[field] = False
                before = self.files()
                with self.assertRaises(MODULE.Conflict):
                    installer.preflight()
                self.assertEqual(before, self.files())
        self.native.jobs[target] = original
        self.assertFalse(self.actions("bootout"))

    def test_gui_cohort_conflict_and_unknown_launchctl_are_not_absence(self):
        installer = self.installer()
        target = f"gui/{os.getuid()}/" + installer.role("control")["label"]
        self.native.jobs[target] = {"spec": installer.plist("control", "foreign"),
                                    "path": str(installer.plist_path("control"))}
        with self.assertRaises(MODULE.Conflict):
            installer.preflight()
        self.native.jobs.clear()
        self.native.unknown_status = True
        with self.assertRaises(MODULE.Conflict):
            installer.preflight()
        self.assertFalse(self.actions("bootstrap") + self.actions("bootout"))

    def test_invalid_profile_and_unknown_process_identity_refuse(self):
        self.install()
        self.native.unknown_identity = True
        with self.assertRaises(MODULE.Conflict):
            self.installer().preflight()
        self.native.unknown_identity = False
        for field, value in (("agent_hosts", True), ("abandon_process_group", False), ("protocol", 1)):
            saved = self.native.profile[field]
            self.native.profile[field] = value
            with self.assertRaises(MODULE.Conflict):
                self.installer()
            self.native.profile[field] = saved
        self.assertFalse(self.actions("bootout"))

    def test_bootstrap_timeout_reconciles_the_same_durable_intent(self):
        self.native.bootstrap_fault = "timeout-unknown"
        with self.assertRaises(MODULE.Conflict):
            self.install()
        intent = self.state()["roles"]["control"]["intent"]
        self.native.unknown_status = False
        self.install()
        self.assertEqual(self.state()["roles"]["control"]["intent"], intent)
        self.assertEqual(len(self.actions("bootstrap")), 3)

    def failed_bootstrap(self, fault):
        self.native.bootstrap_fault = fault
        with self.assertRaises(MODULE.Conflict):
            self.install()
        intent = self.state()["roles"]["control"]["intent"]
        self.native.bootstrap_fault = None
        self.install()
        self.assertEqual(self.state()["roles"]["control"]["intent"], intent)

    def test_refused_bootstrap_retains_intent_for_retry(self):
        self.failed_bootstrap("refused")

    def test_timed_out_absent_bootstrap_retains_intent_for_retry(self):
        self.failed_bootstrap("timeout-absent")

    def test_upgrade_stops_only_owned_roles_and_preserves_worker(self):
        installer = self.installer()
        installer.agents.mkdir(parents=True)
        maintenance = installer.agents / "dev.micro.reap.maintain.plist"
        maintenance.write_bytes(b"protected existing maintenance job")
        maintenance_target = installer.domain + "/dev.micro.reap.maintain"
        self.native.jobs[maintenance_target] = {"foreign": "protected maintenance registration"}
        self.install()
        worker = copy.deepcopy(self.native.worker)
        old = {entry["last_process"]["pid"] for entry in self.state()["roles"].values()}
        self.candidate.write_bytes(b"upgraded binary")
        self.install()
        self.assertEqual(len(self.actions("bootout")), 3)
        self.assertEqual(len(self.actions("bootstrap")), 6)
        self.assertTrue(old.isdisjoint(self.native.processes))
        self.assertEqual(worker, self.native.worker)
        self.assertEqual(maintenance.read_bytes(), b"protected existing maintenance job")
        self.assertEqual(self.native.jobs[maintenance_target], {"foreign": "protected maintenance registration"})
        self.assertFalse(any(maintenance_target in call for call in self.native.calls))
        for call in self.actions("bootout"):
            self.assertEqual(len(call), 3)
            self.assertTrue(call[2].startswith(f"user/{os.getuid()}/dev.micro.reap."))

    def test_running_process_timeout_retains_recovery_and_never_counts_as_exit(self):
        self.install()
        self.candidate.write_bytes(b"upgraded binary")
        self.native.linger = True
        with mock.patch.object(MODULE.time, "monotonic", side_effect=[0, 0, 4]), \
                self.assertRaises(MODULE.Conflict):
            self.install()
        entry = self.state()["roles"]["control"]
        self.assertEqual(entry["phase"], "retiring")
        self.assertIn("replacement", entry)
        old = entry["old_process"]
        self.assertEqual(self.native.processes[old["pid"]], old)
        self.assertEqual(len(self.actions("bootstrap")), 3)
        with self.assertRaises(MODULE.Conflict):
            self.installer().preflight()
        self.native.processes.pop(old["pid"])
        self.native.linger = False
        self.install()
        self.assertEqual(len(self.actions("bootstrap")), 6)

    def test_changed_evidence_after_preflight_is_a_partial_apply_not_adoption(self):
        installer = self.installer()
        plan = installer.preflight()
        role = MODULE.ROLES[-1]
        self.native.jobs[installer.domain + "/" + installer.role(role)["label"]] = {
            "spec": installer.plist(role, "foreign"), "path": str(installer.plist_path(role))}
        self.binary.write_bytes(self.candidate.read_bytes())
        with self.assertRaises(MODULE.Conflict):
            self.installer(self.binary).apply(plan)
        self.assertFalse(self.actions("bootstrap") + self.actions("bootout"))
        self.assertEqual(self.binary.read_bytes(), b"new binary")

    def test_failed_probe_retains_exact_creation_for_a_later_rerun(self):
        self.native.probe_failure = True
        with self.assertRaises(MODULE.Conflict):
            self.install()
        intent = self.state()["roles"]["control"]["intent"]
        self.native.probe_failure = False
        self.install()
        self.assertEqual(self.state()["roles"]["control"]["intent"], intent)
        self.assertEqual(len(self.actions("bootstrap")), 3)

    def test_unavailable_snapshot_is_not_fully_installed(self):
        self.native.probe_phase = "unavailable"
        with self.assertRaises(MODULE.Conflict):
            self.install()
        self.assertEqual(self.state()["roles"]["context-resolver"]["phase"], "creating")
        self.native.probe_phase = "busy"
        self.install()
        self.assertEqual(len(self.actions("bootstrap")), 3)

    def test_control_cached_starting_snapshot_is_not_a_backend_health_probe(self):
        native = self.native

        def cached_control(args, home):
            result = native(args, home)
            if args[1:] == ["__service-probe", "control"]:
                value = json.loads(result.stdout)
                value["snapshot"]["phase"] = "starting"
                return native.response(args, value=value)
            return result

        with mock.patch.object(MODULE, "command", cached_control):
            self.install()
        self.assertEqual(len(self.actions("bootstrap")), 3)

    def test_unknown_old_process_absence_preserves_refresh_evidence(self):
        self.install()
        self.candidate.write_bytes(b"upgraded binary")
        self.native.unknown_after_bootout = True
        with self.assertRaises(MODULE.Conflict):
            self.install()
        entry = self.state()["roles"]["control"]
        self.assertIn("old_process", entry)
        self.assertIn("replacement", entry)
        self.assertEqual(len(self.actions("bootstrap")), 3)
        self.native.unknown_after_bootout = False
        self.native.unknown_identity = False
        self.install()
        self.assertEqual(len(self.actions("bootstrap")), 6)

    def test_refresh_recovers_after_plist_publication_before_state_promotion(self):
        self.install()
        self.candidate.write_bytes(b"upgraded binary")
        installer = self.installer()
        plan = installer.preflight()
        self.binary.write_bytes(self.candidate.read_bytes())
        apply = self.installer(self.binary)
        save = apply.save
        old_intent = self.state()["roles"]["control"]["intent"]

        def interrupted(state):
            entry = state["roles"]["control"]
            if entry["intent"] != old_intent:
                raise OSError("fixture interruption after plist publication")
            save(state)

        with mock.patch.object(apply, "save", interrupted), self.assertRaises(OSError):
            apply.apply(plan)
        state = self.state()
        replacement = state["roles"]["control"]["replacement"]
        self.assertEqual(plistlib.loads(apply.plist_path("control").read_bytes()), replacement["plist"])
        self.install()
        self.assertEqual(self.state()["roles"]["control"]["intent"], replacement["intent"])
        self.assertEqual(len(self.actions("bootstrap")), 6)

    def test_intent_write_failure_prevents_bootstrap(self):
        installer = self.installer()
        plan = installer.preflight()
        self.binary.write_bytes(self.candidate.read_bytes())
        apply = self.installer(self.binary)
        save = apply.save

        def no_creation(state):
            if state["roles"]:
                raise OSError("fixture failed durable creation record")
            save(state)

        with mock.patch.object(apply, "save", no_creation), self.assertRaises(OSError):
            apply.apply(plan)
        self.assertFalse(self.actions("bootstrap"))
        self.install()
        self.assertEqual(len(self.actions("bootstrap")), 3)

    def test_late_foreign_plist_is_preserved_before_bootstrap(self):
        installer = self.installer()
        plan = installer.preflight()
        self.binary.write_bytes(self.candidate.read_bytes())
        apply = self.installer(self.binary)
        preflight = apply.preflight
        calls = 0
        foreign = b"foreign protected plist"

        def late_plist():
            nonlocal calls
            value = preflight()
            calls += 1
            if calls == 2:
                apply.agents.mkdir(parents=True)
                apply.plist_path("control").write_bytes(foreign)
            return value

        with mock.patch.object(apply, "preflight", late_plist), self.assertRaises(ValueError):
            apply.apply(plan)
        self.assertEqual(apply.plist_path("control").read_bytes(), foreign)
        self.assertFalse(self.actions("bootstrap"))

    def test_unowned_or_symlinked_metadata_is_not_adopted(self):
        installer = self.installer()
        installer.root.mkdir(parents=True)
        unknown = installer.root / "unowned"
        unknown.write_text("protected survivor")
        with self.assertRaises(MODULE.Conflict):
            installer.preflight()
        unknown.unlink()
        installer.root.rmdir()
        other = self.home / "other"
        other.mkdir()
        installer.root.symlink_to(other, target_is_directory=True)
        with self.assertRaises(MODULE.Conflict):
            installer.preflight()

    def test_native_command_fixes_home_and_default_state_without_caller_fallback(self):
        with mock.patch.dict(os.environ, {"HOME": "/foreign/home", "XDG_STATE_HOME": "/foreign/state"}), \
                mock.patch.object(MODULE.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, "{}", "")
            NATIVE_COMMAND(["native-probe"], self.home)
        environment = run.call_args.kwargs["env"]
        self.assertEqual(environment["HOME"], str(self.home))
        self.assertEqual(environment["XDG_STATE_HOME"], str(self.home / ".local/state"))
        self.assertEqual(run.call_args.kwargs["timeout"], 5)

    def test_entry_point_uses_host_account_home_in_both_install_phases(self):
        plan = self.base / "plan.json"
        account = mock.Mock(pw_dir=str(self.home))
        with mock.patch.object(MODULE.sys, "platform", "darwin"), \
                mock.patch.object(MODULE.pwd, "getpwuid", return_value=account), \
                mock.patch.object(MODULE.sys, "stdout", io.StringIO()), \
                mock.patch.dict(os.environ, {"HOME": "/caller/home", "XDG_STATE_HOME": "/caller/state"}):
            with mock.patch.object(MODULE.sys, "argv", ["install_services_macos.py", "preflight", "--candidate",
                                                       str(self.candidate), "--binary", str(self.binary), "--plan", str(plan)]):
                self.assertEqual(MODULE.main(), 0)
            self.binary.write_bytes(self.candidate.read_bytes())
            with mock.patch.object(MODULE.sys, "argv", ["install_services_macos.py", "apply", "--binary",
                                                       str(self.binary), "--plan", str(plan)]):
                self.assertEqual(MODULE.main(), 0)
        self.assertEqual(self.state()["home"], str(self.home))

    def test_concurrent_apply_refuses_without_another_bootstrap(self):
        self.install()
        installer = self.installer()
        plan = installer.preflight()
        with (installer.root / "install.lock").open("r+") as lock:
            MODULE.fcntl.flock(lock, MODULE.fcntl.LOCK_EX | MODULE.fcntl.LOCK_NB)
            with self.assertRaises(MODULE.Conflict):
                installer.apply(plan)
        self.assertEqual(len(self.actions("bootstrap")), 3)
        self.assertFalse(self.actions("bootout"))


if __name__ == "__main__":
    unittest.main()
