"""Tests for the optional R8B gate wrapper; no real-volume scans."""
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

RUNNER = Path(__file__).resolve().parents[1] / 'tools/control/CortexVolumeInventoryGate.py'

class GateTests(unittest.TestCase):
    def test_missing_sources_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run([sys.executable, '-B', str(RUNNER), '--root', tmp], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('no operation performed', result.stderr)
    def test_no_real_volume_scan_in_runner(self):
        source = RUNNER.read_text(encoding='utf-8')
        self.assertNotIn('inventory(root', source)
        self.assertNotIn('vault_catalog.db', source)

if __name__ == '__main__':
    unittest.main()
