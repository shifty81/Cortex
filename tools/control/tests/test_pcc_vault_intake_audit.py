from __future__ import annotations

import json
import os
import tempfile
import unittest
import zipfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
import sys
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

from PCCVaultIntakeAudit import create_handoff, latest_summary, scan_intake, status


class VaultIntakeAuditTests(unittest.TestCase):
    def test_scan_classifies_projects_assets_duplicates_and_needs_sorted(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            vault = root / "Vault"
            intake = vault / "Intake"
            needs = intake / "Needs Sorted"
            project = needs / "OldProject"
            assets = intake / "Loose Assets"
            project.mkdir(parents=True)
            assets.mkdir(parents=True)

            (project / "Cargo.toml").write_text('[package]\nname="old_project"\nversion="0.1.0"\n', encoding="utf-8")
            (project / "README.md").write_text("old project\n", encoding="utf-8")
            (project / "LICENSE").write_text("license\n", encoding="utf-8")
            (assets / "grass.png").write_bytes(b"\x89PNG\r\n\x1a\n" + b"x" * 64)
            (needs / "dupe-a.txt").write_text("same duplicate content\n", encoding="utf-8")
            (needs / "dupe-b.txt").write_text("same duplicate content\n", encoding="utf-8")
            (needs / "mystery.zzq").write_bytes(b"mystery")

            archive = intake / "project-backup.zip"
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zf:
                zf.writestr("ArchivedThing/CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\n")
                zf.writestr("ArchivedThing/assets/thing.png", b"fake")

            old_env = os.environ.get("CORTEX_VAULT_ROOT")
            os.environ["CORTEX_VAULT_ROOT"] = str(vault)
            try:
                before = {p.relative_to(intake).as_posix(): p.read_bytes() for p in intake.rglob("*") if p.is_file()}
                summary = scan_intake(root, intake=intake)
                after = {p.relative_to(intake).as_posix(): p.read_bytes() for p in intake.rglob("*") if p.is_file()}

                self.assertEqual(before, after, "audit must not mutate intake content")
                self.assertEqual(summary["status"], "PASS")
                self.assertGreaterEqual(summary["files"], 7)
                self.assertGreaterEqual(summary["projectCandidates"], 1)
                self.assertGreaterEqual(summary["exactDuplicateGroups"], 1)
                self.assertGreaterEqual(summary["archiveInventories"], 1)
                self.assertGreaterEqual(summary["unknownFiles"], 1)
                self.assertGreater(summary["zoneCounts"].get("needs_sorted", 0), 0)
                self.assertFalse(summary["safety"]["moves"])
                self.assertFalse(summary["safety"]["deletes"])
                self.assertFalse(summary["safety"]["archiveExtraction"])

                again = scan_intake(root, intake=intake)
                self.assertGreater(again["reusedMetadata"], 0)
                self.assertGreaterEqual(again["archiveInventories"], 1)

                handoff = create_handoff(root)
                self.assertTrue(handoff.is_file())
                with zipfile.ZipFile(handoff) as zf:
                    names = set(zf.namelist())
                    self.assertIn("HANDOFF.md", names)
                    self.assertIn("intake-summary.json", names)
                    self.assertIn("project-candidates.json", names)
                    self.assertIn("exact-duplicates.json", names)
                    self.assertIn("unknown-review.json", names)
                    payload = json.loads(zf.read("intake-summary.json"))
                    self.assertEqual(payload["scanId"], again["scanId"])

                current = latest_summary(root)
                self.assertIsNotNone(current)
                state = status(root)
                self.assertEqual(Path(state["intakeRoot"]), intake.resolve())
                self.assertEqual(Path(state["latestHandoff"]), handoff)
            finally:
                if old_env is None:
                    os.environ.pop("CORTEX_VAULT_ROOT", None)
                else:
                    os.environ["CORTEX_VAULT_ROOT"] = old_env


if __name__ == "__main__":
    unittest.main()
