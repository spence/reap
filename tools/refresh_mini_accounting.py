"""Capture Mini storage accounting without importing or granting cleanup authority."""

import argparse
import collections
import datetime
import hashlib
import json
import os
import socket
import sqlite3
import stat
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import unquote, urlsplit

from audit_exclusions import Exclusions


REPO = Path(__file__).resolve().parents[1]
HOST = 'catalyst-mini.local'
ROOTS = ('/Volumes/kytos', '/Users/spencer')
INVENTORY = ('attribution', 'kytos_top_level', 'handoff_index', 'project_totals',
             'quarantine_by_project', 'unknown_and_shared', 'project_packets', 'audit_documents')
METADATA_NAMES = ('.Spotlight', '.fseventsd', '.DocumentRevisions', '.Trashes',
                  '.TemporaryItems', '.DS_Store')
SCHEMA = '''
CREATE TABLE metadata (key TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE projects (project_id TEXT PRIMARY KEY, primary_root TEXT, identity_basis TEXT);
CREATE TABLE prior_evidence (
 path TEXT, project_id TEXT, bucket TEXT, basis TEXT, priority INTEGER,
 review_id INTEGER, quarantine_id TEXT
);
CREATE INDEX evidence_path ON prior_evidence(path);
CREATE TABLE items (
 path TEXT PRIMARY KEY, parent_path TEXT, scope_root TEXT, top_level_path TEXT,
 kind TEXT, state TEXT, project_id TEXT, bucket TEXT NOT NULL,
 attribution_basis TEXT, confidence TEXT, source_review_id INTEGER,
 device INTEGER, inode INTEGER, mtime_ns INTEGER, link_target TEXT,
 exclusive_allocated_bytes INTEGER NOT NULL, known_inclusive_allocated_bytes INTEGER NOT NULL,
 inclusive_allocated_bytes INTEGER, inclusive_files INTEGER, inclusive_directories INTEGER,
 quarantine_id TEXT, primary_project_root INTEGER NOT NULL,
 child_review_allowed INTEGER NOT NULL, observed_at TEXT NOT NULL,
 cleanup_authorized INTEGER NOT NULL DEFAULT 0 CHECK(cleanup_authorized = 0)
);
CREATE INDEX items_project ON items(project_id);
CREATE INDEX items_top ON items(top_level_path);
CREATE TABLE scan_errors (path TEXT, operation TEXT, error TEXT);
CREATE TABLE quarantine_entries (
 id TEXT PRIMARY KEY, original_path TEXT, payload_path TEXT, project_id TEXT, held INTEGER
);
CREATE VIEW project_totals AS
 SELECT p.project_id, p.primary_root,
 COUNT(CASE WHEN i.state = 'PRESENT' THEN 1 END) AS present_units,
 COALESCE(SUM(CASE WHEN i.quarantine_id IS NULL THEN i.exclusive_allocated_bytes ELSE 0 END),0)
   AS source_allocated_bytes,
 COALESCE(SUM(CASE WHEN i.quarantine_id IS NOT NULL THEN i.exclusive_allocated_bytes ELSE 0 END),0)
   AS quarantine_allocated_bytes,
 COALESCE(SUM(i.exclusive_allocated_bytes),0) AS allocated_bytes,
 COUNT(CASE WHEN i.state IN ('PRESENT','UNREADABLE','MOUNT_SKIPPED','NOT_SCANNED')
   AND i.inclusive_allocated_bytes IS NULL THEN 1 END) AS incomplete_size_units
 FROM projects p LEFT JOIN items i ON i.project_id = p.project_id
 GROUP BY p.project_id;
CREATE VIEW storage_buckets AS
 SELECT COALESCE(project_id,bucket) AS owner,
 COUNT(CASE WHEN state = 'PRESENT' THEN 1 END) AS present_units,
 SUM(exclusive_allocated_bytes) AS allocated_bytes,
 COUNT(CASE WHEN state IN ('PRESENT','UNREADABLE','MOUNT_SKIPPED','NOT_SCANNED')
   AND inclusive_allocated_bytes IS NULL THEN 1 END) AS incomplete_size_units
 FROM items GROUP BY COALESCE(project_id,bucket);
CREATE VIEW kytos_top_level AS
 SELECT * FROM items WHERE parent_path = '/Volumes/kytos' AND state <> 'ABSENT';
CREATE VIEW top_level_projects AS
 SELECT top_level_path, project_id, bucket, SUM(exclusive_allocated_bytes) AS allocated_bytes
 FROM items WHERE top_level_path IS NOT NULL
 GROUP BY top_level_path, project_id, bucket;
'''


