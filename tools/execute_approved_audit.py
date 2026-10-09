"""Execute the Mini audit's current exact approvals, preserving guard failures in SQLite."""

import argparse
import bisect
import collections
import contextlib
import datetime
import fcntl
import hashlib
import json
import os
import re
import shutil
import socket
import sqlite3
import stat
import subprocess
import time
from pathlib import Path


BASE = Path('/Users/spencer/src/reap/.reap-audits/2026-10-07-mini')
STATE = Path('/Users/spencer/.local/state/reap')
QUARANTINE = Path('/Volumes/kytos/reap-quarantine')
REAP = '/Users/spencer/.cargo/bin/reap'
HOST = 'catalyst-mini.local'
ACTOR = 'Codex /root SQLite-approved cleanup executor'
PROTECTED = (
    '/Volumes/kytos/src/nextaskai-parity-718bd48',
    '/Volumes/kytos/mcp-durability-m2-v4-2c17186f',
)
IDENTITIES = (
    'identity_at_closeout', 'identity_at_followup', 'current_identity',
    'expected_identity', 'identity_to_recheck', 'observed_identity',
    'identity_at_review', 'measured_evidence',
)
ACTIVITY_FAILURE = None
OWNED_PENDING_PROBES = []


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def emit(kind, **values):
    print(json.dumps({'event': kind, **values}), flush=True)


def below(path, parent):
    return path == parent or path.startswith(parent.rstrip('/') + '/')


def overlap(a, b):
    return below(a, b) or below(b, a)


def completed(item):
    return item.get('cleanup_completed') is True or str(item.get('execution_status', '')).startswith('DELETED')


def authorized(item, decision):
    return item.get('decision') == decision and item.get('cleanup_authorized') is True and not completed(item)


def metadata(path):
    s = os.lstat(path)
    return dict(device=s.st_dev, inode=s.st_ino, mode=s.st_mode,
                size_bytes=s.st_size, mtime_ns=s.st_mtime_ns)


def check_identity(path, expected, content=True):
    actual = metadata(path)
    aliases = {'dev': 'device', 'ino': 'inode', 'size': 'size_bytes'}
    for key, value in expected.items():
        key = aliases.get(key, key)
        if key in ('device', 'inode') or content and key in ('size_bytes', 'mtime_ns'):
            if value is not None and int(value) != actual[key]:
                raise ValueError(f'changed {key}: {path}')
    return actual


def expected_identity(item):
    return next((item[k] for k in IDENTITIES if isinstance(item.get(k), dict)), {})


def parse_plan(output, target):
    target_line = re.search(r'^    target : (.+)$', output, re.M)
    total = re.search(r'^    TOTAL\s+(\d+) items', output, re.M)
    paths = [line[8:] for line in output.splitlines() if line.startswith('        /')]
    if not target_line or target_line[1] != str(target):
        raise ValueError('native planner selected a different target')
    if 'nothing reclaimable' in output:
        if paths:
            raise ValueError('inconsistent empty native plan')
        return set()
    if not total or int(total[1]) != len(paths) or len(paths) != len(set(paths)):
        raise ValueError('native verbose plan is incomplete or ambiguous')
    for path in paths:
        if not below(path, str(target)) or path == str(target):
            raise ValueError('native candidate escapes target')
    return set(paths)


@contextlib.contextmanager
def lock_file(path):
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        opened, current = os.fstat(fd), os.lstat(path)
        if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
            raise ValueError(f'unsafe lock identity: {path}')
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)


def cargo_profiles(target, approved_paths=None):
    if approved_paths is not None:
        profiles = set()
        for path in approved_paths:
            relative = str(Path(path).relative_to(target))
            match = re.search(r'^(.*?)(debug|release)/(deps|\.fingerprint|build|incremental)/', relative)
            if not match:
                raise ValueError(f'not a native Cargo artifact path: {path}')
            profile = target / (match[1] + match[2])
            if os.path.realpath(profile) != str(profile) or not stat.S_ISDIR(profile.lstat().st_mode) or profile.lstat().st_dev != target.lstat().st_dev:
                raise ValueError(f'unsafe selected Cargo profile: {profile}')
            profiles.add(profile)
        return sorted(profiles)
    out = []
    for parent in [target] + [x for x in target.iterdir() if stat.S_ISDIR(x.lstat().st_mode)]:
        for name in ('debug', 'release'):
            profile = parent / name
            if profile.exists():
                m = profile.lstat()
                if not stat.S_ISDIR(m.st_mode) or m.st_dev != target.lstat().st_dev:
                    raise ValueError(f'unsafe Cargo profile: {profile}')
                out.append(profile)
    return out


