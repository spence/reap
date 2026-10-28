"""Run preservation-veto regressions through an actual Reap executable."""

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
GIT_GLOBAL = os.environ.get("GIT_CONFIG_GLOBAL", str(pathlib.Path.home() / ".gitconfig"))
ROLES = ("conversation_source", "permanent_archive")
PAYLOAD = b"synthetic protected conversation bytes\n"


class Fixture:
    def __init__(self, base=None):
        base = base or ROOT / "target/control-t13"
        base.mkdir(parents=True, exist_ok=True)
        self.root = pathlib.Path(tempfile.mkdtemp(prefix="preservation-cli-", dir=base)).resolve()
        self.unconfirmed_jobs = 0
        self.home = self.root / "home"
        self.projects = self.home / "projects"
        self.state = self.root / "state/reap"
        self.quarantine = self.state / "quarantine"
        self.projects.mkdir(parents=True)
        self.quarantine.mkdir(parents=True)
        (self.home / ".config/reap").mkdir(parents=True)
        self.config = self.home / ".config/reap/config.json"
        self.config.write_text(json.dumps({
            "roots": [str(self.projects)], "governed_roots": [str(self.projects)],
            "reap_file_grace_hours": 0, "reap_file_min_age_minutes": 0,
            "quarantine": {"auto_purge": True, "purge_after_days": 0},
        }))
        self.environment = dict(os.environ, HOME=str(self.home),
                                XDG_STATE_HOME=str(self.root / "state"))
        for name in ("REAP_OWNER", "REAP_SESSION", "REAP_INTERNAL_CONFIG_SNAPSHOT",
                     "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_CONFIG_GLOBAL"):
            self.environment.pop(name, None)

    def close(self):
        if self.unconfirmed_jobs:
            (self.root / ".reap").write_text(json.dumps({
                "version": 1, "keep": {"reason": "unconfirmed preservation fixture job"},
            }))
            print(f"Retained unconfirmed preservation fixture: {self.root}")
        else:
            shutil.rmtree(self.root)

    def run(self, *arguments):
        files = arguments and arguments[0] == "files"
        coordinated = arguments and arguments[0] in ("files", "plan", "clean", "sweep", "lease")
        if coordinated:
            self.unconfirmed_jobs += 1
            arguments = (*arguments, "--wait", "30")
        result = subprocess.run([str(BINARY), *map(str, arguments)], cwd=self.root,
                                env=self.environment, capture_output=True, timeout=35 if coordinated else 30)
        if coordinated:
            receipt = json.loads(result.stdout)
            assert receipt["protocol"] == 2, receipt
            if receipt["state"] == "completed":
                self.unconfirmed_jobs -= 1
        if files:
            detail = json.loads(receipt["job"]["result"]["detail"])
            result.stdout = detail["stdout"].encode()
            result.stderr = detail["stderr"].encode()
        return result

    def role(self, directory, role):
        directory.mkdir(parents=True, exist_ok=True)
        (directory / ".reap").write_text(json.dumps({
            "version": 2, "role": role, "keep": {"reason": "synthetic preservation fixture"},
            "seal": role == "permanent_archive",
        }))

    def payload(self, directory):
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "payload"
        path.write_bytes(PAYLOAD)
        return path

    def age(self, directory):
        stamp = time.time() - 40 * 86400
        for parent, directories, files in os.walk(directory, followlinks=False):
            for name in files + directories:
                os.utime(pathlib.Path(parent) / name, (stamp, stamp), follow_symlinks=False)
        os.utime(directory, (stamp, stamp), follow_symlinks=False)

    def retire(self, directory):
        added = self.run("lease", "add", directory, "--ttl", "0", "--scratch",
                         "--owner", "preservation-fixture", "--purpose", "synthetic disposable unit")
        assert added.returncode == 0, added
        self.age(directory)
        retired = self.run("retire", directory, "--now", "--apply", "--min-age-minutes", "0")
        assert retired.returncode == 0, retired
        index = json.loads((self.quarantine / "index.json").read_text())
        entry = next(item for item in index["entries"] if item["original_path"] == str(directory))
        return entry["id"], self.quarantine / "entries" / entry["id"] / entry["name"]


