"""Apply the owner's audit scope exclusions without changing directory contents."""

import argparse
import collections
import datetime
import hashlib
import json
import os
import re
import sqlite3
from decimal import Decimal
from pathlib import Path


POLICY = Path(__file__).resolve().parents[1] / 'audit-exclusions.json'
DROP = object()
PATH_FIELDS = ('path', 'item_id', 'original_path', 'payload_path', 'entrypoint_root', 'root_path')
JSON_TABLES = {'project_packets': 'json', 'audit_documents': 'json', 'agent_reviews': 'review_json'}
INVENTORY_TABLES = ('attribution', 'kytos_top_level', 'unknown_and_shared')
PERMISSION_FIELDS = (
    'path', 'quarantine_entry_id', 'decision', 'cleanup_authorized', 'restore_authorized',
    'cleanup_paths', 'cleanup_identity_records', 'cleanup_identity_columns', 'restore_to',
    'early_purge_approved', 'cleanup_completed', 'execution_status', 'retained_child_moves',
)


class Exclusions:
    def __init__(self, policy=POLICY):
        data = json.loads(Path(policy).read_text())
        if data.get('version') != 1 or not isinstance(data.get('paths'), list):
            raise ValueError('Invalid audit exclusion policy')
        self.paths = []
        for item in data['paths']:
            path = item['path']
            if not isinstance(path, str) or not os.path.isabs(path) or path == '/' or os.path.normpath(path) != path or not item.get('reason'):
                raise ValueError('Exclusions require a normalized absolute non-root path and reason')
            self.paths.append(path)
        self.reference = re.compile('|'.join(re.escape(p) + r"(?=/|[^\w./-]|[.,](?=\s|[\"')\]}]|$)|$)" for p in self.paths)) if self.paths else None

    def contains(self, path):
        if not isinstance(path, str) or not os.path.isabs(path):
            return False
        path = os.path.normpath(path)
        return any(path == p or path.startswith(p + '/') for p in self.paths)

    def overlaps(self, path):
        path = os.path.normpath(path)
        return self.contains(path) or any(p.startswith(path.rstrip('/') + '/') for p in self.paths)

    def clean(self, value):
        if isinstance(value, dict):
            if any(self.contains(value.get(k)) for k in PATH_FIELDS):
                return DROP
            result = {}
            for key, item in value.items():
                if self.contains(key):
                    continue
                cleaned = self.clean(item)
                if cleaned is not DROP:
                    result[key] = cleaned
            return result
        if isinstance(value, list):
            return [cleaned for item in value if (cleaned := self.clean(item)) is not DROP]
        if isinstance(value, str):
            if self.contains(value):
                return DROP
            return self.reference.sub('[audit-excluded personal directory]', value) if self.reference and any(p in value for p in self.paths) else value
        return value


def permission_fingerprint(review, exclusions):
    records = []
    for key in ('decisions', 'quarantine_decisions'):
        for item in review.get(key, []):
            if exclusions.contains(item.get('path')):
                continue
            records.append({k: item[k] for k in PERMISSION_FIELDS if k in item})
    return hashlib.sha256(json.dumps(records, sort_keys=True).encode()).hexdigest()


def matching_json_rows(connection, table, column, exclusions):
    if not exclusions.paths:
        return []
    condition = ' OR '.join(f'instr("{column}", ?) > 0' for _ in exclusions.paths)
    return list(connection.execute(f'SELECT rowid, length("{column}") FROM "{table}" WHERE {condition}', exclusions.paths))


def reconcile(connection, exclusions, apply=False):
    tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    plan = {'inventory_rows': {}, 'json_rows': {}, 'apply': apply}
    candidates = {}
    for table, column in JSON_TABLES.items():
        if table in tables:
            candidates[table] = matching_json_rows(connection, table, column, exclusions)
            plan['json_rows'][table] = len(candidates[table])
    for table in INVENTORY_TABLES:
        if table in tables:
            rows = list(connection.execute(f'SELECT rowid, * FROM "{table}"'))
            plan['inventory_rows'][table] = [row[0] for row in rows if exclusions.contains(row['path'])]
    if not apply:
        plan['inventory_rows'] = {k: len(v) for k, v in plan['inventory_rows'].items()}
        plan['json_bytes_to_review'] = sum(size for rows in candidates.values() for _, size in rows)
        return plan

    latest = set()
    if 'agent_reviews' in tables:
        latest = {row[0] for row in connection.execute('SELECT MAX(id) FROM agent_reviews GROUP BY root_project')}
    removed_sizes = collections.defaultdict(lambda: [Decimal(0), 0])
    for table, rowids in plan['inventory_rows'].items():
        for rowid in rowids:
            if table == 'attribution':
                row = connection.execute('SELECT * FROM attribution WHERE rowid=?', (rowid,)).fetchone()
                amount = row['exclusive_allocated_gib_estimate']
                if amount not in (None, '', 'UNKNOWN'):
                    removed_sizes[row['root_project']][0] += Decimal(amount)
                removed_sizes[row['root_project']][1] += 1
            connection.execute(f'DELETE FROM "{table}" WHERE rowid=?', (rowid,))
    for project, (size, count) in removed_sizes.items():
        for table, column in (('project_totals', 'allocated_gib_estimate'), ('handoff_index', 'allocated_gib_estimate_not_reclaimable')):
            if table not in tables:
                continue
            row = connection.execute(f'SELECT "{column}" FROM "{table}" WHERE root_project=?', (project,)).fetchone()
            if row and row[0] not in (None, '', 'UNKNOWN'):
                connection.execute(f'UPDATE "{table}" SET "{column}"=? WHERE root_project=?', (str(Decimal(row[0]) - size), project))
        if 'project_totals' in tables:
            connection.execute('UPDATE project_totals SET unit_count=CAST(unit_count AS INTEGER)-? WHERE root_project=?', (count, project))

    for table, rows in candidates.items():
        column = JSON_TABLES[table]
        for rowid, _ in rows:
            raw = connection.execute(f'SELECT "{column}" FROM "{table}" WHERE rowid=?', (rowid,)).fetchone()[0]
            before = json.loads(raw)
            after = exclusions.clean(before)
            if after is DROP:
                raise ValueError('Refusing to remove a complete review or document')
            if table == 'agent_reviews':
                if permission_fingerprint(before, exclusions) != permission_fingerprint(after, exclusions):
                    raise ValueError('Exclusion changes unrelated cleanup authority')
                if rowid in latest:
                    refresh_summary(before, after, exclusions)
            encoded = json.dumps(after, ensure_ascii=True, separators=(',', ':'))
            if exclusions.reference and exclusions.reference.search(encoded):
                raise ValueError('Excluded path reference survived reconciliation')
            connection.execute(f'UPDATE "{table}" SET "{column}"=? WHERE rowid=?', (encoded, rowid))
            print(json.dumps({'updated_table': table, 'rowid': rowid}), flush=True)
    plan['inventory_rows'] = {k: len(v) for k, v in plan['inventory_rows'].items()}
    return plan