def cargo_target(item):
    target = item.get('cargo_target') or item.get('cargo_plan', {}).get('target') or item.get('native_plan', {}).get('target')
    if target:
        return Path(target)
    paths = item.get('cleanup_paths', [])
    if not paths:
        return None
    match = re.search(r'/(debug|release)/(deps|\.fingerprint|build|incremental)/', paths[0])
    if not match:
        return None
    start = Path(paths[0][:match.start()])
    for parent in [start] + list(start.parents):
        marker = parent / 'CACHEDIR.TAG'
        if marker.is_file():
            return parent
    manifest = start.parent / '.reap.json'
    if manifest.is_file():
        with manifest.open() as f:
            declared = json.load(f)
        if os.path.normpath(str(start.parent / declared.get('target', 'target'))) == str(start):
            return start
    return None


def native_command(items, target):
    commands = {tuple(x.get('cargo_plan', {}).get('command', [])) for x in items}
    commands.discard(())
    if len(commands) > 1:
        raise ValueError('inconsistent approved native planning contexts')
    if commands:
        command = list(commands.pop())
        if len(command) < 3 or command[1] != 'plan':
            raise ValueError('approved context is not a native dry-run plan')
        source, options = command[2], command[3:]
    else:
        source = str(target) if (target / 'CACHEDIR.TAG').is_file() else str(target.parent)
        options = []
    result = [REAP, 'plan', source]
    valued = {'--keep-recent', '--min-age-minutes', '--stale-debug', '--stale-release'}
    flags = {'--no-incremental', '--no-build-scripts'}
    i = 0
    while i < len(options):
        flag = options[i]
        if flag in ('--verbose', '--quick'):
            i += 1
        elif flag in flags:
            result.append(flag)
            i += 1
        elif flag in valued and i + 1 < len(options):
            value = options[i + 1]
            if float(value) < 0 or flag == '--min-age-minutes' and float(value) < 10:
                raise ValueError('approved command relaxes the minimum safety floor')
            result.extend([flag, value])
            i += 2
        else:
            raise ValueError(f'unsupported approved planning option: {flag}')
    return result + ['--quick', '--verbose']


def remove_path(path):
    if not stat.S_ISDIR(os.lstat(path).st_mode):
        os.unlink(path)
        return
    stack = [path]
    while stack:
        directory = stack.pop()
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            s = os.fstat(fd)
            if not s.st_mode & stat.S_IWUSR:
                if s.st_uid != os.geteuid() or getattr(s, 'st_flags', 0):
                    raise ValueError(f'cannot change foreign or flagged directory permissions: {directory}')
                os.fchmod(fd, s.st_mode | stat.S_IWUSR)
            with os.scandir(fd) as children:
                stack.extend(os.path.join(directory, child.name) for child in children if child.is_dir(follow_symlinks=False))
        finally:
            os.close(fd)
    shutil.rmtree(path)


def tree_guard(path, minimum_age=600, allow_git_pointer=None):
    root = os.lstat(path)
    newest = root.st_mtime
    todo = [path] if stat.S_ISDIR(root.st_mode) else []
    while todo:
        directory = todo.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                s = entry.stat(follow_symlinks=False)
                if s.st_dev != root.st_dev:
                    raise ValueError(f'nested mount: {entry.path}')
                if entry.name == '.git' and allow_git_pointer and stat.S_ISREG(s.st_mode):
                    text = Path(entry.path).read_text().strip()
                    if not text.startswith('gitdir: '):
                        raise ValueError(f'invalid Git pointer: {entry.path}')
                    gitdir = os.path.normpath(os.path.join(directory, text[8:]))
                    if not below(gitdir, allow_git_pointer):
                        raise ValueError(f'Git metadata is not retained in the approved root: {entry.path}')
                    continue
                if entry.name in ('.reap-lease', '.reap-parent', '.reap', '.git'):
                    raise ValueError(f'protected marker: {entry.path}')
                if not (stat.S_ISDIR(s.st_mode) or stat.S_ISREG(s.st_mode) or stat.S_ISLNK(s.st_mode)):
                    raise ValueError(f'special file: {entry.path}')
                newest = max(newest, s.st_mtime)
                if stat.S_ISDIR(s.st_mode):
                    todo.append(entry.path)
    if time.time() - newest < minimum_age:
        raise ValueError(f'not quiet for ten minutes: {path}')
    return metadata(path)


