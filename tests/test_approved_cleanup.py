import importlib.util
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock, patch


sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'tools'))
SPEC = importlib.util.spec_from_file_location('approved_cleanup', Path(__file__).resolve().parents[1] / 'tools/execute_approved_audit.py')
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)


class CleanupTests(unittest.TestCase):
    def compiler_executor(self):
        executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
        executor.apply, executor.results = True, {}
        executor.current, executor.refresh_leases = Mock(), Mock()
        executor.path_guard, executor.event = Mock(), Mock()
        return executor

    def compiler_approval(self, profile, paths):
        columns = ['device', 'inode', 'mode', 'size_bytes', 'mtime_ns']
        return {'path': str(profile), 'current_identity': cleanup.metadata(profile),
                'cleanup_paths': [str(p) for p in paths], 'cleanup_identity_columns': columns,
                'cleanup_identity_records': [[cleanup.metadata(p)[k] for k in columns] for p in paths]}

    def test_exact_compiler_cleanup_preserves_unlisted_files_directories_and_link_targets(self):
        with tempfile.TemporaryDirectory() as root:
            profile = (Path(root) / 'target/release').resolve()
            profile.mkdir(parents=True)
            selected, retained = profile / 'old.o', profile / 'keep.o'
            selected.write_bytes(b'obsolete')
            retained.write_bytes(b'needed')
            link = profile / 'obsolete-link'
            link.symlink_to(retained)
            for p in (selected, link):
                os.utime(p, (time.time() - 7200, time.time() - 7200), follow_symlinks=False)
            executor = self.compiler_executor()
            with patch.object(cleanup, 'check_activity') as activity:
                executor.compiler('example', profile, [self.compiler_approval(profile, [selected, link])])
            activity.assert_called_once_with(str(profile), recursive=False)
            self.assertFalse(selected.exists())
            self.assertFalse(link.is_symlink())
            self.assertEqual(retained.read_bytes(), b'needed')
            self.assertTrue(profile.is_dir())
            self.assertTrue((profile / '.cargo-lock').is_file())
            self.assertEqual(executor.results[str(selected)]['status'], 'DELETED_VERIFIED')

    def test_exact_compiler_cleanup_preserves_changed_recent_directory_and_lock_selectors(self):
        with tempfile.TemporaryDirectory() as root:
            profile = (Path(root) / 'release').resolve()
            profile.mkdir()
            changed, recent, lock, directory = [profile / n for n in ('changed.o', 'recent.o', '.cargo-lock', 'directory')]
            for p in (changed, recent, lock):
                p.write_text('original')
            directory.mkdir()
            for p in (changed, lock, directory):
                os.utime(p, (time.time() - 7200, time.time() - 7200))
            item = self.compiler_approval(profile, [changed, recent, lock, directory])
            changed.write_text('new content')
            executor = self.compiler_executor()
            with patch.object(cleanup, 'check_activity'):
                executor.compiler('example', profile, [item])
            self.assertEqual(changed.read_text(), 'new content')
            self.assertEqual(recent.read_text(), 'original')
            self.assertTrue(lock.is_file())
            self.assertTrue(directory.is_dir())
            self.assertTrue(all(r['status'] == 'BLOCKED' for r in executor.results.values()))

    def test_exact_compiler_cleanup_refuses_foreign_locks_activity_and_stale_reviews(self):
        with tempfile.TemporaryDirectory() as root:
            profile = (Path(root) / 'release').resolve()
            profile.mkdir()
            selected = profile / 'old.o'
            selected.write_text('old')
            os.utime(selected, (time.time() - 7200, time.time() - 7200))
            item = self.compiler_approval(profile, [selected])
            executor = self.compiler_executor()
            with cleanup.lock_file(profile / '.cargo-lock'), patch.object(cleanup, 'check_activity') as activity:
                executor.compiler('example', profile, [item])
                activity.assert_not_called()
            self.assertTrue(selected.exists())
            executor = self.compiler_executor()
            with patch.object(cleanup, 'check_activity', side_effect=ValueError('open handles')):
                executor.compiler('example', profile, [item])
            self.assertTrue(selected.exists())
            executor = self.compiler_executor()
            executor.current.side_effect = ValueError('approval superseded')
            with patch.object(cleanup, 'check_activity') as activity:
                executor.compiler('example', profile, [item])
                activity.assert_not_called()
            self.assertTrue(selected.exists())
            self.assertEqual(executor.results[str(selected)]['status'], 'BLOCKED')

    def test_exact_compiler_cleanup_refuses_symlinked_profiles(self):
        with tempfile.TemporaryDirectory() as root:
            outside = (Path(root) / 'outside').resolve()
            outside.mkdir()
            selected = outside / 'keep.o'
            selected.write_text('needed')
            profile = Path(root) / 'release'
            profile.symlink_to(outside, target_is_directory=True)
            executor = self.compiler_executor()
            with patch.object(cleanup, 'check_activity') as activity:
                executor.compiler('example', profile, [self.compiler_approval(profile, [profile / 'keep.o'])])
                activity.assert_not_called()
            self.assertEqual(selected.read_text(), 'needed')
            self.assertEqual(executor.results[str(profile / 'keep.o')]['status'], 'BLOCKED')

    def test_global_activity_probe_filters_paths_but_refuses_in_profile_handles(self):
        profile = '/Volumes/kytos/cold/target/release'
        process = Mock(returncode=0)
        process.communicate.return_value = (b'p123\0\nn/outside/active\0\nn' + os.fsencode(profile + '/.cargo-lock') + b'\0', b'')
        health = Mock(returncode=0, stdout='')
        with patch.object(cleanup, 'ACTIVITY_FAILURE', None), \
             patch.object(cleanup.subprocess, 'run', return_value=health), \
             patch.object(cleanup.subprocess, 'Popen', return_value=process) as spawn:
            cleanup.check_activity(profile, recursive=False)
            self.assertEqual(spawn.call_args.args[0], ['/usr/sbin/lsof', '-n', '-P', '-F0n'])
            process.communicate.return_value = (b'p123\0\nn' + os.fsencode(profile + '/deps/in-use') + b'\0', b'')
            with self.assertRaisesRegex(ValueError, 'open handles'):
                cleanup.check_activity(profile, recursive=False)
            process.kill.assert_not_called()

    def test_preservation_requires_matching_live_identity_and_content(self):
        with tempfile.TemporaryDirectory() as root:
            archive = (Path(root) / 'capsule.tar').resolve()
            archive.write_bytes(b'retained source and proof')
            proof = {'status': 'PRESERVATION_COMPLETED_AND_INDEPENDENTLY_VERIFIED',
                     'destination': str(archive), 'destination_identity': cleanup.metadata(archive),
                     'capsule_sha256': hashlib.sha256(archive.read_bytes()).hexdigest()}
            item = {'preservation_verified': proof}
            cleanup.verify_preservation(item)
            proof['capsule_sha256'] = '0' * 64
            with self.assertRaisesRegex(ValueError, 'digest changed'):
                cleanup.verify_preservation(item)
            archive.write_bytes(b'different')
            with self.assertRaisesRegex(ValueError, 'changed'):
                cleanup.verify_preservation(item)
            self.assertEqual(archive.read_bytes(), b'different')

    def test_completed_receipt_is_preserved_and_recreated_path_is_never_replayed(self):
        with tempfile.TemporaryDirectory() as root:
            path = str(Path(root) / 'original')
            item = {'cleanup_paths': [path]}
            executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
            executor.results = {path: {'status': 'DELETED_VERIFIED', 'identity': {'inode': 123}}}
            executor.event = Mock()
            executor.direct('example', item)
            self.assertEqual(executor.results[path]['status'], 'DELETED_VERIFIED')
            Path(path).write_text('new work')
            executor.direct('example', item)
            self.assertEqual(executor.results[path]['status'], 'BLOCKED')
            self.assertEqual(Path(path).read_text(), 'new work')

    def test_indexed_current_native_options_are_used_without_changing_approvals(self):
        target = Path('/source/target')
        plans = [{'command': [cleanup.REAP, 'plan', '/source', '--quick', '--verbose', '--stale-debug', '14']}]
        item = {'native_plan_numbers': [0], 'cleanup_paths': ['/source/target/debug/deps/old']}
        self.assertEqual(cleanup.native_command([item], target, plans),
                         [cleanup.REAP, 'plan', '/source', '--stale-debug', '14', '--quick', '--verbose'])
        self.assertNotIn('cargo_plan', item)
        with self.assertRaisesRegex(ValueError, 'ambiguous'):
            cleanup.native_command([{**item, 'native_plan_numbers': [7]}], target, plans)

    def test_unguarded_compiler_paths_never_fall_back_to_direct_deletion(self):
        item = {'host': cleanup.HOST, 'path': '/compiler/group', 'decision': 'DELETE',
                'cleanup_authorized': True, 'cleanup_paths': ['/compiler/old.o'],
                'cleanup_scope': 'EXACT_EXISTING_LISTED_COMPILER_FILES_AND_SYMLINK_ENTRIES_ONLY'}
        executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
        executor.apply, executor.results = False, {}
        executor.reviews = {'example': (1, {'decisions': [item]})}
        executor.direct = Mock()
        with patch.object(cleanup, 'cargo_target', return_value=None), patch.object(cleanup, 'emit'):
            executor.run()
        executor.direct.assert_not_called()
        self.assertEqual(executor.results['/compiler/old.o']['status'], 'BLOCKED')

    def test_project_and_direct_phase_filter_preserve_other_approvals(self):
        direct = {'host': cleanup.HOST, 'path': '/export', 'decision': 'DELETE',
                  'cleanup_authorized': True, 'cleanup_paths': ['/export']}
        compiler = {**direct, 'path': '/compiler', 'cleanup_scope': 'EXACT_EXISTING_LISTED_COMPILER_FILES_ONLY'}
        other = {**direct, 'path': '/other'}
        executor = cleanup.Cleanup.__new__(cleanup.Cleanup)
        executor.apply, executor.results = False, {}
        executor.reviews = {'example': (1, {'decisions': [direct, compiler]}),
                            'other': (2, {'decisions': [other]})}
        executor.direct = Mock()
        with patch.object(cleanup, 'cargo_target', return_value=None), patch.object(cleanup, 'emit'):
            executor.run('example', 'direct')
        executor.direct.assert_called_once_with('example', direct)
        self.assertTrue(compiler['cleanup_authorized'])
        self.assertTrue(other['cleanup_authorized'])

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
