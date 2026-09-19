#!/usr/bin/env python3
"""Read-only, fail-closed project/PCC inventory. No application bootstrap or writes.

A01/A03 preflight: this is an evidence collector, not the canonical PCC dispatcher.
Existing project-owned commands, patches, gates and Git are never executed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

VERSION = 'UPCC-A01-0.1'
MAX_MANIFEST_BYTES = 4 * 1024 * 1024
SKIP_DIRS = frozenset({'.git', '.cortex', 'target', 'node_modules', '.venv', 'venv', '__pycache__',
                       'artifacts', 'logs', 'updates', '.gradle', '.idea', '.vs', 'build', 'dist',
                       'out', '.next', '.cache', 'Library', 'Temp', 'obj', 'bin'})
SUPPORTED_SCHEMA = frozenset({'forge.project.v1', 'cortex.v1'})
RISK = frozenset({'read_only', 'local_mutation', 'external_mutation', 'destructive'})
RUST_ROLLBACK = frozenset({'none', 'snapshot', 'transactional', 'provider_owned'})
KNOWN_LEGACY_ROLLBACK = frozenset({'git_or_snapshot', 'process_stop'})
CANCEL = frozenset({'cooperative', 'bounded_kill', 'not_supported'})
MARKERS = ('project.control.json', 'PROJECT_CONTROL_CENTER.cmd', 'Cargo.toml', 'CMakeLists.txt',
           'pyproject.toml', 'package.json', '.uproject', 'build.gradle', 'build.gradle.kts', 'pom.xml')


def _pairs_unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for k, v in pairs:
        if k in result:
            raise ValueError(f'duplicate JSON key: {k}')
        result[k] = v
    return result


def _read_manifest(path: Path) -> tuple[dict[str, Any] | None, str | None, str | None]:
    try:
        if path.stat().st_size > MAX_MANIFEST_BYTES:
            return None, None, 'manifest exceeds 4 MiB inspection limit'
        content = path.read_bytes()
        parsed = json.loads(content.decode('utf-8-sig'), object_pairs_hook=_pairs_unique)
        if not isinstance(parsed, dict):
            return None, None, 'manifest root must be an object'
        return parsed, hashlib.sha256(content).hexdigest(), None
    except (OSError, UnicodeError, ValueError) as e:
        return None, None, f'manifest cannot be read/parsed: {type(e).__name__}: {e}'


def _git_head(root: Path) -> dict[str, Any]:
    if not (root / '.git').exists():
        return {'state': 'NOT_PRESENT', 'head': None, 'branch': None}
    try:
        result = subprocess.run(['git', '-C', str(root), 'rev-parse', '--verify', 'HEAD'],
                                capture_output=True, text=True, timeout=5, check=False)
        if result.returncode:
            return {'state': 'UNRESOLVED', 'head': None, 'branch': None}
        head = result.stdout.strip()
        if not re.fullmatch(r'[0-9a-fA-F]{40,64}', head):
            return {'state': 'UNRESOLVED', 'head': None, 'branch': None}
        branch = subprocess.run(['git', '-C', str(root), 'symbolic-ref', '--quiet', '--short', 'HEAD'],
                                capture_output=True, text=True, timeout=5, check=False)
        return {'state': 'RESOLVED', 'head': head, 'branch': branch.stdout.strip() if branch.returncode == 0 else None}
    except (OSError, subprocess.TimeoutExpired):
        return {'state': 'UNAVAILABLE', 'head': None, 'branch': None}


def _entries(value: Any, label: str, errors: list[str]) -> list[dict[str, Any]]:
    if value is None:
        return []
    if not isinstance(value, list):
        errors.append(f'{label} must be an array; refusing lossy conversion')
        return []
    result = []
    for index, item in enumerate(value):
        if not isinstance(item, dict):
            errors.append(f'{label}[{index}] must be an object')
        else:
            result.append(item)
    return result


def validate_contract(raw: dict[str, Any], *, rust_consumer: bool = True) -> dict[str, Any]:
    """Validate explicit semantics, preserve source metadata; never normalize by dropping fields."""
    errors: list[str] = []
    warnings: list[str] = []
    schema = raw.get('schema')
    if schema is None and raw.get('schema_version') == 1:
        schema = 'cortex.v1'
        warnings.append('legacy numeric schema detected; importer not yet certified')
    if schema not in SUPPORTED_SCHEMA:
        errors.append(f'unsupported schema {schema!r}; original contract retained')

    project = raw.get('project')
    if not isinstance(project, dict) or not isinstance(project.get('id'), str) or not project['id'].strip():
        errors.append('project.id must be a non-empty string')
        project_id = None
    else:
        project_id = project['id']

    commands = _entries(raw.get('commands'), 'commands', errors)
    if 'commands' not in raw:
        warnings.append('no commands registry; project may require a legacy adapter')
    command_keys: dict[str, dict[str, Any]] = {}
    summaries = []
    for index, cmd in enumerate(commands):
        key = cmd.get('key')
        if not isinstance(key, str) or not key.strip():
            errors.append(f'commands[{index}].key must be nonempty')
            continue
        folded = key.casefold()
        if folded in command_keys:
            errors.append(f'duplicate command key: {key!r} conflicts with {command_keys[folded]["key"]!r}')
        else:
            command_keys[folded] = cmd
        program = cmd.get('program')
        if not isinstance(program, str) or not program.strip():
            errors.append(f'command {key}: missing executable/program')
        if 'args' in cmd and (not isinstance(cmd['args'], list) or any(not isinstance(arg, str) for arg in cmd['args'])):
            errors.append(f'command {key}: args must be an array of strings')
        risk = cmd.get('risk', 'read_only')
        if risk not in RISK:
            errors.append(f'command {key}: unsupported risk {risk!r}; execution not authorized')
        cancel = cmd.get('cancellation', 'cooperative')
        if cancel not in CANCEL:
            errors.append(f'command {key}: unsupported cancellation {cancel!r}')
        rollback = cmd.get('rollback', 'none')
        if rollback not in RUST_ROLLBACK:
            if rollback in KNOWN_LEGACY_ROLLBACK and not rust_consumer:
                warnings.append(f'command {key}: legacy rollback {rollback!r} requires semantic adapter')
            else:
                errors.append(f'command {key}: rollback {rollback!r} is incompatible with typed Rust contract; no lexical substitution')
        cwd = cmd.get('cwd', '.')
        if not isinstance(cwd, str) or '\x00' in cwd:
            errors.append(f'command {key}: invalid cwd')
        elif cwd.replace('\\', '/').split('/').count('..'):
            errors.append(f'command {key}: cwd escapes declared project via ..')
        summaries.append({'key': key, 'risk': risk, 'rollback': rollback, 'cancellation': cancel,
                          'registered': True, 'verified_executable': False})

    # Explicit gates can be a list of {key,stages} or a mapping of IDs to {stages}.
    gate_value = raw.get('quality_gates', raw.get('gates'))
    if isinstance(gate_value, dict):
        gates = []
        for key, value in gate_value.items():
            if not isinstance(value, dict):
                errors.append(f'gate {key}: definition must be an object')
            else:
                if 'key' in value and value['key'] != key:
                    errors.append(f'gate {key}: mapped key conflicts with embedded key {value["key"]!r}')
                gates.append({**value, 'key': key})
    else:
        gates = _entries(gate_value, 'quality_gates/gates', errors)
    gate_summaries = []
    gate_keys: set[str] = set()
    for index, gate in enumerate(gates):
        key = gate.get('key')
        if not isinstance(key, str) or not key.strip():
            errors.append(f'gates[{index}].key must be nonempty')
            continue
        if key.casefold() in gate_keys:
            errors.append(f'duplicate gate key: {key!r}')
        gate_keys.add(key.casefold())
        stages = gate.get('stages')
        if not isinstance(stages, list) or not stages:
            errors.append(f'gate {key}: stages must be a nonempty array')
            continue
        stage_keys: list[str] = []
        for index2, stage in enumerate(stages):
            stage_key = stage if isinstance(stage, str) else stage.get('key') if isinstance(stage, dict) else None
            if not isinstance(stage_key, str) or not stage_key.strip():
                errors.append(f'gate {key}: stage[{index2}] must declare an exact command key')
                continue
            stage_keys.append(stage_key)
            if stage_key.casefold() not in command_keys:
                errors.append(f'gate {key}: unresolved required stage {stage_key!r}')
        gate_digest = hashlib.sha256(json.dumps(gate, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode('utf-8')).hexdigest()
        gate_summaries.append({'key': key, 'stages': stage_keys, 'stage_count': len(stage_keys),
                               'definition_sha256': gate_digest, 'metadata_keys': sorted(gate)})
    if gate_value is None:
        warnings.append('no explicit quality gates; not certified as buildable')

    # Do not assert capability, provider, gate or source parity based on declared values.
    return {'schema': schema, 'project_id': project_id, 'status': 'INVALID' if errors else 'DECLARED',
            'errors': errors, 'warnings': warnings, 'commands': summaries, 'gates': gate_summaries,
            'command_count': len(commands), 'gate_count': len(gates),
            'original_fields': sorted(raw), 'execution_performed': False, 'source_certified': False}


def inspect_project(root: Path, *, rust_consumer: bool = True) -> dict[str, Any]:
    root = root.expanduser().resolve()
    info: dict[str, Any] = {'root': str(root), 'audit_version': VERSION, 'inspection': 'READ_ONLY',
                            'git': _git_head(root) if root.is_dir() else {'state': 'MISSING', 'head': None, 'branch': None}}
    if not root.is_dir():
        return {**info, 'status': 'MISSING', 'contract': None, 'errors': ['project root is not a directory']}
    info['markers'] = sorted(marker for marker in MARKERS if (root / marker).is_file())
    info['launcher'] = 'PROJECT_CONTROL_CENTER.cmd' if (root / 'PROJECT_CONTROL_CENTER.cmd').is_file() else None
    manifest = root / 'project.control.json'
    if not manifest.is_file():
        return {**info, 'status': 'UNMANAGED', 'contract': None, 'errors': [],
                'notes': ['no root project.control.json; cannot infer or certify native PCC command registry']}
    parsed, digest, error = _read_manifest(manifest)
    info['manifest_sha256'] = digest
    if error:
        return {**info, 'status': 'INVALID', 'contract': None, 'errors': [error]}
    assert parsed is not None
    contract = validate_contract(parsed, rust_consumer=rust_consumer)
    info['contract'] = contract
    info['status'] = contract['status']
    info['errors'] = contract['errors']
    # Not a governed source fingerprint. An exact manifest hash is all that is supported here.
    info['source_fingerprint'] = None
    info['runtime_tested'] = False
    return info


def discover(scan_root: Path, max_depth: int, max_dirs: int, max_projects: int) -> dict[str, Any]:
    """Bounded metadata-only discovery; no following symlinks, no file-content indexing."""
    scan_root = scan_root.expanduser().resolve()
    found: list[Path] = []
    inspected = 0
    overflow = False
    errors: list[str] = []
    pending: list[tuple[Path, int]] = [(scan_root, 0)]
    while pending:
        if inspected >= max_dirs or len(found) >= max_projects:
            overflow = True
            break
        path, depth = pending.pop()
        inspected += 1
        try:
            with os.scandir(path) as entries:
                items = sorted(entries, key=lambda item: item.name.casefold())
        except OSError as e:
            errors.append(f'{path}: {type(e).__name__}')
            continue
        if any(item.name == 'project.control.json' and item.is_file(follow_symlinks=False) for item in items):
            found.append(path)
        if depth >= max_depth:
            if any(item.is_dir(follow_symlinks=False) and item.name.casefold() not in {name.casefold() for name in SKIP_DIRS} for item in items):
                overflow = True
            continue
        children = [Path(item.path) for item in items if item.is_dir(follow_symlinks=False)
                    and item.name.casefold() not in {name.casefold() for name in SKIP_DIRS} and not item.name.startswith('.')]
        pending.extend((child, depth + 1) for child in reversed(children))
    return {'scan_root': str(scan_root), 'directories_examined': inspected, 'max_depth': max_depth,
            'max_directories': max_dirs, 'max_projects': max_projects, 'incomplete': overflow or bool(errors),
            'discovered_roots': [str(path) for path in found], 'scan_errors': errors[:50],
            'no_mutation': True}


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description='Read-only PCC implementation inventory and semantic contract preflight')
    p.add_argument('action', choices=['inspect', 'inventory'])
    p.add_argument('--root', action='append', default=[], help='Project root, repeatable')
    p.add_argument('--scan-root', action='append', default=[], help='Explicitly enable bounded recursive discovery')
    p.add_argument('--max-depth', type=int, default=5)
    p.add_argument('--max-directories', type=int, default=12000)
    p.add_argument('--max-projects', type=int, default=500)
    p.add_argument('--legacy-only', action='store_true', help='Report typed Rust enum incompatibilities as warnings, never auto-translate')
    args = p.parse_args(argv)
    if not args.root and not args.scan_root:
        p.error('one or more --root or --scan-root entries are required')
    if not (0 <= args.max_depth <= 32 and 1 <= args.max_directories <= 100000 and 1 <= args.max_projects <= 10000):
        p.error('scan bounds outside supported range')
    scans = [discover(Path(r), args.max_depth, args.max_directories, args.max_projects) for r in args.scan_root]
    roots = sorted({str(Path(r).expanduser().resolve()) for r in args.root}.union(
        root for scan in scans for root in scan['discovered_roots']), key=str.casefold)
    projects = [inspect_project(Path(root), rust_consumer=not args.legacy_only) for root in roots]
    summary = {'project_count': len(projects), 'invalid': sum(p['status'] in {'INVALID', 'MISSING'} for p in projects),
               'unmanaged': sum(p['status'] == 'UNMANAGED' for p in projects),
               'scan_incomplete': any(scan['incomplete'] for scan in scans)}
    print(json.dumps({'schema': 'upcc.audit.v1', 'version': VERSION, 'summary': summary,
                      'scans': scans, 'projects': projects, 'read_only': True}, indent=2, sort_keys=True))
    return 2 if summary['invalid'] or summary['scan_incomplete'] else 0


if __name__ == '__main__':
    raise SystemExit(main())
