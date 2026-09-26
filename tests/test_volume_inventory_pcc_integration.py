"""Verify inventory fixtures are mandatory in the authoritative full gate."""
from pathlib import Path
import unittest


class InventoryFullGateContractTests(unittest.TestCase):
    def test_inventory_fixtures_are_mandatory(self):
        source = (Path(__file__).resolve().parents[1] / "tools/control/CortexPCC.py").read_text(encoding="utf-8")
        start = source.index("def run_universal_python_regressions(")
        end = source.index("\ndef run_self_tests(", start)
        gate = source[start:end]
        self.assertIn('root / "tests/test_volume_inventory.py",', gate)
        self.assertIn('root / "tests/test_volume_inventory_gate.py",', gate)
        self.assertIn('missing = [str(p.relative_to(root))', gate)


if __name__ == "__main__":
    unittest.main()
