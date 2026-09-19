#!/usr/bin/env python3
"""Read-only command/gate plan inspection across project-owned PCC contracts.

This is deliberately NOT an execution engine. It never guesses a build target,
changes manifests or invokes an arbitrary command from a project contract.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Any

from UniversalPCCAudit import _read_manifest, validate_contract

VERSION = 'UPCC-A02-0.1'


def _executable(program: str, root: Path) -> dict[str, Any]:
    """Resolve without executing, loading or trusting the binary."""
    if not isinstance(program, str) or not program.strip() or '\x00' in program:
        return {'status': 'UNAVAILABLE', 'path': None, 'reason': 'missing or invalid executable'}
    path = Path(program)
    if path.is_absolute():
        return {'status': 'RESOLVED' if path.is_file() else 'UNAVAILABLE',
                'path': str(path) if path.is_file() else None,
                'reason': 'absolute binary exists, not executed' if path.is_file() else 'absolute binary missing'}
    if '/' in program or '\\' in program:
        candidate = (root / path).resolve()
        try:
            candidate.relative_to(root)
        except ValueError:
            return {'status': 'UNAVAILABLE', 'path': None, 'reason': 'relative executable escapes project root'}
        return {'status': 'RESOLVED' if candidate.is_file() else 'UNAVAILABLE',
                'path': str(candidate) if candidate.is_file() else None,
                'reason': 'project-local binary exists, not executed' if candidate.is_file() else 'project-local binary missing'}
    found = shutil.which(program)
    return {'status': 'RESOLVED' if found else 'UNAVAILABLE', 'path': found,
            'reason': 'found on PATH, not executed' if found else 'executable not on PATH'}


def inspect_gate(root: Path, key: str = 'full', *, rust_consumer: bool = False) -> dict[str, Any]:
    """Plan only. Legacy policy compatibility is reported, not misrepresented as Rust-ready."""
    root = root.expanduser().resolve()
    result: dict[str, Any] = {
        'schema': 'universal.pcc.plan.v1', 'version': VERSION, 'project_root': str(root),
        'gate': key, 'execution_performed': False, 'source_mutated': False,
        'certification': 'UNKNOWN', 'status': 'BLOCKED', 'errors': [], 'warnings': [],
        'stages': [],
    }
    if not root.is_dir():
        result['errors'].append('project root not found')
        return result
    raw, digest, error = _read_manifest(root / 'project.control.json')
    result['manifest_sha256'] = digest
    if error:
        result['errors'].append(error)
        return result
    if raw is None:
        result['errors'].append('project.control.json missing')
        return result
    validated = validate_contract(raw, rust_consumer=rust_consumer)
    result['project_id'] = validated['project_id']
    result['warnings'].extend(validated['warnings'])
    result['errors'].extend(validated['errors'])
    if validated['errors']:
        return result
    matches = [g for g in validated['gates'] if g['key'].casefold() == key.casefold()]
    if len(matches) != 1:
        result['errors'].append(f'gate {key!r} not registered')
        return result
    commands = {c['key'].casefold(): c for c in raw['commands']}
    for index, stage_key in enumerate(matches[0]['stages']):
        command = commands[stage_key.casefold()]
        program = command['program']
        executable = _executable(program, root)
        cwd = command.get('cwd', '.')
        working_dir = (root / cwd).resolve()
        try:
            working_dir.relative_to(root)
            cwd_valid = working_dir.is_dir()
        except ValueError:
            cwd_valid = False
        row = {
            'index': index + 1, 'command_key': command['key'], 'program': program,
            'args': command.get('args', []), 'risk': command.get('risk', 'read_only'),
            'rollback': command.get('rollback', 'none'), 'cancellation': command.get('cancellation', 'cooperative'),
            'resolved_executable': executable['path'], 'executable_status': executable['status'],
            'working_directory': str(working_dir), 'working_directory_exists': cwd_valid,
            'execution_performed': False, 'verified_working': False,
        }
        result['stages'].append(row)
        if executable['status'] != 'RESOLVED':
            result['errors'].append(f'{stage_key}: {executable["reason"]}')
        if not cwd_valid:
            result['errors'].append(f'{stage_key}: working directory unavailable or outside project: {cwd}')
    result['gate_definition_sha256'] = matches[0]['definition_sha256']
    result['status'] = 'BLOCKED' if result['errors'] else 'RESOLVED_NOT_TESTED'
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Read-only project command/gate build-plan preflight')
    parser.add_argument('--root', default='.')
    parser.add_argument('--gate', default='full')
    parser.add_argument('--rust-consumer', action='store_true', help='Enforce typed Rust rollback contract')
    options = parser.parse_args(argv)
    plan = inspect_gate(Path(options.root), options.gate, rust_consumer=options.rust_consumer)
    print(json.dumps(plan, indent=2, sort_keys=True))
    return 0 if plan['status'] == 'RESOLVED_NOT_TESTED' else 2


if __name__ == '__main__':
    raise SystemExit(main())
