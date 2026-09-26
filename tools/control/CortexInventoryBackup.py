#!/usr/bin/env python3
"""Explicit SQLite online backup / offline restore rehearsal for Cortex metadata only.

Never writes to the active inventory; never replaces the live DB, volume marker or
Vault catalog. This is NOT the governed-source/Vault recovery certification gate.
"""
from __future__ import annotations

import argparse
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
from typing import Any

from CortexPersistentVolumeInventory import _connection, _stored_volume_id, index_location


def _digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def inspect_backup(path: Path | str, *, volume_id: str) -> dict[str, Any]:
    """Open the backup read-only; include structural and volume identity verification."""
    path = Path(path)
    with closing(_connection(path, writable=False)) as conn:
        conn.execute('PRAGMA query_only=ON')
        actual = _stored_volume_id(conn)
        if actual != volume_id:
            raise ValueError('Backup volume identity mismatch')
        quick = [str(row[0]) for row in conn.execute('PRAGMA quick_check')]
        if quick != ['ok']:
            raise RuntimeError('SQLite backup quick_check failed: ' + str(quick[:5]))
        counts = {str(row[0]): int(row[1]) for row in conn.execute('SELECT type,COUNT(*) FROM entries GROUP BY type')}
        return {'sqlite_quick_check': True, 'volume_id': actual,
                'actual_entry_count': sum(counts.values()), 'actual_counts': counts,
                'user_version': int(conn.execute('PRAGMA user_version').fetchone()[0]),
                'source_read_only': True}


def backup_inventory(db_path: Path | str, *, volume_id: str, output_dir: Path | str) -> dict[str, Any]:
    """Snapshot active WAL index to a separate artifact and verify before publication."""
    source = Path(db_path).resolve(strict=True)
    output = Path(output_dir).resolve()
    if source.name != 'inventory.sqlite3':
        raise ValueError('Only the dedicated inventory.sqlite3 may be backed up')
    if output == source.parent or source in output.parents:
        raise ValueError('Backup target cannot be the live database directory')
    if shutil.disk_usage(output if output.exists() else output.parent).free < source.stat().st_size * 2 + 16 * 1024 * 1024:
        raise OSError('Insufficient available storage for a verified SQLite backup')
    output.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix='.inventory-online-backup-', suffix='.sqlite3', dir=output)
    os.close(fd)
    temp = Path(temp_name)
    try:
        with closing(_connection(source, writable=False)) as src:
            if _stored_volume_id(src) != volume_id:
                raise ValueError('Source inventory identity mismatch')
            with closing(sqlite3.connect(str(temp), timeout=30)) as dest:
                src.backup(dest, pages=256, sleep=0.05)
                dest.commit()
        proof = inspect_backup(temp, volume_id=volume_id)
        digest = _digest(temp)
        stamp = datetime.now(timezone.utc).strftime('%Y%m%d-%H%M%S-%f')
        backup = output / f'inventory-{volume_id}-{stamp}.sqlite3'
        os.replace(temp, backup)
        report = {'schema': 'cortex.inventory.online_backup.v1',
                  'created_utc': datetime.now(timezone.utc).isoformat(),
                  'backup': str(backup), 'sha256': digest, 'bytes': backup.stat().st_size,
                  'verification': proof, 'source_database': str(source),
                  'live_database_replaced': False, 'project_source_backed_up': False}
        receipt = backup.with_name(backup.name + '.receipt.json')
        receipt.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        return report
    finally:
        temp.unlink(missing_ok=True)


def restore_rehearsal(backup: Path | str, *, volume_id: str, expected_sha256: str | None = None) -> dict[str, Any]:
    """Validate a stand-alone copy without ever restoring over the live DB."""
    backup = Path(backup).resolve(strict=True)
    digest = _digest(backup)
    if expected_sha256 and expected_sha256.lower() != digest:
        raise ValueError('Backup SHA-256 receipt mismatch')
    result = inspect_backup(backup, volume_id=volume_id)
    result.update({'schema': 'cortex.inventory.restore_rehearsal.v1', 'sha256': digest,
                   'backup': str(backup), 'live_database_replaced': False,
                   'restore_applied': False})
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Explicit backup or verification of Cortex inventory metadata')
    parser.add_argument('--cortex-root', type=Path, required=True)
    choices = parser.add_mutually_exclusive_group(required=True)
    choices.add_argument('--backup', action='store_true')
    choices.add_argument('--verify-copy', type=Path)
    parser.add_argument('--expected-sha256')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    try:
        _, db, volume_id = index_location(args.cortex_root)
        report = (backup_inventory(db, volume_id=volume_id,
                     output_dir=args.cortex_root / 'artifacts' / 'inventory-backups')
                  if args.backup else restore_rehearsal(args.verify_copy, volume_id=volume_id,
                                                        expected_sha256=args.expected_sha256))
        exit_code = 0
    except (OSError, sqlite3.Error, ValueError, RuntimeError) as exc:
        report, exit_code = {'passed': False, 'error': str(exc), 'live_database_replaced': False}, 2
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else str(report))
    return exit_code


if __name__ == '__main__':
    raise SystemExit(main())