class Evidence:
    def __init__(self, exclusions, aliases=None):
        self.exclusions = exclusions
        self.path_aliases = aliases or {}
        self.projects = {}
        self.aliases = {}
        self.rules = collections.defaultdict(list)
        self.quarantine = {}
        self.quarantine_claims = collections.defaultdict(set)
        self.review_ids = {}
        self.inventory_counts = {}
        self.lifecycle_info = {}

    def canonical(self, path):
        path = os.path.normpath(path)
        for source, target in self.path_aliases.items():
            if path == source or path.startswith(source + '/'):
                return target + path[len(source):]
        return path

    def add(self, path, project, bucket, basis, priority, review_id=None, quarantine_id=None):
        if not isinstance(path, str) or not os.path.isabs(path) or self.exclusions.contains(path):
            return
        path = self.canonical(path)
        if self.exclusions.contains(path):
            return
        rule = {'project_id': project, 'bucket': bucket if project is None else project,
                'basis': basis, 'priority': priority, 'review_id': review_id,
                'quarantine_id': quarantine_id}
        if rule not in self.rules[path]:
            self.rules[path].append(rule)

    def at(self, path):
        rules = self.rules.get(self.canonical(path), [])
        if not rules:
            return None
        priority = max(r['priority'] for r in rules)
        best = [r for r in rules if r['priority'] == priority]
        owners = {(r['project_id'], r['bucket']) for r in best}
        if len(owners) > 1:
            return owner(None, 'ownership-conflict', 'Conflicting existing ownership evidence', 100)
        return dict(best[0])

    def project_for(self, value):
        if not isinstance(value, str):
            return None
        if value.startswith('github.com/'):
            for alias, pid in self.aliases.items():
                if pid and alias.casefold() == value.casefold():
                    return pid
        if value in self.projects:
            return value
        if self.aliases.get(value):
            return self.aliases[value]
        if value.startswith('/'):
            rule = self.at(value)
            return rule['project_id'] if rule else None
        key = repository_key(value)
        return self.project_for(key) if key and key != value else None

    def load(self, previous):
        c = readonly(previous)
        self.projects = {r['project_id']: dict(r) for r in c.execute(
            'SELECT project_id,primary_root,known_roots_json,identity_basis FROM projects')}
        self.aliases = {r['inventory_key']: r['project_id'] for r in c.execute(
            'SELECT inventory_key,project_id FROM project_aliases')}
        self.inventory_counts = {t: c.execute('SELECT count(*) FROM ' + t).fetchone()[0]
                                 for t in INVENTORY}
        for row in c.execute('SELECT path,root_project,confidence FROM attribution '
                             'WHERE host=?', (HOST,)):
            pid = self.aliases.get(row['root_project'])
            self.add(row['path'], pid, row['root_project'],
                     'Existing dated attribution (' + row['confidence'] + ')', 10)
        for row in c.execute('SELECT path,root_project,confidence FROM kytos_top_level '
                             'WHERE host=?', (HOST,)):
            self.add(row['path'], self.aliases.get(row['root_project']), row['root_project'],
                     'Existing top-level attribution (' + row['confidence'] + ')', 10)
        for pid, project in self.projects.items():
            if project['primary_root']:
                self.add(project['primary_root'], pid, pid, 'Existing canonical primary root', 70)
            for path in json.loads(project['known_roots_json']):
                self.add(path, pid, pid, 'Existing canonical known root', 60)
        headers = list(c.execute('SELECT id,canonical_project_id FROM current_project_reviews'))
        for header in headers:
            pid = header['canonical_project_id']
            if not pid:
                continue
            rid = header['id']
            self.review_ids[pid] = rid
            raw = c.execute('SELECT review_json FROM agent_reviews WHERE id=?', (rid,)).fetchone()[0]
            review = json.loads(raw)
            del raw
            for number, claim in enumerate(review.get('ownership_claims', [])):
                if not claim.get('ownership_confirmed') or claim.get('host', HOST) != HOST:
                    continue
                claimed = claim.get('claimed_root_project', claim.get('root_project', pid))
                if self.aliases.get(claimed, claimed) != pid:
                    continue
                qid = claim.get('quarantine_entry_id')
                if qid:
                    self.quarantine_claims[qid].add(pid)
                self.add(claim.get('path'), pid, pid,
                         f'Existing confirmed ownership claim {number}, review {rid}', 100, rid, qid)
            for decision in review.get('decisions', []):
                correction = decision.get('ownership_correction', {})
                if decision.get('ownership_confirmed') is False and correction.get('correct_owner_category'):
                    self.add(decision.get('path'), None, 'shared-agent-data',
                             'Existing explicit shared-owner correction', 110, rid)
            del review
            progress({'phase': 'existing_evidence', 'project_id': pid, 'review_id': rid})
        for row in c.execute('SELECT id,payload_path,root_project FROM quarantine_by_project '
                             'WHERE host=?', (HOST,)):
            pid = self.aliases.get(row['root_project'])
            if pid:
                self.add(row['payload_path'], pid, pid, 'Existing quarantine attribution',
                         10, quarantine_id=row['id'])
        c.close()

    def lifecycle(self, home, quarantine_root=None):
        state = Path(home) / '.local/state/reap'
        leases = json.loads((state / 'leases.json').read_text())
        for lease in leases.get('leases', []):
            pid = self.project_for(lease.get('project'))
            if pid:
                self.add(lease.get('path'), pid, pid,
                         'Current lease project provenance; not disposal authority', 85)
            elif not self.at(lease.get('path') or '/'):
                self.add(lease.get('path'), None, 'unattributed',
                         'Current registered lease; project ownership unresolved', 5)
        config = json.loads((Path(home) / '.config/reap/config.json').read_text())
        root = Path(quarantine_root or config.get('quarantine', {}).get('dir') or state / 'quarantine')
        index_raw = (root / 'index.json').read_bytes()
        index = json.loads(index_raw)
        for entry in index.get('entries', []):
            qid = entry['id']
            payload = str(root / 'entries' / qid / entry['name'])
            provenance = entry.get('provenance') or {}
            pid = self.project_for(provenance.get('project')) or self.project_for(entry.get('project'))
            claims = self.quarantine_claims.get(qid, set())
            if len(claims) == 1:
                pid = next(iter(claims))
            original = entry.get('original_path')
            if not pid and original:
                probe = self.canonical(original)
                while probe != '/':
                    old = self.at(probe)
                    if old and old['project_id']:
                        pid = old['project_id']
                        break
                    probe = os.path.dirname(probe)
            if not pid:
                old = self.at(payload)
                pid = old['project_id'] if old else None
            bucket = 'ownership-conflict' if len(claims) > 1 else 'quarantine-unattributed'
            self.add(payload, None if len(claims) > 1 else pid, bucket,
                     'Live quarantine provenance plus existing project evidence', 120,
                     quarantine_id=qid)
            self.quarantine[payload] = {'id': qid, 'original_path': original,
                                        'held': qid in index.get('held_ids', [])}
        self.lifecycle_info = {
            'quarantine_root': str(root),
            'quarantine_index_sha256': hashlib.sha256(index_raw).hexdigest(),
            'indexed_quarantine_entries': len(index.get('entries', [])),
            'lease_count': len(leases.get('leases', [])),
        }
        return self.lifecycle_info