@unittest.skipUnless(BINARY, "install reap or set REAP_TEST_BIN")
class Preservation(unittest.TestCase):
    def fixture(self, base=None):
        fixture = Fixture(base)
        self.addCleanup(fixture.close)
        return fixture

    def assert_payload(self, path):
        self.assertTrue(path.is_file(), f"protected payload was removed: {path}")
        self.assertEqual(path.read_bytes(), PAYLOAD)

    def test_legacy_retire_and_files_preserve_non_branch_reference_metadata(self):
        for operation in ("retire", "files"):
            with self.subTest(operation=operation):
                fixture = self.fixture(pathlib.Path(tempfile.gettempdir()))
                workspace = fixture.projects / "ref-workspace"
                workspace.mkdir()

                def git(*arguments):
                    result = subprocess.run(
                        ["/usr/bin/git", "-c", "core.hooksPath=/dev/null",
                         "-c", "commit.gpgsign=false", *arguments], cwd=workspace,
                        env=dict(fixture.environment, GIT_CONFIG_GLOBAL=GIT_GLOBAL),
                        capture_output=True, timeout=5)
                    self.assertEqual(result.returncode, 0, result)
                    return result.stdout.strip()

                git("init", "-q", "--initial-branch=main")
                payload = fixture.payload(workspace)
                git("add", "payload")
                git("commit", "-qm", "initial fixture")
                git("update-ref", "refs/remotes/origin/main", "HEAD")
                git("config", "remote.origin.url", "../remote")
                git("config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
                git("config", "branch.main.remote", "origin")
                git("config", "branch.main.merge", "refs/heads/main")
                payload.write_bytes(b"private ref payload\n")
                git("commit", "-qam", "private fixture")
                unique = git("rev-parse", "HEAD")
                git("update-ref", "refs/custom/retained", unique.decode())
                git("reset", "--hard", "HEAD~1")
                git("pack-refs", "--all")
                if operation == "retire":
                    added = fixture.run("lease", "add", workspace, "--ttl", "0",
                                        "--owner", "ref-fixture", "--purpose", "synthetic ref fixture")
                    self.assertEqual(added.returncode, 0, added)
                else:
                    (workspace / ".reap").write_text(json.dumps({
                        "version": 1, "expires": "2020-01-01T00:00:00Z"}))
                fixture.age(workspace)
                if operation == "retire":
                    result = fixture.run("retire", workspace, "--now", "--apply", "--min-age-minutes", "0")
                else:
                    result = fixture.run("files", fixture.projects, "--apply")
                self.assert_payload(payload)
                self.assertIn(b"non-branch reference metadata", result.stdout + result.stderr)
                self.assertEqual(git("rev-parse", "refs/custom/retained"), unique)

    def test_cargo_ancestor_roles_veto_compaction_but_ordinary_keep_does_not(self):
        for role in (*ROLES, "ordinary"):
            with self.subTest(role=role):
                fixture = self.fixture()
                project = fixture.root / "cargo"
                target = project / "target"
                deps = target / "debug/deps"
                deps.mkdir(parents=True)
                (project / "Cargo.toml").write_text("[workspace]\n")
                (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55")
                old = deps / "libfixture-0000000000000001.rlib"
                new = deps / "libfixture-0000000000000002.rlib"
                old.write_bytes(PAYLOAD)
                new.write_bytes(b"new build")
                fixture.age(old)
                fixture.role(project, role)
                result = fixture.run("clean", project, "--min-age-minutes", "0")
                self.assertTrue(new.is_file(), result)
                if role == "ordinary":
                    self.assertEqual(result.returncode, 0, result)
                    self.assertFalse(old.exists(), "ordinary KEEP must permit Cargo compaction")
                else:
                    self.assert_payload(old)

    def test_cargo_nested_role_survives_without_retaining_an_unrelated_candidate(self):
        for role in ROLES:
            with self.subTest(role=role):
                fixture = self.fixture()
                project = fixture.root / "cargo"
                target = project / "target"
                (target / "debug/incremental").mkdir(parents=True)
                (project / "Cargo.toml").write_text("[workspace]\n")
                (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55")
                protected = target / "debug/incremental/protected/history"
                payload = fixture.payload(protected)
                fixture.role(protected, role)
                ordinary = fixture.payload(target / "debug/incremental/disposable")
                fixture.age(target / "debug")
                result = fixture.run("clean", project, "--min-age-minutes", "0")
                self.assert_payload(payload)
                self.assertFalse(ordinary.exists(), result)

    def test_store_delete_and_quarantine_preserve_nested_roles_and_remove_eligible_siblings(self):
        for role in ROLES:
            for disposition in ("delete", "quarantine"):
                with self.subTest(role=role, disposition=disposition):
                    fixture = self.fixture()
                    project = fixture.root / "store-project"
                    project.mkdir()
                    (project / ".reap.json").write_text(json.dumps({
                        "version": 2, "stores": [{"path": "logs", "disposition": disposition,
                            "retention": {"keep_last": 0, "min_age_hours": 0, "max_age_days": 1}}],
                    }))
                    initialized = fixture.run("stores", "--init", project)
                    self.assertEqual(initialized.returncode, 0, initialized)
                    store = project / "logs"
                    payload = fixture.payload(store / "protected/history")
                    fixture.role(payload.parent, role)
                    ordinary = fixture.payload(store / "disposable")
                    fixture.age(store)
                    result = fixture.run("stores", "--apply", project)
                    self.assert_payload(payload)
                    self.assertFalse(ordinary.exists(), result)
                    self.assertTrue((store / "REAP-STORE.TAG").is_file())

    def test_expired_seal_cannot_remove_nested_roles_while_independent_unit_retires(self):
        for role in ROLES:
            with self.subTest(role=role):
                fixture = self.fixture()
                protected = fixture.projects / "expired-seal"
                payload = fixture.payload(protected / "history")
                fixture.role(payload.parent, role)
                disposable = fixture.projects / "disposable"
                fixture.payload(disposable)
                for directory in (protected, disposable):
                    (directory / ".reap").write_text(json.dumps({
                        "version": 1, "expires": "2020-01-01T00:00:00Z",
                        "scratch": True, "seal": True,
                    }))
                    fixture.age(directory)
                result = fixture.run("files", "--apply")
                self.assert_payload(payload)
                self.assertFalse(disposable.exists(), result)

    def test_scratch_and_now_cannot_retire_exact_nested_or_ancestor_roles(self):
        for role in ROLES:
            for position in ("exact", "nested", "ancestor"):
                with self.subTest(role=role, position=position):
                    fixture = self.fixture()
                    directory = fixture.root / "container/leased"
                    payload = fixture.payload(directory)
                    added = fixture.run("lease", "add", directory, "--ttl", "0", "--scratch",
                                        "--owner", "preservation-fixture")
                    self.assertEqual(added.returncode, 0, added)
                    where = {"exact": directory, "nested": directory / "history",
                             "ancestor": directory.parent}[position]
                    fixture.role(where, role)
                    fixture.age(directory.parent)
                    result = fixture.run("retire", directory, "--now", "--apply", "--min-age-minutes", "0")
                    self.assert_payload(payload)
                    self.assertNotEqual(result.returncode, 0, "refused exact retirement must not report success")

    def test_every_purge_selector_preserves_roles_and_keeps_recovery_metadata(self):
        for role in ROLES:
            for selector in ("id", "all", "owner", "auto"):
                with self.subTest(role=role, selector=selector):
                    fixture = self.fixture()
                    protected = fixture.root / "protected"
                    disposable = fixture.root / "disposable"
                    fixture.payload(protected)
                    fixture.payload(disposable)
                    key, destination = fixture.retire(protected)
                    other, other_destination = fixture.retire(disposable)
                    fixture.role(destination, role)
                    args = {"id": ["--id", key], "all": ["--all"],
                            "owner": ["--owner", "preservation-fixture"], "auto": []}[selector]
                    result = fixture.run("purge", *args, "--apply")
                    self.assert_payload(destination / "payload")
                    rows = json.loads((fixture.quarantine / "index.json").read_text())["entries"]
                    self.assertIn(key, [row["id"] for row in rows], "protected recovery record must remain")
                    if selector == "id":
                        result = fixture.run("purge", "--id", other, "--apply")
                    self.assertFalse(other_destination.exists(), result)

    def test_orphan_purge_preserves_a_nested_role_and_removes_only_the_unprotected_slot(self):
        for role in ROLES:
            with self.subTest(role=role):
                fixture = self.fixture()
                protected = fixture.quarantine / "entries/abcdef12/capture/history"
                payload = fixture.payload(protected)
                fixture.role(protected, role)
                ordinary = fixture.quarantine / "entries/fedcba21/cache"
                fixture.payload(ordinary)
                result = fixture.run("purge", "--all", "--apply")
                self.assert_payload(payload)
                self.assertFalse(ordinary.exists(), result)

    def test_ambiguous_declarations_refuse_purge_without_opening_special_files(self):
        for kind in ("symlink", "hardlink", "fifo", "invalid", "unknown_role", "duplicate", "oversized"):
            with self.subTest(kind=kind):
                fixture = self.fixture()
                protected = fixture.quarantine / "entries/abcdef12/capture"
                payload = fixture.payload(protected)
                declaration = protected / ".reap"
                external = fixture.root / "external-policy"
                external.write_text(json.dumps({"version": 2, "role": "permanent_archive",
                    "keep": {"reason": "synthetic source policy"}, "seal": True}))
                if kind == "symlink":
                    declaration.symlink_to(external)
                elif kind == "hardlink":
                    os.link(external, declaration)
                elif kind == "fifo":
                    os.mkfifo(declaration)
                elif kind == "invalid":
                    declaration.write_text("{incomplete")
                elif kind == "unknown_role":
                    declaration.write_text('{"version":2,"role":"unknown"}')
                elif kind == "duplicate":
                    declaration.write_text('{"version":2,"role":"permanent_archive","role":"ordinary"}')
                else:
                    declaration.write_bytes(b" " * (65536 + 1))
                ordinary = fixture.quarantine / "entries/fedcba21/cache"
                fixture.payload(ordinary)
                result = fixture.run("purge", "--all", "--apply")
                self.assert_payload(payload)
                self.assertFalse(ordinary.exists(), result)
                self.assertTrue(external.is_file())

    def test_cargo_removes_a_symlink_unit_without_following_its_archive_target(self):
        fixture = self.fixture()
        project = fixture.root / "cargo"
        target = project / "target"
        incremental = target / "debug/incremental"
        incremental.mkdir(parents=True)
        (project / "Cargo.toml").write_text("[workspace]\n")
        (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55")
        external = fixture.root / "archive"
        payload = fixture.payload(external)
        fixture.role(external, "permanent_archive")
        alias = incremental / "disposable-alias"
        alias.symlink_to(external, target_is_directory=True)
        fixture.age(incremental)
        result = fixture.run("clean", project, "--min-age-minutes", "0")
        self.assert_payload(payload)
        self.assertFalse(alias.is_symlink(), result)

    @unittest.skipIf(os.geteuid() == 0, "requires ordinary user file permissions")
    def test_cargo_execution_failure_returns_nonzero_after_independent_cleanup(self):
        fixture = self.fixture()
        project = fixture.root / "cargo"
        target = project / "target"
        deps = target / "debug/deps"
        deps.mkdir(parents=True)
        (project / "Cargo.toml").write_text("[workspace]\n")
        (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55")
        old = deps / "libfixture-0000000000000001.rlib"
        new = deps / "libfixture-0000000000000002.rlib"
        old.write_bytes(PAYLOAD)
        fixture.age(old)
        new.write_bytes(b"new build")
        disposable = fixture.payload(target / "debug/incremental/disposable")
        fixture.age(disposable.parent)
        deps.chmod(0o555)
        try:
            result = fixture.run("clean", project, "--min-age-minutes", "0")
        finally:
            deps.chmod(0o755)
        self.assert_payload(old)
        self.assertTrue(new.is_file())
        self.assertFalse(disposable.exists(), result)
        self.assertNotEqual(result.returncode, 0, "incomplete application must not report success")

    def test_lease_release_keeps_a_marker_inside_a_preservation_role(self):
        for role in ROLES:
            with self.subTest(role=role):
                fixture = self.fixture()
                directory = fixture.root / "leased"
                payload = fixture.payload(directory)
                added = fixture.run("lease", "add", directory, "--ttl", "0", "--scratch")
                self.assertEqual(added.returncode, 0, added)
                marker = directory / ".reap-lease"
                before = marker.read_bytes()
                fixture.role(directory, role)
                released = fixture.run("lease", "release", directory)
                self.assertTrue(marker.is_file(), released)
                self.assertEqual(marker.read_bytes(), before)
                self.assert_payload(payload)

    def test_lease_creation_cannot_replace_payload_marker_bytes_inside_a_role(self):
        for role in ROLES:
            with self.subTest(role=role):
                fixture = self.fixture()
                directory = fixture.root / "history"
                payload = fixture.payload(directory)
                fixture.role(directory, role)
                marker = directory / ".reap-lease"
                marker.write_bytes(PAYLOAD)
                before = sorted(path.name for path in directory.iterdir())
                result = fixture.run("lease", "add", directory, "--ttl", "0", "--scratch")
                self.assertEqual(marker.read_bytes(), PAYLOAD, result)
                self.assertEqual(sorted(path.name for path in directory.iterdir()), before, result)
                self.assert_payload(payload)
                self.assertNotEqual(result.returncode, 0)

    def test_cargo_relative_parent_components_keep_their_filesystem_meaning(self):
        fixture = self.fixture()
        project = fixture.root / "cargo"
        target = project / "target"
        (fixture.root / "step").mkdir()
        disposable = fixture.payload(target / "debug/incremental/disposable")
        (project / "Cargo.toml").write_text("[workspace]\n")
        (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55")
        fixture.age(disposable.parent)
        result = fixture.run("clean", "step/../cargo", "--min-age-minutes", "0")
        self.assertEqual(result.returncode, 0, result)
        self.assertFalse(disposable.exists(), result)

    def test_marker_links_outside_a_role_cannot_change_its_payload(self):
        for kind in ("symlink", "hardlink"):
            with self.subTest(kind=kind):
                fixture = self.fixture()
                archive = fixture.root / "archive"
                payload = fixture.payload(archive)
                fixture.role(archive, "permanent_archive")
                ordinary = fixture.root / "ordinary"
                ordinary.mkdir()
                marker = ordinary / ".reap-lease"
                if kind == "symlink":
                    marker.symlink_to(payload)
                else:
                    os.link(payload, marker)
                result = fixture.run("lease", "add", ordinary, "--ttl", "0", "--scratch")
                self.assert_payload(payload)
                self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