def refresh_summary(before, after, exclusions):
    summary = after.get('summary', {})
    removed = [x for x in before.get('decisions', []) if exclusions.contains(x.get('path'))]
    if not removed:
        return
    summary['scope_exclusions_applied'] = {
        'policy': 'audit-exclusions.json', 'removed_path_decisions': len(removed),
        'observed_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'authority': 'Spencer requires personal directories to be outside project audits and absent from this database.',
        'unrelated_cleanup_permissions_preserved': True,
    }
    boundaries = summary.get('completion_boundaries', {})
    accounting = boundaries.get('accounting', {})
    if accounting.get('pending_exact_paths') == []:
        accounting['status'] = 'COMPLETE'
        accounting['exclusion_policy'] = 'audit-exclusions.json'
    coverage = boundaries.get('coverage', {})
    if isinstance(coverage.get('current_project_items'), int):
        coverage['current_project_items'] -= len(removed)
        coverage['scope_exclusions_applied'] = len(removed)
    if 'current_live_decision_counts' in summary:
        counts = summary['current_live_decision_counts']
        for item in removed:
            verdict = item.get('decision')
            if item.get('current_exists') is True and verdict in counts:
                counts[verdict] -= 1
    if accounting.get('status') == 'COMPLETE':
        if isinstance(summary.get('formal_review_status'), str):
            summary['formal_review_status'] = summary['formal_review_status'].replace('_AND_THREE_CENTRAL_ACCOUNTING_CORRECTIONS', '')
        delivery = summary.get('delivery', {})
        if isinstance(delivery.get('status'), str):
            delivery['status'] = delivery['status'].replace('EXISTING_AUTHORITY_ACCOUNTING_EXCEPTIONS_REMAIN', 'EXISTING_AUTHORITY_EXCEPTIONS_REMAIN')
        if isinstance(delivery.get('remaining_review_exceptions'), str):
            for phrase in (' and three central shared/personal attribution corrections', ', and three central personal/shared accounting corrections are unchanged'):
                delivery['remaining_review_exceptions'] = delivery['remaining_review_exceptions'].replace(phrase, '')
        delivery['accounting_status'] = 'COMPLETE'
    old_groups = before.get('summary', {}).get('space_accounting', {}).get('groups', [])
    removed_bytes = sum(sum(x.get('allocated_bytes_by_content_verdict', {}).values()) for x in old_groups if exclusions.contains(x.get('entrypoint_root')))
    if removed_bytes:
        space = summary['space_accounting']
        for obj, key in ((space, 'total_current_allocated_bytes'), (space, 'projected_keep_bytes'), (summary.get('content_audit', {}), 'projected_retained_upper_bound_bytes')):
            if isinstance(obj.get(key), int):
                obj[key] -= removed_bytes
        for obj in (space, summary.get('content_audit', {})):
            counts = obj.get('allocated_bytes_by_content_verdict', {})
            if isinstance(counts.get('KEEP'), int):
                counts['KEEP'] -= removed_bytes
        space['scope_exclusion_adjustment_bytes'] = removed_bytes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('database', type=Path)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    exclusions = Exclusions()
    connection = sqlite3.connect(args.database.resolve().as_uri() + ('?mode=rw' if args.apply else '?mode=ro'), uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute('BEGIN IMMEDIATE' if args.apply else 'BEGIN')
        plan = reconcile(connection, exclusions)
        print(json.dumps(plan), flush=True)
        if args.apply:
            volume = os.statvfs(args.database.parent)
            if volume.f_bavail * volume.f_frsize < plan['json_bytes_to_review'] * 2 + 128 * 1024**2:
                raise ValueError('Insufficient journal and transaction headroom')
            result = reconcile(connection, exclusions, apply=True)
            if connection.execute('PRAGMA quick_check').fetchone()[0] != 'ok':
                raise ValueError('SQLite quick_check failed')
            connection.commit()
            print(json.dumps({'completed': result, 'quick_check': 'ok'}), flush=True)
    finally:
        connection.close()


if __name__ == '__main__':
    main()