class Accountant:
    def __init__(self, connection, evidence, output_paths=()):
        self.c = connection
        self.evidence = evidence
        self.exclusions = evidence.exclusions
        self.output_paths = {str(p) for p in output_paths}
        self.seen_units = set()
        self.hardlinks = set()
        self.entries = 0
        self.last_progress = time.monotonic()
        self.errors = 0
        self.excluded = 0
        self.started = now()
        self.top_level_before = set()

    def error(self, path, operation, error):
        self.errors += 1
        self.c.execute('INSERT INTO scan_errors VALUES (?,?,?)',
                       (path, operation, str(error)[:1000]))

    def scan(self, roots):
        for root in roots:
            if root == '/Volumes/kytos':
                self.top_level_before = {e.path for e in os.scandir(root)
                                         if not self.exclusions.contains(e.path)}
            progress({'phase': 'scan_root', 'root': root})
            self.walk(root, root, owner(None, 'unattributed', 'No project evidence', 0), 0)
            self.c.commit()
        for path in sorted(self.evidence.rules):
            if path in self.seen_units or self.exclusions.contains(path):
                continue
            try:
                os.lstat(path)
            except (FileNotFoundError, NotADirectoryError):
                self.record_missing(path, 'ABSENT')
            except OSError as e:
                self.error(path, 'prior_path_lstat', e)
                self.record_missing(path, 'UNREADABLE')
            else:
                self.record_missing(path, 'NOT_SCANNED')
        if self.top_level_before:
            for _ in range(2):
                captured = {r[0] for r in self.c.execute(
                    "SELECT path FROM items WHERE parent_path='/Volumes/kytos'")}
                missing = {e.path for e in os.scandir('/Volumes/kytos')} - captured
                if not missing:
                    break
                for path in sorted(missing):
                    self.walk(path, '/Volumes/kytos',
                              owner(None, 'unattributed', 'Created during scan', 0), 1,
                              device=os.lstat('/Volumes/kytos').st_dev)
            after = {e.path for e in os.scandir('/Volumes/kytos')
                     if not self.exclusions.contains(e.path)}
            captured = {r[0] for r in self.c.execute(
                "SELECT path FROM items WHERE parent_path='/Volumes/kytos'")}
            return {'start_entries': len(self.top_level_before), 'end_entries': len(after),
                    'missing_start_entries': sorted(self.top_level_before - captured),
                    'unscanned_end_entries': sorted(after - captured),
                    'created_during_scan': sorted(after - self.top_level_before),
                    'removed_during_scan': sorted(self.top_level_before - after)}
        return {}

    def walk(self, path, root, inherited, depth, unit=None, device=None, git_scope=None, qid=None):
        if self.exclusions.contains(path):
            self.excluded += 1
            return 0, 0, 0, True
        if path in self.output_paths:
            return 0, 0, 0, True
        try:
            s = os.lstat(path)
        except OSError as e:
            self.error(path, 'lstat', e)
            if depth <= 2 or path in self.evidence.rules:
                self.record_missing(path, 'ABSENT' if isinstance(e, (FileNotFoundError,
                                    NotADirectoryError)) else 'UNREADABLE', inherited, root)
            return 0, 0, 0, False
        self.entries += 1
        if self.entries % 10000 == 0 and time.monotonic() - self.last_progress >= 20:
            progress({'phase': 'walking', 'entries': self.entries, 'units': len(self.seen_units),
                      'errors': self.errors})
            self.last_progress = time.monotonic()
        directory = stat.S_ISDIR(s.st_mode)
        kind = 'directory' if directory else 'symlink' if stat.S_ISLNK(s.st_mode) else 'file' if stat.S_ISREG(s.st_mode) else 'special'
        selected = self.evidence.at(path)
        if not selected or (selected['bucket'] == 'unattributed' and inherited['project_id']):
            selected = dict(inherited)
            selected['basis'] = 'Inherited accounting: ' + inherited['basis'].removeprefix('Inherited accounting: ')
        git = None
        if directory and not git_scope and os.path.lexists(path + '/.git') and not any(p in path.split('/') for p in
                ('.git', '.cargo', '.rustup', '.codex', '.codex-micro', '.codex-gmail',
                 '.claude', '.claude-micro', 'node_modules')):
            git = git_identity(path, self.evidence)
            if git:
                pid = git
                if selected['priority'] < 100 and selected['project_id'] is None:
                    if pid not in self.evidence.projects:
                        self.evidence.projects[pid] = {
                            'project_id': pid, 'primary_root': None,
                            'identity_basis': 'Fresh local Git origin identity',
                        }
                    selected = owner(pid, pid, 'Fresh Git repository identity', 80)
                git_scope = path
        if path.startswith('/Volumes/kytos/') and path.count('/') == 3 and os.path.basename(path).startswith(METADATA_NAMES):
            selected = owner(None, 'macos-volume-metadata', 'macOS-owned volume metadata', 200)
        if path in self.evidence.quarantine:
            qid = self.evidence.quarantine[path]['id']
        quarantine_root = self.evidence.lifecycle_info.get('quarantine_root')
        if quarantine_root and path.startswith(quarantine_root + '/entries/'):
            qid = path[len(quarantine_root + '/entries/'):].split('/')[0]
        make_unit = unit is None or depth <= 2 or path in self.evidence.rules or bool(git)
        if selected['project_id'] != inherited['project_id'] or selected['bucket'] != inherited['bucket']:
            make_unit = True
        allocated = s.st_blocks * 512
        if not directory and s.st_nlink > 1:
            identity = (s.st_dev, s.st_ino)
            if identity in self.hardlinks:
                allocated = 0
            self.hardlinks.add(identity)
        row = None
        if make_unit:
            unit = {'exclusive': allocated}
            self.seen_units.add(path)
            link = os.readlink(path) if kind == 'symlink' else None
            if link and self.exclusions.reference:
                link = self.exclusions.reference.sub('[audit-excluded]', link)
            primary = any(p['primary_root'] and self.evidence.canonical(p['primary_root']) == path
                          for p in self.evidence.projects.values())
            top = '/Volumes/kytos/' + path[len('/Volumes/kytos/'):].split('/')[0] if path.startswith('/Volumes/kytos/') else None
            row = [path, os.path.dirname(path), root, top, kind, 'PRESENT', selected['project_id'],
                   selected['bucket'], selected['basis'],
                   'inherited_accounting' if selected['basis'].startswith('Inherited accounting:') else
                   'confirmed_prior' if selected['priority'] >= 100 else
                   'live_git' if selected['priority'] == 80 else 'carried_forward' if selected['priority'] else 'unknown',
                   selected.get('review_id'), s.st_dev, s.st_ino, s.st_mtime_ns, link]
        else:
            unit['exclusive'] += allocated
        total, files, dirs, complete = allocated, int(not directory), int(directory), True
        if directory:
            if device is not None and s.st_dev != device:
                complete = False
                self.error(path, 'nested_mount', 'Different device; subtree not entered')
                if row:
                    row[5] = 'MOUNT_SKIPPED'
            else:
                try:
                    with os.scandir(path) as entries:
                        for entry in entries:
                            values = self.walk(entry.path, root, selected, depth + 1, unit,
                                               s.st_dev, git_scope, qid)
                            total += values[0]
                            files += values[1]
                            dirs += values[2]
                            complete = complete and values[3]
                except OSError as e:
                    self.error(path, 'scandir', e)
                    complete = False
                    if row:
                        row[5] = 'UNREADABLE'
        if row:
            row += [unit['exclusive'], total, total if complete else None, files, dirs, qid,
                    int(primary), int(directory and selected['bucket'] != 'macos-volume-metadata'), now(), 0]
            self.c.execute('INSERT INTO items VALUES (' + ','.join('?' for _ in row) + ')', row)
        return total, files, dirs, complete

    def record_missing(self, path, state, inherited=None, root=None):
        selected = self.evidence.at(path) or inherited or owner(None, 'unattributed', 'Not observed', 0)
        top = '/Volumes/kytos/' + path[len('/Volumes/kytos/'):].split('/')[0] if path.startswith('/Volumes/kytos/') else None
        values = [path, os.path.dirname(path), root, top, 'unknown', state,
                  selected['project_id'], selected['bucket'], selected['basis'], 'historical_only',
                  selected.get('review_id'), None, None, None, None, 0, 0,
                  0 if state == 'ABSENT' else None, 0, 0, selected.get('quarantine_id'), 0, 0, now(), 0]
        self.c.execute('INSERT INTO items VALUES (' + ','.join('?' for _ in values) + ')', values)
        self.seen_units.add(path)


