#!/usr/bin/env python3
"""CTX-UPCC-A05: explicit, source-preserving Cortex project contract migration.

A manifest's rollback declaration must never promise recovery that the executing
provider does not actually implement. These two historical values are metadata,
not a functioning formatter snapshot or a process rollback implementation.
The formatter remains a governed workspace mutation with NO automated rollback;
the runner keeps bounded process-tree cancellation independently.

Default is read-only. --yes is an explicit user-authorized mutation. This helper
is Cortex-project-specific and must not be used to rewrite another project.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import UniversalPCCAudit as audit

SCHEMA = 'cortex.contract_migration.v1'
MAX_BYTES = 4 * 1024 * 1024
# (key, obsolete policy, supported honest policy, program, argv, risk, side effect, permission)
RULES = (
    ('fmt.apply', 'git_or_snapshot', 'none', 'cargo', ['fmt', '--all'],
     'local_mutation', 'source_files', 'workspace_write'),
    ('forge.rust.run', 'process_stop', 'none', 'cargo',
     ['run', '--manifest-path', 'products/forge-rust/Cargo.toml', '-p', 'forge-rs'],
     'local_mutation', 'process_launch', None),
)


class MigrationError(RuntimeError):
    pass


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise MigrationError(f'duplicate JSON key: {key}')
        result[key] = value
    return result


def plan(root: Path, expected_sha256: str | None = None) -> tuple[dict[str, Any], bytes, bytes]:
    root = root.expanduser().resolve()
    path = root / 'project.control.json'
    if not path.is_file() or path.is_symlink():
        raise MigrationError('project.control.json must be a regular, non-symlink source file')
    if path.stat().st_size > MAX_BYTES:
        raise MigrationError('project contract exceeds the 4 MiB safety limit')
    original = path.read_bytes()
    if expected_sha256 and sha(original).lower() != expected_sha256.lower():
        raise MigrationError('contract SHA-256 preimage mismatch; refusing migration')
    try:
        manifest = json.loads(original.decode('utf-8-sig'), object_pairs_hook=_unique_object)
    except (UnicodeError, ValueError) as exc:
        raise MigrationError(f'cannot parse unambiguous contract: {exc}') from exc
    if not isinstance(manifest, dict) or manifest.get('schema') != 'forge.project.v1':
        raise MigrationError('unsupported project schema; no migration')
    project = manifest.get('project')
    if not isinstance(project, dict) or project.get('id') != 'cortex':
        raise MigrationError('project identity is not exactly cortex; no cross-project rewrite')
    commands = manifest.get('commands')
    if not isinstance(commands, list):
        raise MigrationError('command registry missing')
    changes: list[dict[str, str]] = []
    for key, old, new, program, args, risk, side_effect, permission in RULES:
        matching = [cmd for cmd in commands if isinstance(cmd, dict) and cmd.get('key') == key]
        if len(matching) != 1:
            raise MigrationError(f'exactly one {key} command is required')
        command = matching[0]
        if (command.get('program') != program or command.get('args') != args
                or command.get('risk') != risk or side_effect not in command.get('side_effects', [])
                or command.get('cancellation') != 'bounded_kill'
                or (permission is not None and permission not in command.get('permissions', []))):
            raise MigrationError(f'{key}: command semantics changed; cannot assume migration is safe')
        current = command.get('rollback')
        if current == new:
            continue
        if current != old:
            raise MigrationError(f'{key}: unexpected rollback {current!r}; refusing replacement')
        if original.count(('"' + old + '"').encode('utf-8')) != 1:
            raise MigrationError(f'{key}: obsolete token is not unique in source; refusing textual rewrite')
        changes.append({'command': key, 'from': old, 'to': new})

    if not changes:
        proposed = original
    else:
        proposed = original
        for change in changes:
            proposed = proposed.replace(('"' + change['from'] + '"').encode(),
                                        ('"' + change['to'] + '"').encode(), 1)
        # Compare semantic trees, excluding *only* the approved field changes.
        expected = json.loads(original.decode('utf-8-sig'), object_pairs_hook=_unique_object)
        for command in expected['commands']:
            for change in changes:
                if command['key'] == change['command']:
                    command['rollback'] = change['to']
        candidate = json.loads(proposed.decode('utf-8-sig'), object_pairs_hook=_unique_object)
        if candidate != expected:
            raise MigrationError('unexpected unrelated JSON change')
        validation = audit.validate_contract(candidate, rust_consumer=True)
        if validation['status'] == 'INVALID':
            raise MigrationError('post-migration Rust compatibility failed: ' + '; '.join(validation['errors'][:5]))
    report = {
        'schema': SCHEMA, 'root': str(root), 'originalSha256': sha(original),
        'proposedSha256': sha(proposed), 'changes': changes, 'requiresConsent': bool(changes),
        'automaticRollbackGuaranteed': False, 'previewOnly': True,
        'warning': 'fmt.apply changes source without automatic rollback; preserve Git/snapshot recovery separately. '
                   'process_stop is cancellation, not rollback; bounded_kill is retained.',
    }
    return report, original, proposed


def apply(root: Path, expected_sha256: str | None = None) -> dict[str, Any]:
    # Reuse the pre-existing PCC single-writer operation lock. No parallel authority.
    from CortexPCCMaintenance import OperationLock
    root = root.expanduser().resolve()
    with OperationLock(root, 'contract-migration'):
        report, original, proposed = plan(root, expected_sha256)
        if not report['changes']:
            report['status'] = 'ALREADY_COMPATIBLE'
            return report
        # Preserve original bytes before any source mutation.
        recovery = root / 'artifacts' / 'recovery' / 'contract' / (
            datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + uuid.uuid4().hex[:12])
        recovery.mkdir(parents=True, exist_ok=False)
        backup = recovery / 'project.control.json'
        with backup.open('xb') as handle:
            handle.write(original)
            handle.flush()
            os.fsync(handle.fileno())
        if sha(backup.read_bytes()) != sha(original):
            raise MigrationError('backup byte verification failed; original contract untouched')
        path = root / 'project.control.json'
        if sha(path.read_bytes()) != sha(original) or path.is_symlink():
            raise MigrationError('contract changed during backup; original source not overwritten')
        temporary = path.with_name(f'.project.control.{uuid.uuid4().hex}.tmp')
        try:
            with temporary.open('xb') as handle:
                handle.write(proposed)
                handle.flush()
                os.fsync(handle.fileno())
            if sha(temporary.read_bytes()) != sha(proposed):
                raise MigrationError('staged contract checksum mismatch')
            if sha(path.read_bytes()) != sha(original) or path.is_symlink():
                raise MigrationError('contract modified after staging; aborting')
            os.replace(temporary, path)
            if sha(path.read_bytes()) != sha(proposed):
                raise MigrationError('post-apply checksum mismatch; original preserved in backup')
        finally:
            if temporary.exists():
                temporary.unlink()
        report.update(status='APPLIED_SOURCE_NEEDS_NEW_FULL_GATE', previewOnly=False,
                      originalBackup=str(backup), backupSha256=sha(original))
        receipt = recovery / 'receipt.json'
        receipt.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Cortex-only guarded legacy rollback metadata repair')
    parser.add_argument('--root', required=True)
    parser.add_argument('--expected-sha256', help='Optional additional exact preimage guard')
    parser.add_argument('--yes', action='store_true', help='Explicitly accept loss of the legacy unimplemented rollback promise')
    args = parser.parse_args(argv)
    try:
        if args.yes:
            report = apply(Path(args.root), args.expected_sha256)
        else:
            report, _, _ = plan(Path(args.root), args.expected_sha256)
            report['status'] = 'PREVIEW_ONLY'
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    except (MigrationError, OSError, ValueError) as exc:
        print(json.dumps({'schema': SCHEMA, 'status': 'BLOCKED', 'error': str(exc)}), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
