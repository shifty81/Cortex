"""Optional, non-mutating R8A certification entry point.

Does not alter PCC gate configuration, scan a real volume, or write reports.
"""
from __future__ import annotations
import argparse
from pathlib import Path
import subprocess
import sys


def main(argv=None):
    parser = argparse.ArgumentParser(description='Run the R8A fixture tests without scanning a real volume')
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args(argv)
    root = args.root.resolve(strict=True)
    script = root / 'tools/control/CortexVolumeInventory.py'
    tests = root / 'tests/test_volume_inventory.py'
    if not script.is_file() or not tests.is_file():
        print('R8A inventory or tests missing; no operation performed', file=sys.stderr)
        return 2
    print('R8B: running R8A read-only fixture tests; no real-volume scan', flush=True)
    result = subprocess.run([sys.executable, '-B', '-m', 'unittest', '-v', str(tests)], cwd=root, check=False)
    return result.returncode

if __name__ == '__main__':
    raise SystemExit(main())