def owner(project, bucket, basis, priority):
    return {'project_id': project, 'bucket': bucket, 'basis': basis, 'priority': priority,
            'review_id': None, 'quarantine_id': None}


def repository_key(remote):
    if not isinstance(remote, str):
        return None
    remote = remote.strip()
    if any(c.isspace() for c in remote):
        return None
    if '://' in remote:
        url = urlsplit(remote)
        if not url.hostname or url.scheme == 'file':
            return None
        host, path = url.hostname, url.path
    elif ':' in remote and not remote.startswith('/'):
        host, path = remote.split(':', 1)
        host = host.rsplit('@', 1)[-1].lower()
        if host.startswith('github-'):
            host = 'github.com'
    else:
        return None
    if '.' not in host or not path.strip('/'):
        return None
    key = host + '/' + path.lstrip('/')
    return key.removesuffix('.git').rstrip('/') if '/' in key else None


def git_identity(path, evidence):
    marker = Path(path) / '.git'
    if marker.is_symlink():
        return None
    if marker.is_dir():
        common = marker
    elif marker.is_file():
        try:
            with marker.open() as stream:
                line = stream.read(8192).strip()
            if not line.startswith('gitdir: '):
                return None
            common = Path(os.path.normpath(os.path.join(path, line[8:])))
            parent = common / 'commondir'
            if parent.is_file() and not parent.is_symlink():
                with parent.open() as stream:
                    common = Path(os.path.normpath(os.path.join(common, stream.read(8192).strip())))
            old = evidence.at(str(common.parent)) if common.name == '.git' else None
            if old and old['project_id']:
                return old['project_id']
        except (OSError, UnicodeError):
            return None
    else:
        return None
    config = common / 'config'
    if config.is_symlink():
        return None
    try:
        result = subprocess.run(
            ['git', 'config', '--file', str(config), '--get', 'remote.origin.url'],
            capture_output=True, text=True, timeout=10,
            env={**os.environ, 'GIT_OPTIONAL_LOCKS': '0'},
        )
        if result.returncode:
            return None
        remote = result.stdout.strip()
        if remote and remote.startswith('/'):
            return evidence.project_for(remote)
        if remote.startswith('file://'):
            return evidence.project_for(unquote(urlsplit(remote).path))
        key = repository_key(remote)
        return evidence.project_for(key) or key
    except (OSError, UnicodeError, ValueError, subprocess.TimeoutExpired):
        return None


