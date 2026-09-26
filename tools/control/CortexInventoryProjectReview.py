#!/usr/bin/env python3
"""Read-only, metadata-only project candidate preview from the persistent volume index.

The preview is *not* a project registry, classifier of source content, or permission to
move/register anything. It reads an existing SQLite snapshot and only writes an
explicit review JSON under Cortex artifacts on request. No filesystem tree walk.
"""
from __future__ import annotations

import argparse
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import sqlite3
import tempfile
from typing import Any

from CortexPersistentVolumeInventory import _connection, _stored_volume_id, _status, index_location

FILE_MARKERS = {
    'project.control.json': 'pcc', 'cargo.toml': 'cargo', 'cmakelists.txt': 'cmake',
    'package.json': 'node', 'pyproject.toml': 'python', 'go.mod': 'go',
    'gradlew': 'gradle', 'settings.gradle': 'gradle', 'settings.gradle.kts': 'gradle',
    'build.gradle': 'gradle', 'build.gradle.kts': 'gradle',
    'global.json': 'dotnet', 'meson.build': 'meson',
}
DIRECTORY_MARKERS = {'.git': 'git'}
GENERATED = frozenset({'.git', '.cortex', 'artifacts', 'target', 'build', 'builds',
                       'node_modules', '__pycache__', '.venv', 'venv', '.gradle',
                       '.next', '.idea', '.vs', 'dist', 'out', 'bin', 'obj'})


def _generated(relative: str) -> bool:
    parts = PurePosixPath(relative).parts
    # A root itself may contain the .git marker; its parents, never the marker,
    # are tested to avoid suppressing the project that owns it.
    return any(part.casefold() in GENERATED for part in parts[:-1])


def _marker_root(path: str) -> str:
    return PurePosixPath(path).parent.as_posix().removeprefix('./') if '/' in path else ''


def _identity(volume_id: str, relative: str) -> str:
    return hashlib.sha256((volume_id + '\0' + relative.casefold()).encode('utf-8')).hexdigest()[:24]


