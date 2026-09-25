from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
GUI = ROOT / "tools" / "control" / "CortexPCCGui.py"


class PCCStatusGenerationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = GUI.read_text(encoding="utf-8")

    def test_status_refresh_is_generation_guarded(self) -> None:
        self.assertIn("self._status_generation += 1", self.source)
        self.assertIn('(\"status\", (generation, status))', self.source)
        self.assertIn("if int(generation) != self._status_generation:", self.source)

    def test_successful_full_marks_green_as_verifying_before_readback(self) -> None:
        self.assertIn('str(command).casefold() == "full" and int(rc) == 0', self.source)
        self.assertIn('self._set_status_card("GREEN", "Verifying", CYAN)', self.source)
        self.assertIn('self._set_status_card("Git", "Refreshing", CYAN)', self.source)


if __name__ == "__main__":
    unittest.main()
