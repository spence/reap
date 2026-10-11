import importlib.util
import json
import os
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools'))
import audit_exclusions as audit

SPEC = importlib.util.spec_from_file_location('cleanup_exclusion_test', ROOT / 'tools/execute_approved_audit.py')
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)


class ExclusionTests(unittest.TestCase):
    def setUp(self):
        self.policy = audit.Exclusions()
        self.personal = '/Users/spencer/Documents/micro'

    def database(self):
        c = sqlite3.connect(':memory:')
        self.addCleanup(c.close)
        c.row_factory = sqlite3.Row
        c.executescript('''
            CREATE TABLE attribution(host,path,root_project,exclusive_allocated_gib_estimate);
            CREATE TABLE project_totals(root_project,allocated_gib_estimate,unit_count);
            CREATE TABLE handoff_index(root_project,allocated_gib_estimate_not_reclaimable);
            CREATE TABLE project_packets(root_project,json);
            CREATE TABLE agent_reviews(id INTEGER PRIMARY KEY,root_project,review_json);
            CREATE TABLE cleanup_execution_events(id INTEGER PRIMARY KEY,path,receipt_json);
        ''')
        retained = {'path': '/Volumes/kytos/other', 'decision': 'DELETE',
                    'cleanup_authorized': True, 'cleanup_paths': ['/Volumes/kytos/other'],
                    'current_exists': True}
        personal = {'path': self.personal, 'decision': 'KEEP', 'cleanup_authorized': False,
                    'current_exists': True}
        summary = {'completion_boundaries': {'coverage': {'current_project_items': 2},
                   'accounting': {'status': 'PENDING_CENTRAL_CORRECTIONS', 'pending_exact_paths': [self.personal]}},
                   'ownership_corrections': [{'path': self.personal}],
                   'current_live_decision_counts': {'KEEP': 1, 'DELETE': 1},
                   'delivery': {'remaining_review_exceptions': 'Source packages pending execution, and three central personal/shared accounting corrections are unchanged.'}}
        review = {'decisions': [personal, retained], 'ownership_claims': [],
                  'quarantine_decisions': [], 'summary': summary,
                  'history': {'by_path': {self.personal: personal},
                              'description': 'Previously assigned ' + self.personal + '.'}}
        for path, size in ((self.personal, '0.3'), ('/Volumes/kytos/other', '1.0')):
            c.execute('INSERT INTO attribution VALUES(?,?,?,?)', ('catalyst-mini.local', path, 'micro', size))
        c.execute('INSERT INTO project_totals VALUES(?,?,?)', ('micro', '1.3', '2'))
        c.execute('INSERT INTO handoff_index VALUES(?,?)', ('micro', '1.3'))
        c.execute('INSERT INTO project_packets VALUES(?,?)', ('micro', json.dumps({'units': [personal, retained]})))
        c.execute('INSERT INTO agent_reviews VALUES(?,?,?)', (1, 'micro', json.dumps(review)))
        c.execute('INSERT INTO cleanup_execution_events VALUES(?,?,?)', (1, '/Volumes/kytos/other', '{"status":"DELETED_VERIFIED"}'))
        c.commit()
        return c, retained

    def test_path_boundaries(self):
        self.assertEqual(len(self.policy.paths), 3)
        for path in self.policy.paths:
            self.assertTrue(self.policy.contains(path))
            self.assertTrue(self.policy.contains(path + '/nested/file'))
            self.assertFalse(self.policy.contains(path + '-project'))
            self.assertFalse(self.policy.contains(path + '2'))
        self.assertFalse(self.policy.contains('/Volumes/kytos/src/micro'))
        self.assertTrue(self.policy.overlaps('/Users/spencer/Documents'))
        self.assertFalse(self.policy.overlaps('/Users/spencer/Documents/other'))
        sibling = self.personal + '.backup'
        self.assertEqual(self.policy.clean(sibling), sibling)
        self.assertNotIn(self.personal, self.policy.clean('Previously assigned ' + self.personal + '.'))

    def test_database_reconciliation(self):
        c, retained = self.database()
        receipt = tuple(c.execute('SELECT * FROM cleanup_execution_events').fetchone())
        with c:
            result = audit.reconcile(c, self.policy, apply=True)
        self.assertEqual(result['inventory_rows']['attribution'], 1)
        self.assertEqual(c.execute('SELECT path FROM attribution').fetchone()[0], retained['path'])
        raw = c.execute('SELECT review_json FROM agent_reviews').fetchone()[0]
        self.assertNotIn(self.personal, raw)
        review = json.loads(raw)
        self.assertEqual(review['decisions'], [retained])
        self.assertEqual(review['summary']['completion_boundaries']['accounting']['status'], 'COMPLETE')
        self.assertEqual(review['summary']['completion_boundaries']['coverage']['current_project_items'], 1)
        self.assertEqual(review['summary']['delivery']['remaining_review_exceptions'], 'Source packages pending execution.')
        self.assertEqual(tuple(c.execute('SELECT * FROM cleanup_execution_events').fetchone()), receipt)
        self.assertEqual(c.execute('SELECT allocated_gib_estimate,unit_count FROM project_totals').fetchone()[0], '1.0')
        self.assertEqual(c.execute('SELECT unit_count FROM project_totals').fetchone()[0], 1)
        self.assertEqual(json.loads(c.execute('SELECT json FROM project_packets').fetchone()[0])['units'], [retained])

    def test_dry_run_and_directory_survival(self):
        c, _ = self.database()
        before = list(c.iterdump())
        audit.reconcile(c, self.policy)
        self.assertEqual(list(c.iterdump()), before)
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory) / 'kept'
            p.write_text('personal data')
            original = p.stat().st_ino
            policy = Path(directory) / 'policy.json'
            policy.write_text(json.dumps({'version': 1, 'paths': [{'path': str(p), 'reason': 'test fixture'}]}))
            self.personal = str(p)
            self.policy = audit.Exclusions(policy)
            c, _ = self.database()
            with c:
                audit.reconcile(c, self.policy, apply=True)
            self.assertEqual(p.read_text(), 'personal data')
            self.assertEqual(p.stat().st_ino, original)

    def test_permission_change_rolls_back(self):
        c, _ = self.database()
        review = json.loads(c.execute('SELECT review_json FROM agent_reviews').fetchone()[0])
        review['decisions'][1]['cleanup_paths'].append(self.personal + '/data')
        c.execute('UPDATE agent_reviews SET review_json=?', (json.dumps(review),))
        c.commit()
        before = list(c.iterdump())
        with self.assertRaisesRegex(ValueError, 'unrelated cleanup authority'):
            with c:
                audit.reconcile(c, self.policy, apply=True)
        self.assertEqual(list(c.iterdump()), before)

    def test_cleanup_executor_respects_exclusions(self):
        executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
        executor.primaries, executor.keep, executor.claims, executor.pending = [], [], {}, []
        executor.lease_active, executor.lease_all = [], []
        executor.lease_sets = {True: set(), False: set()}
        with patch.object(cleanup.os.path, 'realpath', side_effect=lambda p: p), \
             patch.object(cleanup.os, 'lstat', return_value=Mock(st_mode=0o100644)), \
             patch.object(cleanup.Path, 'exists', return_value=False):
            for path in (self.personal, self.personal + '/data', '/Users/spencer/Documents'):
                with self.assertRaisesRegex(ValueError, 'audit-excluded directory overlap'):
                    executor.path_guard(path, 'micro', cargo=True)
            executor.path_guard('/Volumes/kytos/src/micro/target/debug/deps/old', 'micro', cargo=True)
        with patch.object(cleanup.os.path, 'realpath', return_value=self.personal + '/data'):
            with self.assertRaisesRegex(ValueError, 'audit-excluded directory overlap'):
                executor.path_guard('/Volumes/kytos/alias', 'micro')

    def test_malformed_policy_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory) / 'policy.json'
            for data in ({'version': 2, 'paths': []}, {'version': 1, 'paths': [{'path': '/', 'reason': 'bad'}]},
                         {'version': 1, 'paths': [{'path': 'relative', 'reason': 'bad'}]}):
                p.write_text(json.dumps(data))
                with self.assertRaises(ValueError):
                    audit.Exclusions(p)


if __name__ == '__main__':
    unittest.main()