def verify_quarantine_source(item):
    recovery = item.get('recoverability', {})
    if not isinstance(recovery, dict) or not recovery.get('preserve_this_ref_and_git_root'):
        return None, None
    common, revision, ref = (recovery[k] for k in ('git_common_root', 'git_revision', 'git_ref'))
    if not os.path.isdir(common) or os.path.realpath(common) != common:
        raise ValueError('retained Git object store is unavailable')
    env = {**os.environ, 'GIT_OPTIONAL_LOCKS': '0'}
    prefix = ['/usr/bin/git', '--git-dir', common]
    def git(args):
        result = subprocess.run(prefix + args, capture_output=True, timeout=15, env=env)
        if result.returncode:
            raise ValueError(f'retained source check failed: {result.stderr[:200]!r}')
        return result.stdout
    current = git(['rev-parse', '--verify', ref + '^{commit}']).decode().strip()
    git(['merge-base', '--is-ancestor', revision, current])
    data = git(['ls-tree', '-r', '-z', revision])
    slot = Path(item['payload_path'])
    candidates = [slot]
    checkout = slot / 'checkout'
    if checkout.is_dir():
        candidates.insert(0, checkout)
    files = []
    for row in data.split(b'\0'):
        if not row:
            continue
        header, name = row.split(b'\t', 1)
        mode, kind, oid = header.decode().split()
        if kind != 'blob':
            raise ValueError('quarantine source includes an unsupported Git object')
        files.append((mode, oid, os.fsdecode(name)))
    root = next((r for r in candidates if files and os.path.lexists(r / files[0][2])), None)
    if root is None:
        raise ValueError('cannot locate the recorded source tree in the payload')
    tracked = set()
    for mode, oid, name in files:
        path = root / name
        s = path.lstat()
        if mode == '120000' and stat.S_ISLNK(s.st_mode):
            contents = os.fsencode(os.readlink(path))
        elif mode in ('100644', '100755') and stat.S_ISREG(s.st_mode) and os.path.realpath(path.parent) == str(path.parent):
            contents = path.read_bytes()
        else:
            raise ValueError(f'tracked source changed type: {path}')
        blob = b'blob ' + str(len(contents)).encode() + b'\0' + contents
        actual = hashlib.sha256(blob).hexdigest() if len(oid) == 64 else hashlib.sha1(blob).hexdigest()
        if actual != oid:
            raise ValueError(f'tracked source differs from retained commit: {path}')
        tracked.add(str(path))
    for directory, subdirs, names in os.walk(root, followlinks=False):
        subdirs[:] = [d for d in subdirs if d not in ('node_modules', 'dist', '.git') and not os.path.islink(os.path.join(directory, d))]
        for name in names:
            path = os.path.join(directory, name)
            if path in tracked or name in ('.git', '.DS_Store') or name.endswith('.tsbuildinfo') or os.path.islink(path):
                continue
            raise ValueError(f'non-generated extra file in quarantined source: {path}')
    return common, {'retained_ref': ref, 'retained_revision': revision, 'tracked_files_verified': len(files),
                    'git_common_root_preserved': common}


def check_activity(path):
    global ACTIVITY_FAILURE
    if ACTIVITY_FAILURE:
        raise ValueError(ACTIVITY_FAILURE)
    health = subprocess.run(['/bin/ps', '-axo', 'state=,etime=,comm='], capture_output=True, text=True, timeout=5)
    health.check_returncode()
    for line in health.stdout.splitlines():
        fields = line.split()
        if len(fields) == 3 and 'U' in fields[0] and fields[2].endswith('/lsof'):
            raise ValueError('lsof is kernel-blocked; no new activity probe started')
    command = ['/usr/sbin/lsof', '-n', '-P', '-F0n']
    command += ['+D', path] if os.path.isdir(path) else ['--', path]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        out, err = process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            process.communicate(timeout=0.25)
        except subprocess.TimeoutExpired:
            OWNED_PENDING_PROBES.append(process.pid)
        ACTIVITY_FAILURE = 'scoped activity probe timed out; further probes refused'
        raise ValueError(ACTIVITY_FAILURE)
    if err or process.returncode not in (0, 1):
        ACTIVITY_FAILURE = f'activity probe failed: {path}: {err[:180]!r}'
        raise ValueError(ACTIVITY_FAILURE)
    if process.returncode == 0 or out:
        raise ValueError(f'open handles: {path}')


