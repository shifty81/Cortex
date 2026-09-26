"""Validate one current GUI version against the cumulative release manifest.

Do not hard-code historical patch versions into otherwise unrelated PCC tests.
"""
from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


class GuiVersionContractTests(unittest.TestCase):
    def test_gui_version_is_well_formed_and_matches_cumulative_manifest(self):
        source = (ROOT / "tools/control/CortexPCCGui.py").read_text(encoding="utf-8")
        found = re.findall(r'^GUI_VERSION\s*=\s*"(PCC-GUI-\d+\.\d+\.\d+[a-z]?)"\s*$', source, flags=re.MULTILINE)
        self.assertEqual(len(found), 1, "exactly one canonical GUI_VERSION declaration is required")
        manifest = json.loads((ROOT / "docs/R8J_B_PATCH_MANIFEST.json").read_text(encoding="utf-8"))
        self.assertEqual(found[0], manifest["visible_gui_version"])

    def test_unrelated_gui_tests_do_not_pin_an_old_patch_version(self):
        for path in (
            "tools/control/tests/test_cortex_pcc.py",
            "tools/control/tests/test_pcc_console_chat_runtime.py",
            "tools/control/tests/test_pcc_failure_evidence_performance.py",
        ):
            with self.subTest(path=path):
                source = (ROOT / path).read_text(encoding="utf-8")
                self.assertNotRegex(source, r'PCC-GUI-\d+\.\d+\.\d+[a-z]')


if __name__ == "__main__":
    unittest.main()