def readonly(path):
    c = sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)
    c.row_factory = sqlite3.Row
    return c


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def progress(value):
    print(json.dumps(value), flush=True)


def capture(previous, output, roots, evidence):
    if output.exists() or output.resolve() == previous.resolve():
        raise ValueError('Refusing to replace an existing database')
    os.umask(0o077)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(prefix='.accounting-', suffix='.sqlite', dir=output.parent,
                                     delete=False) as stream:
        staging = Path(stream.name)
    c = sqlite3.connect(staging)
    try:
        c.executescript(SCHEMA)
        accountant = Accountant(c, evidence, (staging, output, str(staging) + '-journal'))
        top_coverage = accountant.scan(roots)
        for project in evidence.projects.values():
            c.execute('INSERT INTO projects VALUES (?,?,?)',
                       (project['project_id'], project['primary_root'], project['identity_basis']))
        for path, rules in evidence.rules.items():
            for rule in rules:
                c.execute('INSERT INTO prior_evidence VALUES (?,?,?,?,?,?,?)',
                           (path, rule['project_id'], rule['bucket'], rule['basis'], rule['priority'],
                            rule.get('review_id'), rule.get('quarantine_id')))
        for path, entry in evidence.quarantine.items():
            rule = evidence.at(path)
            c.execute('INSERT INTO quarantine_entries VALUES (?,?,?,?,?)',
                       (entry['id'], entry['original_path'], path,
                        rule['project_id'] if rule else None, int(entry['held'])))
        summary = {'host': HOST, 'started_at': accountant.started, 'finished_at': now(),
                   'collector_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                   'previous_database': str(previous), 'source_review_ids': evidence.review_ids,
                   'prior_inventory_counts': evidence.inventory_counts, 'scan_roots': roots,
                   'lifecycle': evidence.lifecycle_info,
                   'filesystem_entries_seen': accountant.entries, 'excluded_subtrees_skipped': accountant.excluded,
                   'exclusion_policy': 'audit-exclusions.json', 'scan_errors': accountant.errors,
                   'top_level_coverage': top_coverage, 'cleanup_authority': 'NONE',
                   'existing_review_database_modified': False, 'laptop_storage_scanned': False,
                   'volumes_at_finish': {p: {
                       'capacity_bytes': os.statvfs(p).f_blocks * os.statvfs(p).f_frsize,
                       'available_bytes': os.statvfs(p).f_bavail * os.statvfs(p).f_frsize,
                   } for p in ROOTS if p in roots},
                   'measurement': 'Allocated st_blocks bytes; hardlinks counted once in traversal order. '
                                  'APFS clone/shared extents are not deduplicated. Incomplete sizes are NULL, '
                                  'with measured lower bounds retained. Snapshot is live, not atomic.',
                   'attribution': 'Existing project association takes precedence over differing Git code origins. '
                                  'Fresh Git/provenance resolves unowned paths. '
                                  'Inherited accounting never grants child ownership or cleanup authority.'}
        c.execute('INSERT INTO metadata VALUES (?,?)', ('summary', json.dumps(summary)))
        c.commit()
        if c.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
            raise ValueError('Output database integrity check failed')
        if top_coverage.get('missing_start_entries') or top_coverage.get('unscanned_end_entries'):
            raise ValueError('Top-level starting inventory was not fully recorded')
        for table, column in [('items', 'path'), ('prior_evidence', 'path'), ('scan_errors', 'path')]:
            if any(evidence.exclusions.contains(r[0]) for r in c.execute(f'SELECT {column} FROM {table}')):
                raise ValueError('Excluded scope entered output')
        c.close()
        os.link(staging, output)
        progress({'phase': 'complete', 'database': str(output), **summary})
        return summary
    finally:
        c.close()
        if staging.exists():
            staging.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--previous', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if socket.gethostname().split('.')[0] != 'catalyst-mini' or not os.path.ismount('/Volumes/kytos'):
        parser.error('Run on catalyst-mini with Kytos mounted')
    if not args.output.resolve().is_relative_to((REPO / '.reap-audits').resolve()):
        parser.error('Keep this retained accounting snapshot inside the Reap repository audit directory')
    aliases = {p: os.path.realpath(p) for p in ('/Users/spencer/src', '/Users/spencer/projects', '/tmp')
               if os.path.islink(p)}
    evidence = Evidence(Exclusions(), aliases)
    evidence.load(args.previous)
    lifecycle = evidence.lifecycle('/Users/spencer')
    roots = list(ROOTS)
    for path in sorted(evidence.rules, key=lambda p: (p.count('/'), p)):
        if path.startswith(('/private/tmp/', '/private/var/folders/')) and not any(
                path == root or path.startswith(root + '/') for root in roots):
            try:
                os.lstat(path)
                roots.append(path)
            except OSError:
                pass
    progress({'phase': 'prepared', 'projects': len(evidence.projects), 'evidence_paths': len(evidence.rules),
              'roots': len(roots), **lifecycle})
    capture(args.previous, args.output, roots, evidence)


if __name__ == '__main__':
    main()
