import importlib.util
import json
import os
import sqlite3
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location('approved_cleanup', Path(__file__).resolve().parents[1] / 'tools/execute_approved_audit.py')
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)


class CleanupTests(unittest.TestCase):
    def test_only_current_explicit_uncompleted_approval_is_authority(self):
        item = {'decision': 'DELETE', 'cleanup_authorized': True}
        self.assertTrue(cleanup.authorized(item, 'DELETE'))
        for change in ({'cleanup_authorized': False}, {'cleanup_authorized': 'true'},
                       {'decision': 'KEEP'}, {'cleanup_completed': True},
                       {'execution_status': 'DELETED_VERIFIED'}):
            self.assertFalse(cleanup.authorized({**item, **change}, 'DELETE'))

    def test_native_plan_parser_never_authorizes_unlisted_path(self):
        target = Path('/Volumes/kytos/example/target')
        a, b = str(target / 'debug/deps/old'), str(target / 'debug/deps/other')
        output = f'    target : {target}\n        {a}\n        {b}\n    TOTAL 2 items\n'
        native = cleanup.parse_plan(output, target)
        self.assertEqual(native.intersection({a}), {a})
        self.assertNotIn(b, native.intersection({a}))

    def test_native_plan_rejects_ambiguous_or_escaping_output(self):
        target = Path('/Volumes/kytos/example/target')
        for output in (
            '    target : /wrong\n    TOTAL 0 items\n',
            f'    target : {target}\n        {target}/debug/deps/old\n    TOTAL 2 items\n',
            f'    target : {target}\n        /outside\n    TOTAL 1 items\n',
        ):
            with self.assertRaises(ValueError):
                cleanup.parse_plan(output, target)

    def test_identity_change_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            p = Path(root) / 'data'
            p.write_text('old')
            before = cleanup.metadata(p)
            p.write_text('changed')
            with self.assertRaises(ValueError):
                cleanup.check_identity(p, before)
            self.assertEqual(p.read_text(), 'changed')

    def test_selected_deletion_removes_only_the_selected_old_file(self):
        with tempfile.TemporaryDirectory() as root:
            selected, retained = Path(root) / 'selected', Path(root) / 'retained'
            selected.write_text('old')
            retained.write_text('keep')
            os.utime(selected, (time.time() - 7200, time.time() - 7200))
            executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
            executor.apply, executor.results = True, {}
            executor.path_guard = Mock()
            executor.delete('example', str(selected), cleanup.metadata(selected))
            self.assertFalse(selected.exists())
            self.assertEqual(retained.read_text(), 'keep')
            self.assertEqual(executor.results[str(selected)]['status'], 'DELETED_VERIFIED')

    def test_nonblocking_profile_lock_never_replaces_or_steals_a_foreign_lock(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / '.cargo-lock'
            with cleanup.lock_file(path):
                identity = cleanup.metadata(path)
                with self.assertRaises(BlockingIOError):
                    with cleanup.lock_file(path):
                        self.fail('foreign lock was stolen')
                self.assertEqual(cleanup.metadata(path)['inode'], identity['inode'])

    def test_approved_stale_profile_options_and_source_context_are_preserved(self):
        item = {'cargo_plan': {'command': [cleanup.REAP, 'plan', '/source', '--verbose', '--stale-debug', '30']}}
        command = cleanup.native_command([item], Path('/source/target'))
        self.assertEqual(command, [cleanup.REAP, 'plan', '/source', '--stale-debug', '30', '--quick', '--verbose'])
        with self.assertRaises(ValueError):
            cleanup.native_command([{'cargo_plan': {'command': [cleanup.REAP, 'clean', '/source']}}], Path('/source/target'))

    def test_unselected_symlinked_profile_is_untouched(self):
        with tempfile.TemporaryDirectory() as root:
            target = (Path(root) / 'target').resolve()
            (target / 'debug/deps').mkdir(parents=True)
            other = Path(root) / 'other'
            other.mkdir()
            (target / 'release').symlink_to(other, target_is_directory=True)
            selected = str(target / 'debug/deps/old')
            self.assertEqual(cleanup.cargo_profiles(target, [selected]), [target / 'debug'])
            self.assertTrue((target / 'release').is_symlink())
            with self.assertRaises(ValueError):
                cleanup.cargo_profiles(target, [str(target / 'release/deps/old')])

    def test_profile_batches_keep_exact_scope_identity_alignment(self):
        target = Path('/Volumes/kytos/source/target')
        item = {'cleanup_paths': [str(target / 'debug/deps/old'), str(target / 'release/deps/old')],
                'cleanup_identity_records': [['debug-identity'], ['release-identity']], 'cleanup_identity_columns': ['inode']}
        batches = cleanup.profile_batches(target, [item])
        self.assertEqual(len(batches), 2)
        self.assertEqual(batches[0][0]['cleanup_identity_records'], [['debug-identity']])
        self.assertEqual(batches[1][0]['cleanup_identity_records'], [['release-identity']])
        self.assertEqual(len(item['cleanup_paths']), 2)

    def test_readonly_approved_tree_is_removed_without_following_links(self):
        with tempfile.TemporaryDirectory() as root:
            selected, retained = Path(root) / 'selected', Path(root) / 'retained'
            (selected / 'nested').mkdir(parents=True)
            retained.write_text('keep')
            (selected / 'nested/data').write_text('old')
            (selected / 'link').symlink_to(retained)
            os.chmod(selected / 'nested', 0o555)
            os.chmod(selected, 0o555)
            cleanup.remove_path(str(selected))
            self.assertFalse(selected.exists())
            self.assertEqual(retained.read_text(), 'keep')

    def test_tree_guard_preserves_markers_and_fresh_data(self):
        with tempfile.TemporaryDirectory() as root:
            p = Path(root) / 'data'
            p.mkdir()
            (p / '.reap-lease').write_text('{}')
            with self.assertRaisesRegex(ValueError, 'protected marker'):
                cleanup.tree_guard(p)
            (p / '.reap-lease').unlink()
            with self.assertRaisesRegex(ValueError, 'not quiet'):
                cleanup.tree_guard(p)
            self.assertTrue(p.exists())

    def test_only_proven_retained_git_pointer_can_pass_quarantine_guard(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            slot, common = root / 'slot', root / 'retained.git'
            slot.mkdir()
            common.mkdir()
            pointer = slot / '.git'
            pointer.write_text('gitdir: ' + str(common / 'worktrees/retired'))
            for p in (slot, pointer):
                os.utime(p, (time.time() - 7200, time.time() - 7200))
            with self.assertRaises(ValueError):
                cleanup.tree_guard(str(slot))
            cleanup.tree_guard(str(slot), allow_git_pointer=str(common))
            with self.assertRaisesRegex(ValueError, 'not retained'):
                cleanup.tree_guard(str(slot), allow_git_pointer=str(root / 'other'))
            self.assertTrue(pointer.exists())

    def test_workspace_lease_does_not_override_exact_cargo_cleanup_authority(self):
        executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
        root = '/Volumes/kytos/leased-workspace'
        artifact = root + '/target/debug/deps/old'
        executor.primaries, executor.keep, executor.claims, executor.pending = [], [], {}, []
        executor.lease_active, executor.lease_all = [root], [root]
        executor.lease_sets = {True: {root}, False: {root}}
        with patch.object(cleanup.os.path, 'realpath', side_effect=lambda p: p), \
             patch.object(cleanup.os, 'lstat', return_value=Mock(st_mode=0o100644)), \
             patch.object(cleanup.Path, 'exists', return_value=False):
            executor.path_guard(artifact, 'example', cargo=True)
            with self.assertRaisesRegex(ValueError, 'lease overlap'):
                executor.path_guard(artifact, 'example', cargo=False)
            executor.lease_active = [artifact]
            executor.lease_sets[True] = {artifact}
            with self.assertRaisesRegex(ValueError, 'lease overlap'):
                executor.path_guard(artifact, 'example', cargo=True)

    def test_tree_guard_does_not_follow_symlinks(self):
        with tempfile.TemporaryDirectory() as root:
            outside, p = Path(root) / 'outside', Path(root) / 'data'
            outside.mkdir()
            (outside / '.reap-lease').write_text('retain')
            p.mkdir()
            link = p / 'link'
            link.symlink_to(outside, target_is_directory=True)
            os.utime(link, (time.time() - 7200, time.time() - 7200), follow_symlinks=False)
            os.utime(p, (time.time() - 7200, time.time() - 7200))
            cleanup.tree_guard(p)
            self.assertEqual((outside / '.reap-lease').read_text(), 'retain')

    def test_probe_timeout_signals_only_owned_child_and_prevents_more_probes(self):
        process = Mock(pid=98765)
        process.communicate.side_effect = subprocess.TimeoutExpired('lsof', 5)
        health = Mock(returncode=0, stdout='')
        with patch.object(cleanup, 'ACTIVITY_FAILURE', None), patch.object(cleanup, 'OWNED_PENDING_PROBES', []), \
             patch.object(cleanup.subprocess, 'run', return_value=health), \
             patch.object(cleanup.subprocess, 'Popen', return_value=process) as spawn:
            with self.assertRaisesRegex(ValueError, 'timed out'):
                cleanup.check_activity('/example')
            process.kill.assert_called_once()
            self.assertEqual(cleanup.OWNED_PENDING_PROBES, [98765])
            with self.assertRaisesRegex(ValueError, 'timed out'):
                cleanup.check_activity('/other')
            self.assertEqual(spawn.call_count, 1)

    def test_kernel_blocked_foreign_probe_is_never_signaled_or_duplicated(self):
        health = Mock(returncode=0, stdout='U 02-01:00:00 /usr/sbin/lsof\n')
        with patch.object(cleanup, 'ACTIVITY_FAILURE', None), \
             patch.object(cleanup.subprocess, 'run', return_value=health), \
             patch.object(cleanup.subprocess, 'Popen') as spawn:
            with self.assertRaisesRegex(ValueError, 'kernel-blocked'):
                cleanup.check_activity('/example')
            spawn.assert_not_called()

    def test_reconciliation_withdraws_completed_paths_and_preserves_pending_and_history(self):
        c = sqlite3.connect(':memory:')
        c.row_factory = sqlite3.Row
        c.executescript('''
            CREATE TABLE agent_reviews(id INTEGER PRIMARY KEY, root_project, actor, session, review_json);
            CREATE TABLE project_aliases(inventory_key, project_id);
            CREATE TABLE cleanup_execution_runs(id INTEGER PRIMARY KEY, finished_at, receipt_json);
            CREATE TABLE attribution(path, owner);
            INSERT INTO cleanup_execution_runs VALUES(1,NULL,'{}');
            INSERT INTO attribution VALUES('/retained','original');
        ''')
        item = {'path': '/group', 'decision': 'DELETE', 'cleanup_authorized': True,
                'cleanup_paths': ['/removed', '/blocked']}
        done = {'path': '/done', 'decision': 'DELETE', 'cleanup_authorized': True,
                'cleanup_paths': ['/done']}
        historical = {'path': '/keep', 'decision': 'KEEP', 'cleanup_authorized': False}
        review = {'decisions': [item, done, historical], 'ownership_claims': [{'path': '/keep'}],
                  'quarantine_decisions': [], 'execution_closeout': {'run_id': 42}, 'summary': {}}
        original = json.dumps(review)
        c.execute('INSERT INTO agent_reviews VALUES(1,?,?,?,?)', ('example', 'reviewer', 'session', original))
        c.commit()
        executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
        executor.c, executor.apply = c, True
        executor.run_ids = {'example': 1}
        executor.reviews = {'example': (1, review)}
        executor.results = {'/removed': {'status': 'DELETED_VERIFIED'},
                            '/done': {'status': 'DELETED_VERIFIED'},
                            '/blocked': {'status': 'BLOCKED', 'reason': 'active build'}}
        with patch.object(cleanup, 'emit'):
            executor.reconcile('example', [item, done], [])
        current = json.loads(c.execute('SELECT review_json FROM agent_reviews ORDER BY id DESC LIMIT 1').fetchone()[0])
        self.assertEqual(current['decisions'][0]['cleanup_paths'], ['/blocked'])
        self.assertTrue(current['decisions'][0]['cleanup_authorized'])
        self.assertFalse(current['decisions'][1]['cleanup_authorized'])
        self.assertTrue(current['decisions'][1]['cleanup_completed'])
        self.assertEqual(current['decisions'][2], historical)
        self.assertEqual(current['execution_closeout'], {'run_id': 42})
        self.assertEqual(c.execute('SELECT review_json FROM agent_reviews WHERE id=1').fetchone()[0], original)
        self.assertEqual(tuple(c.execute('SELECT * FROM attribution').fetchone()), ('/retained', 'original'))


if __name__ == '__main__':
    unittest.main()
