import contextlib
import hashlib
import io
import json
import os
import sqlite3
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock


sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tools'))
import refresh_mini_accounting as audit
from audit_exclusions import Exclusions


class AccountingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.root = self.base / 'scan'
        self.root.mkdir()
        self.policy = self.base / 'exclusions.json'
        self.policy.write_text(json.dumps({'version': 1, 'paths': []}))
        self.evidence = audit.Evidence(Exclusions(self.policy))
        self.pid = 'github.com/example/project'
        self.evidence.projects[self.pid] = {
            'project_id': self.pid, 'primary_root': str(self.root), 'identity_basis': 'Fixture',
        }
        self.previous = self.base / 'previous.sqlite'
        self.previous.write_bytes(b'untouched prior audit')
        self.output = self.base / 'accounting.sqlite'

    def tearDown(self):
        self.temp.cleanup()

    def capture(self):
        with contextlib.redirect_stdout(io.StringIO()):
            result = audit.capture(self.previous, self.output, [str(self.root)], self.evidence)
        c = sqlite3.connect(self.output)
        c.row_factory = sqlite3.Row
        self.addCleanup(c.close)
        return c, result

    def add_rule(self, path, pid=None, priority=100):
        self.evidence.add(str(path), pid or self.pid, pid or self.pid,
                          'Existing reviewed ownership', priority, review_id=12)

    def test_prior_ownership_is_preserved_without_new_permission(self):
        child = self.root / 'unclear-name'
        child.mkdir()
        (child / 'data').write_bytes(b'needed source')
        self.add_rule(child)
        before = hashlib.sha256(self.previous.read_bytes()).digest()
        c, summary = self.capture()
        row = c.execute('SELECT * FROM items WHERE path=?', (str(child),)).fetchone()
        self.assertEqual(row['project_id'], self.pid)
        self.assertEqual(row['source_review_id'], 12)
        self.assertEqual(row['cleanup_authorized'], 0)
        self.assertEqual(summary['cleanup_authority'], 'NONE')
        self.assertEqual(hashlib.sha256(self.previous.read_bytes()).digest(), before)
        self.assertTrue((child / 'data').is_file())

    def test_most_specific_child_assignment_splits_parent(self):
        parent = self.root / 'mixed'
        parent.mkdir()
        other = parent / 'other'
        other.mkdir()
        (other / 'keep').write_bytes(b'other project')
        second = 'github.com/example/second'
        self.evidence.projects[second] = {
            'project_id': second, 'primary_root': None, 'identity_basis': 'Fixture',
        }
        self.add_rule(parent)
        self.add_rule(other, second)
        c, _ = self.capture()
        self.assertEqual(c.execute('SELECT project_id FROM items WHERE path=?',
                                  (str(other),)).fetchone()[0], second)
        self.assertEqual(c.execute('SELECT project_id FROM items WHERE path=?',
                                  (str(parent),)).fetchone()[0], self.pid)

    def test_conflicting_claims_remain_unresolved(self):
        child = self.root / 'conflict'
        child.mkdir()
        self.add_rule(child)
        self.add_rule(child, 'github.com/example/other')
        c, _ = self.capture()
        row = c.execute('SELECT project_id,bucket FROM items WHERE path=?',
                        (str(child),)).fetchone()
        self.assertIsNone(row['project_id'])
        self.assertEqual(row['bucket'], 'ownership-conflict')

    def test_excluded_paths_are_not_statted_or_stored(self):
        excluded = self.root / 'personal'
        excluded.mkdir()
        (excluded / 'private').write_bytes(b'not audit input')
        self.policy.write_text(json.dumps({'version': 1, 'paths': [
            {'path': str(excluded), 'reason': 'Owner exclusion'},
        ]}))
        self.evidence = audit.Evidence(Exclusions(self.policy))
        self.add_rule(excluded)
        actual = os.lstat

        def guarded(path, *args, **kwargs):
            self.assertFalse(str(path) == str(excluded) or str(path).startswith(str(excluded) + '/'))
            return actual(path, *args, **kwargs)

        with mock.patch.object(audit.os, 'lstat', side_effect=guarded):
            c, summary = self.capture()
        self.assertEqual(summary['excluded_subtrees_skipped'], 1)
        self.assertEqual(c.execute('SELECT count(*) FROM items WHERE path LIKE ?',
                                  (str(excluded) + '%',)).fetchone()[0], 0)
        self.assertEqual(c.execute('SELECT count(*) FROM prior_evidence').fetchone()[0], 0)

    def test_symlink_is_not_followed(self):
        outside = self.base / 'outside'
        outside.mkdir()
        (outside / 'private').write_bytes(b'x' * 8192)
        link = self.root / 'link'
        link.symlink_to(outside, target_is_directory=True)
        c, _ = self.capture()
        row = c.execute('SELECT * FROM items WHERE path=?', (str(link),)).fetchone()
        self.assertEqual(row['kind'], 'symlink')
        self.assertEqual(row['inclusive_files'], 1)
        self.assertEqual(c.execute('SELECT count(*) FROM items WHERE path LIKE ?',
                                  (str(link) + '/%',)).fetchone()[0], 0)

    def test_hardlinks_and_nested_units_are_not_double_counted(self):
        child = self.root / 'child'
        child.mkdir()
        file = child / 'original'
        file.write_bytes(b'x' * 16384)
        os.link(file, child / 'copy')
        expected = os.lstat(self.root).st_blocks * 512 + os.lstat(child).st_blocks * 512
        expected += os.lstat(file).st_blocks * 512
        c, _ = self.capture()
        exclusive = c.execute('SELECT sum(exclusive_allocated_bytes) FROM items').fetchone()[0]
        inclusive = c.execute('SELECT inclusive_allocated_bytes FROM items WHERE path=?',
                              (str(self.root),)).fetchone()[0]
        self.assertEqual(exclusive, expected)
        self.assertEqual(inclusive, expected)

    def test_permission_error_is_unknown_not_zero(self):
        child = self.root / 'unreadable'
        child.mkdir()
        actual = os.scandir

        def guarded(path):
            if str(path) == str(child):
                raise PermissionError('Fixture denied')
            return actual(path)

        with mock.patch.object(audit.os, 'scandir', side_effect=guarded):
            c, summary = self.capture()
        row = c.execute('SELECT * FROM items WHERE path=?', (str(child),)).fetchone()
        self.assertEqual(row['state'], 'UNREADABLE')
        self.assertIsNone(row['inclusive_allocated_bytes'])
        self.assertEqual(summary['scan_errors'], 1)

    def test_lstat_failure_keeps_an_unknown_inventory_row(self):
        child = self.root / 'unknown'
        child.mkdir()
        actual = os.lstat

        def guarded(path, *args, **kwargs):
            if str(path) == str(child):
                raise PermissionError('Fixture stat denied')
            return actual(path, *args, **kwargs)

        with mock.patch.object(audit.os, 'lstat', side_effect=guarded):
            c, _ = self.capture()
        row = c.execute('SELECT * FROM items WHERE path=?', (str(child),)).fetchone()
        self.assertEqual(row['state'], 'UNREADABLE')
        self.assertIsNone(row['inclusive_allocated_bytes'])

    def test_nested_mount_is_not_entered(self):
        child = self.root / 'mount'
        child.mkdir()
        retained = child / 'not-entered'
        retained.write_bytes(b'foreign volume')
        actual = os.lstat

        def mounted(path, *args, **kwargs):
            value = actual(path, *args, **kwargs)
            if str(path) != str(child):
                return value
            return types.SimpleNamespace(**{k: getattr(value, k) for k in (
                'st_mode', 'st_blocks', 'st_nlink', 'st_ino', 'st_mtime_ns')}, st_dev=value.st_dev + 1)

        with mock.patch.object(audit.os, 'lstat', side_effect=mounted):
            c, _ = self.capture()
        row = c.execute('SELECT * FROM items WHERE path=?', (str(child),)).fetchone()
        self.assertEqual(row['state'], 'MOUNT_SKIPPED')
        self.assertIsNone(row['inclusive_allocated_bytes'])
        self.assertEqual(c.execute('SELECT count(*) FROM items WHERE path=?',
                                  (str(retained),)).fetchone()[0], 0)
        self.assertEqual(retained.read_bytes(), b'foreign volume')

    def test_absent_old_path_does_not_count_as_current_storage(self):
        missing = self.root / 'gone'
        self.add_rule(missing)
        c, _ = self.capture()
        row = c.execute('SELECT * FROM items WHERE path=?', (str(missing),)).fetchone()
        self.assertEqual(row['state'], 'ABSENT')
        self.assertEqual(row['exclusive_allocated_bytes'], 0)
        self.assertEqual(row['cleanup_authorized'], 0)

    def test_existing_output_is_never_replaced(self):
        self.output.write_bytes(b'existing audit')
        with self.assertRaises(ValueError):
            self.capture()
        self.assertEqual(self.output.read_bytes(), b'existing audit')

    def test_git_origin_and_borrowed_checkout_ownership_are_distinct(self):
        repo = self.root / 'checkout'
        (repo / '.git').mkdir(parents=True)
        (repo / '.git/config').write_text('[remote "origin"]\n url = git@github.com:other/source.git\n')
        self.add_rule(repo)
        c, _ = self.capture()
        self.assertEqual(c.execute('SELECT project_id FROM items WHERE path=?',
                                  (str(repo),)).fetchone()[0], self.pid)

    def test_fresh_git_identity_resolves_old_unknown(self):
        repo = self.root / 'new-project'
        (repo / '.git').mkdir(parents=True)
        (repo / '.git/config').write_text('[remote "origin"]\n url = github-spence:spence/new.git\n')
        self.evidence.add(str(repo), None, 'unattributed', 'Dated unknown', 10)
        c, _ = self.capture()
        self.assertEqual(c.execute('SELECT project_id FROM items WHERE path=?',
                                  (str(repo),)).fetchone()[0], 'github.com/spence/new')

    def test_repository_identity_does_not_store_url_credentials(self):
        self.assertEqual(audit.repository_key('https://user:secret@github.com/spence/reap.git'),
                         'github.com/spence/reap')

    def test_source_reviews_supply_ownership_but_not_cleanup_authority(self):
        source = self.base / 'source.sqlite'
        c = sqlite3.connect(source)
        c.executescript('''
          CREATE TABLE projects (project_id,primary_root,known_roots_json,identity_basis);
          CREATE TABLE project_aliases (inventory_key,project_id);
          CREATE TABLE attribution (host,path,root_project,confidence);
          CREATE TABLE kytos_top_level (host,path,root_project,confidence);
          CREATE TABLE quarantine_by_project (host,id,payload_path,root_project);
          CREATE TABLE agent_reviews (id,root_project,review_json);
          CREATE VIEW current_project_reviews AS
            SELECT id,root_project AS canonical_project_id FROM agent_reviews;
          CREATE TABLE handoff_index (id);
          CREATE TABLE project_totals (id);
          CREATE TABLE unknown_and_shared (id);
          CREATE TABLE project_packets (id);
          CREATE TABLE audit_documents (id);
        ''')
        path = self.root / 'prior-claim'
        path.mkdir()
        c.execute('INSERT INTO projects VALUES (?,?,?,?)',
                  (self.pid, str(self.root), json.dumps([str(self.root)]), 'Fixture'))
        c.execute('INSERT INTO project_aliases VALUES (?,?)', (self.pid, self.pid))
        review = {'ownership_claims': [{'path': str(path), 'host': audit.HOST,
                   'ownership_confirmed': True, 'claimed_root_project': self.pid}],
                  'decisions': [{'path': str(path), 'decision': 'DELETE',
                                 'cleanup_authorized': True, 'cleanup_paths': [str(path)]}]}
        c.execute('INSERT INTO agent_reviews VALUES (?,?,?)', (12, self.pid, json.dumps(review)))
        c.commit()
        c.close()
        before = hashlib.sha256(source.read_bytes()).digest()
        with contextlib.redirect_stdout(io.StringIO()):
            self.evidence.load(source)
        c, _ = self.capture()
        self.assertEqual(c.execute('SELECT project_id FROM items WHERE path=?',
                                  (str(path),)).fetchone()[0], self.pid)
        self.assertEqual(c.execute('SELECT sum(cleanup_authorized) FROM items').fetchone()[0], 0)
        self.assertEqual(hashlib.sha256(source.read_bytes()).digest(), before)


if __name__ == '__main__':
    unittest.main()