def build_preview(db_path: Path | str, mount_root: Path | str, *, volume_id: str,
                  candidate_limit: int = 250000) -> dict[str, Any]:
    """Stream indexed marker rows; never open a source file or create an index."""
    if candidate_limit < 1:
        raise ValueError('candidate_limit must be positive')
    with closing(_connection(Path(db_path), writable=False)) as conn:
        conn.execute('PRAGMA query_only=ON')
        conn.execute('BEGIN')
        try:
            if _stored_volume_id(conn) != volume_id:
                raise ValueError('Volume identity mismatch: refusing cross-volume review')
            status = _status(conn, Path(mount_root))
            if status['state'] != 'complete' or status['pending_directories']:
                raise RuntimeError('Complete initial inventory before generating an organization review')
            # SQLite streams rows from its index; neither the drive nor source bytes
            # are inspected. Detect authoritative marker types, not guessed folders.
            markers: dict[str, set[str]] = {}
            paths: dict[str, list[str]] = {}
            examined = 0
            for row in conn.execute('SELECT path, type FROM entries'):
                examined += 1
                rel = str(row['path']).replace('\\', '/')
                if _generated(rel):
                    continue
                kind = str(row['type'])
                basename = PurePosixPath(rel).name.casefold()
                marker = (FILE_MARKERS.get(basename) if kind == 'file' else
                          DIRECTORY_MARKERS.get(basename) if kind == 'directory' else None)
                if not marker:
                    continue
                root = _marker_root(rel)
                key = root.casefold()
                markers.setdefault(key, set()).add(marker)
                paths.setdefault(key, []).append(rel)
                # Fail closed instead of emitting an incomplete authoritative review.
                if len(markers) > candidate_limit:
                    raise RuntimeError('Review candidate limit exceeded; use a narrower volume/source policy')
            candidates: list[dict[str, Any]] = []
            for key in sorted(markers):
                evidence = sorted(paths[key], key=str.casefold)
                relative_root = _marker_root(evidence[0])
                kinds = sorted(markers[key])
                strength = ('strong' if 'pcc' in kinds or 'git' in kinds else
                            'marker_only')
                candidates.append({
                    'candidate_id': _identity(volume_id, relative_root),
                    'volume_id': volume_id,
                    'relative_root': relative_root,
                    'display_name': PurePosixPath(relative_root).name if relative_root else Path(mount_root).name,
                    'marker_kinds': kinds, 'marker_paths': evidence,
                    'evidence_strength': strength, 'parent_candidate_id': None,
                    'action': 'review_required', 'automatically_registered': False,
                })
            # Preserve nested relationships instead of flattening children. A
            # parent-child edge is evidence of containment, not duplicate identity.
            by_root = {item['relative_root'].casefold(): item for item in candidates}
            for item in candidates:
                parts = PurePosixPath(item['relative_root']).parts
                for depth in range(len(parts) - 1, -1, -1):
                    parent = PurePosixPath(*parts[:depth]).as_posix() if depth else ''
                    parent_item = by_root.get(parent.casefold())
                    if parent_item is not None and parent_item is not item:
                        item['parent_candidate_id'] = parent_item['candidate_id']
                        break
            # Similar names are review candidates only, never asserted duplicates.
            same_names: dict[str, list[str]] = {}
            for item in candidates:
                same_names.setdefault(item['display_name'].casefold(), []).append(item['candidate_id'])
            groups = [{'name': name, 'candidate_ids': ids} for name, ids in sorted(same_names.items()) if len(ids) > 1]
            return {
                'schema': 'cortex.inventory.project_review.preview.v1',
                'generated_utc': datetime.now(timezone.utc).isoformat(),
                'volume_id': volume_id, 'mount_root': str(mount_root),
                'inventory_state': status['state'], 'indexed_entries': status['entries'],
                'index_access_gaps': status['errors'], 'entries_examined': examined,
                'candidate_count': len(candidates), 'same_name_group_count': len(groups),
                'candidates': candidates, 'same_name_review_groups': groups,
                'disclaimer': 'Metadata-only candidate proposal; not registration, duplication proof or source classification.',
                'source_files_read': False, 'source_files_mutated': False,
                'registry_mutated': False, 'inventory_mutated': False,
                'requires_user_approval': True,
            }
        finally:
            conn.rollback()


def export_preview(report: dict[str, Any], cortex_root: Path | str) -> Path:
    """Explicitly persist a preview under artifacts, never in the project registry."""
    dest = Path(cortex_root).resolve() / 'artifacts' / 'reviews'
    dest.mkdir(parents=True, exist_ok=True)
    fd, temp = tempfile.mkstemp(prefix='.inventory-project-review-', suffix='.json.tmp', dir=dest)
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as handle:
            json.dump(report, handle, indent=2, ensure_ascii=False, sort_keys=True)
            handle.write('\n')
            handle.flush(); os.fsync(handle.fileno())
        unique = str(datetime.now(timezone.utc).strftime('%Y%m%d-%H%M%S-%f'))
        target = dest / f'inventory-project-review-{unique}.json'
        os.replace(temp, target)
        return target
    finally:
        if os.path.exists(temp):
            os.unlink(temp)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Read-only inventory-backed project-candidate proposal')
    parser.add_argument('--cortex-root', type=Path, required=True)
    parser.add_argument('--save-review', action='store_true', help='Write a JSON proposal under Cortex artifacts/reviews')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    try:
        mount, db, vid = index_location(args.cortex_root)
        report = build_preview(db, mount, volume_id=vid)
        if args.save_review:
            report['review_file'] = str(export_preview(report, args.cortex_root))
    except (OSError, RuntimeError, ValueError, sqlite3.Error) as exc:
        report = {'schema': 'cortex.inventory.project_review.preview.v1', 'error': str(exc),
                  'passed': False, 'source_files_mutated': False, 'registry_mutated': False}
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(json.dumps({'candidate_count': report.get('candidate_count'), 'error': report.get('error'),
                          'review_file': report.get('review_file')}, indent=2))
    return 2 if report.get('error') else 0


if __name__ == '__main__':
    raise SystemExit(main())
