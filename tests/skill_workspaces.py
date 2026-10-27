"""Exercise documented workspace lifecycles with a real Reap binary and private state."""

import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path


REAP = os.environ.get('REAP_BIN') or shutil.which('reap')
GIT_GLOBAL = os.environ.get('GIT_CONFIG_GLOBAL', str(Path.home() / '.gitconfig'))
CONTENT = 'retained source fixture\n'


class WorkspaceLifecycle(unittest.TestCase):
    def setUp(self):
        if not REAP:
            self.skipTest('Set REAP_BIN or install reap before running native workflow checks')
        self.temporary = tempfile.TemporaryDirectory(prefix='reap-skill-workspace-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.home = self.root / 'home'
        self.projects = self.home / 'projects'
        self.workspace = self.projects / 'fixture/workspace'
        self.source = self.root / 'source'
        self.state = self.root / 'state/reap'
        self.env = dict(os.environ, HOME=str(self.home), XDG_STATE_HOME=str(self.root / 'state'),
                        GIT_CONFIG_GLOBAL=GIT_GLOBAL, GIT_OPTIONAL_LOCKS='0')
        for key in ('REAP_OWNER', 'REAP_SESSION', 'GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE'):
            self.env.pop(key, None)
        (self.home / '.config/reap').mkdir(parents=True)
        self.workspace.parent.mkdir(parents=True)
        (self.state / 'quarantine').mkdir(parents=True)
        config = {'reap_file_grace_hours': 0, 'quarantine': {'auto_purge': False}}
        (self.home / '.config/reap/config.json').write_text(json.dumps(config))
        for field in ('user.name', 'user.email'):
            identity = self.run_command(['git', 'config', '--get', field], check=False)
            if identity.returncode != 0:
                self.skipTest('Existing Git authorship configuration is required; never overridden')
        self.run_command(['git', 'init', '-q', '--bare', str(self.root / 'remote.git')])
        self.run_command(['git', 'init', '-q', '--initial-branch=main', str(self.source)])
        (self.source / 'source.txt').write_text(CONTENT)
        self.git(self.source, 'add', 'source.txt')
        self.git(self.source, '-c', 'commit.gpgsign=false', 'commit', '-qm',
                 'test: workspace source fixture')
        self.git(self.source, 'remote', 'add', 'origin', str(self.root / 'remote.git'))
        self.git(self.source, 'push', '-qu', 'origin', 'main')
        self.git(self.source, 'worktree', 'add', '-qb', 'review', str(self.workspace), 'main')
        self.git(self.workspace, 'branch', '--set-upstream-to=origin/main', 'review')
        exclude = Path(self.git(self.workspace, 'rev-parse', '--path-format=absolute',
                                '--git-path', 'info/exclude').stdout.strip())
        with exclude.open('a') as stream:
            stream.write('\n/.reap\n')
        self.declare(keep={'reason': 'interactive workspace awaiting human closeout'})
        self.assertEqual(self.git(self.workspace, 'check-ignore', '.reap').stdout.strip(), '.reap')
        self.assertEqual(self.git(self.workspace, 'status', '--porcelain').stdout, '')

    def run_command(self, args, check=True):
        result = subprocess.run(args, cwd=self.root, env=self.env, capture_output=True, text=True)
        if check:
            self.assertEqual(result.returncode, 0, (args, result.stdout, result.stderr))
        return result

    def git(self, path, *args):
        return self.run_command(['git', '-c', 'core.hooksPath=/dev/null', '-C', str(path), *args])

    def reap(self, *args, check=True):
        if args and args[0] == 'files':
            args = (*args, '--wait', '30')
        result = self.run_command([REAP, *map(str, args)], check=check)
        if args and args[0] == 'files':
            receipt = json.loads(result.stdout)
            self.assertEqual(receipt['protocol'], 2)
            self.assertIn(receipt['state'], ('completed', 'failed'))
            detail = json.loads(receipt['job']['result']['detail'])
            result.stdout, result.stderr = detail['stdout'], detail['stderr']
        return result

    def declare(self, **lifetime):
        body = {'version': 1, 'owner': 'workspace-fixture', 'purpose': 'isolated native test',
                'disposition': 'quarantine', **lifetime}
        (self.workspace / '.reap').write_text(json.dumps(body))

    def age(self):
        past = time.time() - 7200
        for directory, children, files in os.walk(self.workspace):
            for name in children + files:
                os.utime(Path(directory) / name, (past, past), follow_symlinks=False)
            os.utime(directory, (past, past), follow_symlinks=False)

    def test_declaration_keeps_active_work_then_quarantines_clean_work_and_restores(self):
        self.age()
        self.reap('files', '--apply', self.projects)
        self.assertTrue(self.workspace.exists())
        self.declare(expires='2020-01-01T00:00:00Z')
        (self.workspace / 'source.txt').write_text('uncommitted work\n')
        self.age()
        blocked = self.reap('files', '--apply', self.projects)
        self.assertIn('uncommitted changes', blocked.stdout + blocked.stderr)
        self.assertEqual((self.workspace / 'source.txt').read_text(), 'uncommitted work\n')
        (self.workspace / 'source.txt').write_text(CONTENT)
        self.age()
        result = self.reap('files', '--apply', self.projects)
        self.assertFalse(self.workspace.exists(), result.stdout + result.stderr)
        index = json.loads((self.state / 'quarantine/index.json').read_text())
        self.assertEqual(len(index['entries']), 1)
        entry = index['entries'][0]
        self.assertEqual(entry['original_path'], str(self.workspace))
        payload = self.state / 'quarantine/entries' / entry['id'] / entry['name']
        self.assertEqual((payload / 'source.txt').read_text(), CONTENT)
        self.reap('quarantine', 'restore', entry['id'])
        self.assertEqual((self.workspace / 'source.txt').read_text(), CONTENT)
        self.assertEqual(self.git(self.workspace, 'status', '--porcelain').stdout, '')

    def test_mixed_lease_and_declaration_remain_blocked(self):
        self.reap('lease', 'add', self.workspace, '--ttl', '1h', '--owner', 'workspace-fixture',
                  '--purpose', 'isolated overlap test', '--project', self.source)
        self.declare(expires='2020-01-01T00:00:00Z')
        self.age()
        declared = self.reap('files', '--apply', self.projects)
        self.assertIn('overlaps lease', declared.stdout + declared.stderr)
        retired = self.reap('retire', self.workspace, '--now', '--apply', check=False)
        self.assertIn('ignored path', retired.stdout + retired.stderr)
        self.assertTrue(self.workspace.exists())
        self.assertEqual((self.workspace / 'source.txt').read_text(), CONTENT)

    def test_normal_lease_quarantines_approved_cargo_output_and_preserves_ignored_notes(self):
        (self.workspace / '.reap').unlink()
        (self.workspace / 'Cargo.toml').write_text(
            '[package]\nname = "workspace-fixture"\nversion = "0.1.0"\n')
        (self.workspace / '.gitignore').write_text('target/\nlocal-notes/\n')
        self.git(self.workspace, 'add', 'Cargo.toml', '.gitignore')
        self.git(self.workspace, '-c', 'commit.gpgsign=false', 'commit', '-qm',
                 'test: disposable Cargo fixture')
        self.git(self.workspace, 'push', '-q', 'origin', 'HEAD:main')
        target = self.workspace / 'target'
        (target / 'debug').mkdir(parents=True)
        (target / 'CACHEDIR.TAG').write_text('Signature: 8a477f597d28d172789f06886806bc55\n')
        executable = target / 'debug/workspace-fixture'
        executable.write_bytes(b'retained executable')
        self.reap('lease', 'add', self.workspace, '--ttl', '0', '--owner', 'workspace-fixture',
                  '--purpose', 'Cargo retirement fixture', '--project', self.source)
        notes = self.workspace / 'local-notes/evidence.txt'
        notes.parent.mkdir()
        notes.write_bytes(b'required local evidence')
        self.age()
        command = ('retire', self.workspace, '--disposable-cargo-target', 'target')
        blocked = self.reap(*command, '--apply', check=False)
        self.assertNotEqual(blocked.returncode, 0)
        self.assertIn('git-ignored', blocked.stdout)
        self.assertEqual(notes.read_bytes(), b'required local evidence')
        self.assertEqual(executable.read_bytes(), b'retained executable')
        notes.unlink()
        notes.parent.rmdir()
        self.age()
        plan = self.reap(*command)
        self.assertIn(str(target), plan.stdout)
        self.assertTrue(executable.exists())
        self.assertFalse((self.state / 'quarantine/index.json').exists())
        self.reap(*command, '--apply')
        self.assertFalse(self.workspace.exists())
        index = json.loads((self.state / 'quarantine/index.json').read_text())
        self.assertEqual(len(index['entries']), 1)
        entry = index['entries'][0]
        self.assertFalse(entry['scratch'])
        payload = self.state / 'quarantine/entries' / entry['id'] / entry['name']
        self.assertEqual((payload / 'target/debug/workspace-fixture').read_bytes(),
                         b'retained executable')
        self.assertEqual((payload / 'source.txt').read_text(), CONTENT)

    def test_declaration_preserves_commits_on_a_noncurrent_branch(self):
        self.declare(expires='2020-01-01T00:00:00Z')
        self.git(self.workspace, 'checkout', '-qb', 'unpublished')
        (self.workspace / 'branch-only.txt').write_text('unpublished source\n')
        self.git(self.workspace, 'add', 'branch-only.txt')
        self.git(self.workspace, '-c', 'commit.gpgsign=false', 'commit', '-qm',
                 'test: unpublished branch fixture')
        self.git(self.workspace, 'checkout', '-q', 'review')
        self.age()
        result = self.reap('files', '--apply', self.projects)
        self.assertTrue(self.workspace.exists(), result.stdout + result.stderr)
        self.assertIn('commit', result.stdout)
        self.assertEqual(self.git(self.workspace, 'show', 'unpublished:branch-only.txt').stdout,
                         'unpublished source\n')

    def test_declaration_preserves_ignored_local_data(self):
        self.declare(expires='2020-01-01T00:00:00Z')
        exclude = Path(self.git(self.workspace, 'rev-parse', '--path-format=absolute',
                                '--git-path', 'info/exclude').stdout.strip())
        with exclude.open('a') as stream:
            stream.write('/local-notes.txt\n')
        notes = self.workspace / 'local-notes.txt'
        notes.write_text('required local evidence\n')
        self.age()
        result = self.reap('files', '--apply', self.projects)
        self.assertTrue(notes.exists(), result.stdout + result.stderr)
        self.assertEqual(notes.read_text(), 'required local evidence\n')
        self.assertIn('ignored path', result.stdout)


if __name__ == '__main__':
    unittest.main(verbosity=2)