class Cleanup:
    def __init__(self, database, apply):
        self.apply = apply
        self.c = sqlite3.connect(f'file:{database}?mode={"rw" if apply else "ro"}', uri=True, timeout=20)
        self.c.row_factory = sqlite3.Row
        self.reviews = {}
        self.keep = set()
        self.claims = collections.defaultdict(list)
        self.primaries = [r[0] for r in self.c.execute('SELECT primary_root FROM projects') if r[0]]
        for row in self.c.execute('SELECT * FROM current_project_reviews ORDER BY id'):
            project = row['canonical_project_id'] or row['root_project']
            j = json.loads(row['review_json'])
            self.reviews[project] = (row['id'], j)
            for item in j.get('decisions', []):
                if item.get('decision') == 'KEEP':
                    self.keep.add(item['path'])
            for claim in j.get('ownership_claims', []):
                if claim.get('ownership_confirmed') is True and claim.get('host', j.get('host')) == HOST:
                    self.claims[claim['path']].append((project, claim.get('include_descendants') is True))
        self.keep = sorted(self.keep)
        self.results = {}
        self.run_ids = {}
        self.refresh_leases()

    def refresh_leases(self):
        with (STATE / 'leases.json').open() as f:
            leases = json.load(f)
        self.pending = [x['path'] for x in leases.get('creating', []) + leases.get('parents', [])]
        self.lease_all = sorted({x['path'] for x in leases['leases']})
        self.lease_active = sorted({x['path'] for x in leases['leases'] if x['expires_unix'] > time.time()})
        self.lease_sets = {False: set(self.lease_all), True: set(self.lease_active)}

    def current(self, project):
        row = self.c.execute('SELECT MAX(r.id) FROM agent_reviews r LEFT JOIN project_aliases a '
                             'ON a.inventory_key=r.root_project WHERE COALESCE(a.project_id,r.root_project)=?',
                             (project,)).fetchone()
        if not row or row[0] != self.reviews[project][0]:
            raise ValueError(f'approval superseded: {project}')

    def event(self, project, path, action, status, facts):
        emit('scope', project=project, path=path, action=action, status=status,
             counts=facts.get('counts'), reason=facts.get('reason'))
        if self.apply:
            self.c.execute('INSERT INTO cleanup_execution_events(run_id,path,action,status,receipt_json) VALUES(?,?,?,?,?)',
                           (self.run_ids[project], path, action, status, json.dumps(facts)))
            self.c.commit()

    def path_guard(self, path, project, cargo=False):
        if not path.startswith(('/Volumes/kytos/', '/Users/spencer/', '/private/tmp/')) or str(Path(path)) != path:
            raise ValueError(f'out-of-scope path: {path}')
        if os.path.realpath(path) != path or stat.S_ISLNK(os.lstat(path).st_mode):
            raise ValueError(f'symlinked path: {path}')
        if any(overlap(path, p) for p in PROTECTED) or any(below(p, path) for p in self.primaries):
            raise ValueError(f'protected repository overlap: {path}')
        ix = bisect.bisect_left(self.keep, path)
        if ix < len(self.keep) and below(self.keep[ix], path):
            raise ValueError(f'current KEEP overlap: {self.keep[ix]}')
        for parent in [path] + [str(x) for x in Path(path).parents]:
            owners = {p for p, descendants in self.claims.get(parent, []) if parent == path or descendants}
            if owners:
                if owners != {project}:
                    raise ValueError(f'conflicting owner claim: {path}')
                break
        for pending in self.pending:
            if overlap(path, pending):
                raise ValueError(f'pending creation or managed parent: {pending}')
        paths = self.lease_active if cargo else self.lease_all
        ix = bisect.bisect_left(paths, path)
        if ix < len(paths) and below(paths[ix], path):
            raise ValueError(f'lease overlap: {paths[ix]}')
        for parent in [path] + [str(x) for x in Path(path).parents]:
            if parent in self.lease_sets[cargo]:
                raise ValueError(f'lease overlap: {parent}')
        for parent in [Path(path)] + list(Path(path).parents):
            decl = parent / '.reap'
            if decl.exists():
                raise ValueError(f'lifetime declaration requires native review: {decl}')

    def delete(self, project, path, expected):
        self.path_guard(path, project, cargo=True)
        before = check_identity(path, expected)
        tree_guard(path)
        if self.apply:
            check_identity(path, before)
            remove_path(path)
            if os.path.lexists(path):
                raise ValueError(f'path survived deletion: {path}')
        self.results[path] = {'status': 'DELETED_VERIFIED' if self.apply else 'ELIGIBLE', 'identity': before}

    def cargo(self, project, target, items):
        allowed = {}
        for item in items:
            cp = item['cleanup_paths']
            identities = item.get('cleanup_identity_records', [])
            if identities and len(identities) != len(cp):
                raise ValueError('unaligned approved identity records')
            for ix, path in enumerate(cp):
                identity = dict(zip(item['cleanup_identity_columns'], identities[ix])) if identities else expected_identity(item) if path == item['path'] else {}
                allowed[path] = identity
        try:
            self.current(project)
            self.refresh_leases()
            if not target.is_dir():
                raise ValueError('Cargo target is absent')
            for item in items:
                if os.path.lexists(item['path']):
                    check_identity(item['path'], expected_identity(item), content=False)
            with contextlib.ExitStack() as stack:
                for profile in cargo_profiles(target, allowed):
                    stack.enter_context(lock_file(profile / '.cargo-lock'))
                before = {p: metadata(p) for p in allowed if os.path.lexists(p)}
                command = native_command(items, target)
                result = subprocess.run(command, capture_output=True, text=True, timeout=60)
                if result.returncode or result.stderr:
                    raise ValueError(f'native plan failed: {result.stderr[:350]}')
                eligible = parse_plan(result.stdout, target)
                chosen = sorted(eligible.intersection(allowed))
                self.event(project, str(target), 'cargo', 'PLANNED', {
                    'exact_selected_paths': chosen, 'approved_count': len(allowed),
                    'unapproved_native_candidates_preserved': len(eligible.difference(allowed)),
                    'profile_locks_held': True,
                })
                for path in allowed:
                    if path not in before:
                        self.results[path] = {'status': 'ABSENT_BEFORE_EXECUTION'}
                    elif path not in eligible:
                        self.results[path] = {'status': 'BLOCKED', 'reason': 'not eligible in fresh native plan'}
                progress_at = time.monotonic()
                for ix, path in enumerate(chosen):
                    if ix % 2048 == 0:
                        self.current(project)
                    if time.monotonic() - progress_at > 20:
                        emit('cargo_progress', project=project, target=str(target), checked=ix, selected=len(chosen))
                        progress_at = time.monotonic()
                    try:
                        check_identity(path, allowed[path])
                        self.delete(project, path, before[path])
                    except (OSError, ValueError) as e:
                        self.results[path] = {'status': 'BLOCKED', 'reason': str(e)}
                counts = dict(collections.Counter(self.results[p]['status'] for p in allowed))
                self.event(project, str(target), 'cargo', 'VERIFIED', {
                    'counts': counts, 'exact_results': {p: self.results[p] for p in allowed},
                    'profile_locks_held': True,
                })
        except (OSError, ValueError, subprocess.TimeoutExpired) as e:
            for path in allowed:
                self.results.setdefault(path, {'status': 'BLOCKED', 'reason': str(e)})
            self.event(project, str(target), 'cargo', 'BLOCKED', {'reason': str(e), 'count': len(allowed)})

    def direct(self, project, item):
        for path in item['cleanup_paths']:
            if not os.path.lexists(path):
                with (QUARANTINE / 'index.json').open() as f:
                    moved = [x['id'] for x in json.load(f)['entries'] if x['original_path'] == path]
                if moved:
                    self.results[path] = {'status': 'BLOCKED', 'reason': f'source moved to quarantine; reconcile exact entry {moved} before permanent purge'}
                else:
                    self.results[path] = {'status': 'ABSENT_BEFORE_EXECUTION'}
                self.event(project, path, 'direct', self.results[path]['status'], self.results[path])
                continue
            try:
                with lock_file(STATE / 'state.lock'):
                    self.current(project)
                    self.refresh_leases()
                    self.path_guard(path, project)
                    check_identity(path, expected_identity(item))
                    for retained, identity in item.get('content_verification', {}).get('identities', {}).items():
                        check_identity(retained, identity)
                    for retained in item.get('retained_paths', []):
                        if retained != path and not below(retained, path) and not os.path.lexists(retained):
                            raise ValueError(f'retained recovery path absent: {retained}')
                    before = tree_guard(path)
                    check_activity(path)
                    self.event(project, path, 'direct', 'PLANNED', {'identity': before})
                    check_identity(path, before)
                    if self.apply:
                        remove_path(path)
                        if os.path.lexists(path):
                            raise ValueError('path survived deletion')
                    self.results[path] = {'status': 'DELETED_VERIFIED' if self.apply else 'ELIGIBLE', 'identity': before}
                    self.event(project, path, 'direct', self.results[path]['status'], self.results[path])
            except (OSError, ValueError, subprocess.TimeoutExpired) as e:
                self.results[path] = {'status': 'BLOCKED', 'reason': str(e)}
                self.event(project, path, 'direct', 'BLOCKED', self.results[path])

    def purge(self, project, item):
        entry_id = item.get('quarantine_entry_id') or item.get('id')
        payload = item['payload_path']
        slot = str(QUARANTINE / 'entries' / entry_id)
        try:
            self.current(project)
            with (QUARANTINE / 'index.json').open() as f:
                index = json.load(f)
            if entry_id in index.get('held_ids', []):
                raise ValueError('quarantine entry is held')
            entries = [x for x in index['entries'] if x['id'] == entry_id]
            if not os.path.lexists(slot):
                self.results[payload] = {'status': 'ABSENT_BEFORE_EXECUTION'}
                return
            if len(entries) != 1 or str(QUARANTINE / 'entries' / entry_id / entries[0]['name']) != payload:
                raise ValueError('quarantine index/payload mismatch')
            if any(overlap(entries[0]['original_path'], p) for p in PROTECTED):
                raise ValueError('protected original path')
            git_common, recovery_receipt = verify_quarantine_source(item)
            before = tree_guard(slot, allow_git_pointer=git_common)
            check_activity(slot)
            diagnosis = subprocess.run([REAP, 'doctor', '--quarantine', '--id', entry_id], capture_output=True, text=True, timeout=40)
            if diagnosis.returncode or 'blocked' in diagnosis.stdout.lower() and not re.search(r'blocked 0', diagnosis.stdout):
                raise ValueError(f'quarantine diagnosis failed: {diagnosis.stdout[:400]} {diagnosis.stderr[:200]}')
            self.event(project, slot, 'purge', 'PLANNED', {'entry_id': entry_id, 'identity': before,
                                                       'native_diagnosis': diagnosis.stdout, 'recoverability': recovery_receipt})
            check_identity(slot, before)
            if self.apply:
                result = subprocess.run([REAP, 'purge', '--id', entry_id, '--apply'], capture_output=True, text=True, timeout=60)
                if result.returncode or os.path.lexists(slot):
                    raise ValueError(f'native purge failed: {result.stdout[:250]} {result.stderr[:250]}')
            self.results[payload] = {'status': 'DELETED_VERIFIED' if self.apply else 'ELIGIBLE'}
            source = item.get('source_approval_path')
            if source and self.results.get(source, {}).get('reason', '').startswith('source moved to quarantine') and not os.path.lexists(source):
                self.results[source] = {'status': self.results[payload]['status'], 'quarantine_entry_id': entry_id}
            self.event(project, slot, 'purge', self.results[payload]['status'], self.results[payload])
        except (OSError, ValueError, subprocess.TimeoutExpired) as e:
            self.results[payload] = {'status': 'BLOCKED', 'reason': str(e)}
            self.event(project, slot, 'purge', 'BLOCKED', self.results[payload])

    def reconcile(self, project, items, quarantine):
        counts = collections.Counter()
        for item in items + quarantine:
            paths = [item['payload_path']] if item in quarantine else item['cleanup_paths']
            pending = [p for p in paths if self.results[p]['status'] not in ('DELETED_VERIFIED', 'ABSENT_BEFORE_EXECUTION')]
            counts.update(self.results[p]['status'] for p in paths)
            item['approved_cleanup_execution'] = {'run_id': self.run_ids[project], 'checked_at_utc': now(),
                                                  'counts': dict(collections.Counter(self.results[p]['status'] for p in paths))}
            if not pending:
                item['cleanup_authorized'] = False
                item['cleanup_completed'] = True
                item['execution_status'] = 'DELETED_VERIFIED' if any(self.results[p]['status'] == 'DELETED_VERIFIED' for p in paths) else 'ABSENT_BEFORE_EXECUTION'
            else:
                item['execution_status'] = 'BLOCKED_NATIVE_GUARD'
                item['execution_blockers'] = [{'path': p, 'reason': self.results[p].get('reason', 'guard blocked')} for p in pending]
                if item not in quarantine:
                    if item.get('cleanup_identity_records'):
                        pending_set = set(pending)
                        item['cleanup_identity_records'] = [identity for p, identity in zip(paths, item['cleanup_identity_records']) if p in pending_set]
                    item['cleanup_paths'] = pending
        self.c.execute('BEGIN IMMEDIATE')
        try:
            self.current(project)
            source, review = self.reviews[project]
            receipt = {'source_review_id': source, 'counts': dict(counts), 'completed_at_utc': now(),
                       'scope': 'Exact current approvals only; blocked scopes preserved; completed approvals withdrawn.',
                       'foreign_processes_preserved': True}
            review['approved_cleanup_execution'] = {'run_id': self.run_ids[project], **receipt}
            summary = review.setdefault('summary', {})
            summary['approved_cleanup_execution'] = {'run_id': self.run_ids[project], **receipt}
            summary['execution_status'] = 'PARTIAL_GUARD_BLOCKED' if counts['BLOCKED'] else 'COMPLETED_VERIFIED'
            if 'current_approved_cleanup_paths' in summary:
                summary['current_approved_cleanup_paths'] = sum(len(x.get('cleanup_paths', [])) for x in items if authorized(x, 'DELETE'))
            if 'current_approved_native_cargo_artifacts' in summary:
                summary['current_approved_native_cargo_artifacts'] = sum(len(x.get('cleanup_paths', [])) for x in items if authorized(x, 'DELETE') and cargo_target(x))
            review['actor'] = ACTOR
            review['session'] = 'UNKNOWN'
            row = self.c.execute('INSERT INTO agent_reviews(root_project,actor,session,review_json) VALUES(?,?,?,?)',
                                 (project, ACTOR, 'UNKNOWN', json.dumps(review)))
            receipt['replacement_review_id'] = row.lastrowid
            self.c.execute('UPDATE cleanup_execution_runs SET finished_at=CURRENT_TIMESTAMP,receipt_json=? WHERE id=?',
                           (json.dumps(receipt), self.run_ids[project]))
            self.c.commit()
            self.reviews[project] = (row.lastrowid, review)
            emit('project_complete', project=project, **receipt)
        except BaseException:
            self.c.rollback()
            raise

    def run(self):
        order = sorted(self.reviews, key=lambda p: p != 'github.com/spence/micro')
        for project in order:
            source, review = self.reviews[project]
            items = [x for x in review.get('decisions', []) if authorized(x, 'DELETE') and x.get('host', review.get('host')) == HOST]
            quarantine = [x for x in review.get('quarantine_decisions', []) if authorized(x, 'PURGE_NOW') and x.get('host', review.get('host')) == HOST]
            if not items and not quarantine:
                continue
            if self.apply:
                self.current(project)
                row = self.c.execute('INSERT INTO cleanup_execution_runs(root_project,actor,session,source_review_id,receipt_json) VALUES(?,?,?,?,?)',
                                     (project, ACTOR, 'UNKNOWN', source, json.dumps({'owner_instruction_verbatim': 'lets clean up whats been approved', 'started_at_utc': now()})))
                self.run_ids[project] = row.lastrowid
                self.c.commit()
            jobs = collections.defaultdict(list)
            direct = []
            for item in items:
                target = cargo_target(item)
                if target:
                    jobs[target].append(item)
                else:
                    direct.append(item)
            for target, subset in jobs.items():
                self.cargo(project, target, subset)
            for item in direct:
                self.direct(project, item)
            for item in quarantine:
                self.purge(project, item)
            if self.apply:
                self.reconcile(project, items, quarantine)
        emit('finished', apply=self.apply, counts=dict(collections.Counter(x['status'] for x in self.results.values())),
             owned_pending_activity_probe_pids=OWNED_PENDING_PROBES)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    if socket.gethostname() != HOST or not os.path.ismount('/Volumes/kytos'):
        raise SystemExit('This executor requires catalyst-mini.local with Kytos mounted.')
    Cleanup(BASE / 'audit.sqlite', args.apply).run()
