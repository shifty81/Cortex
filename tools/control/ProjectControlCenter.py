#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path
from typing import Sequence


def project_root() -> Path:
    return Path(__file__).resolve().parents[2]


def run(argv: list[str], root: Path) -> int:
    print('[INFO] START ' + subprocess.list2cmdline(argv))
    cp = subprocess.run(argv, cwd=str(root), check=False)
    print(('[PASS]' if cp.returncode == 0 else '[FAIL]') + f' END exit={cp.returncode}')
    return int(cp.returncode)


def run_forge_rust(command: str, root: Path) -> int:
    manifest = root / 'products' / 'forge-rust' / 'Cargo.toml'
    if not manifest.is_file():
        print(f'[FAIL] Rust Forge workspace is missing: {manifest}', file=sys.stderr)
        return 2

    # `--manifest-path` belongs to each Cargo subcommand, not to the top-level
    # `cargo` invocation. Keep the canonical ordering here so the project-side
    # provider behaves exactly like the forge.project.v1 command descriptors.
    stages: dict[str, list[str]] = {
        'forge-rust-fmt': [
            'cargo', 'fmt', '--manifest-path', str(manifest), '--all', '--', '--check'
        ],
        'forge-rust-check': [
            'cargo', 'check', '--manifest-path', str(manifest), '--workspace', '--all-targets'
        ],
        'forge-rust-test': [
            'cargo', 'test', '--manifest-path', str(manifest), '--workspace', '--all-targets'
        ],
        'forge-rust-clippy': [
            'cargo', 'clippy', '--manifest-path', str(manifest), '--workspace', '--all-targets', '--', '-D', 'warnings'
        ],
        'forge-rust-build': [
            'cargo', 'build', '--manifest-path', str(manifest), '-p', 'forge-rs'
        ],
        'forge-rust-run': [
            'cargo', 'run', '--manifest-path', str(manifest), '-p', 'forge-rs', '--', '--root', str(root)
        ],
    }
    if command == 'forge-rust-gate':
        for stage in (
            'forge-rust-fmt',
            'forge-rust-check',
            'forge-rust-test',
            'forge-rust-clippy',
            'forge-rust-build',
        ):
            rc = run(stages[stage], root)
            if rc != 0:
                return rc
        return 0
    argv = stages.get(command)
    if argv is None:
        return -1
    return run(argv, root)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('command', nargs='?', default='interactive')
    parser.add_argument('--root')
    known, rest = parser.parse_known_args(argv)
    root = Path(known.root).expanduser().resolve() if known.root else project_root()
    rust_rc = run_forge_rust(known.command, root)
    if rust_rc >= 0:
        return rust_rc

    # Forward every existing Cortex project-native command unchanged.
    control_dir = str((root / 'tools' / 'control').resolve())
    if control_dir not in sys.path:
        sys.path.insert(0, control_dir)
    from CortexPCC import main as cortex_main
    forwarded = [known.command, *rest]
    if known.root:
        forwarded += ['--root', str(root)]
    return int(cortex_main(forwarded))


if __name__ == '__main__':
    raise SystemExit(main())
