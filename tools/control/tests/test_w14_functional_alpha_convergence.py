from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[1]
ROOT = CONTROL.parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPCCMaintenance as maintenance
import CortexRepositoryLayout as layout
from PCCChatStore import workspace_key


class FunctionalAlphaConvergenceTests(unittest.TestCase):
    def test_pcc_chat_surface_reports_live_execution_and_truthful_modes(self) -> None:
        source = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn("self.chat_live_label", source)
        self.assertIn("[CortexLive]", source)
        self.assertIn("_friendly_cortex_stage", source)
        self.assertIn("Chat can inspect or perform governed project work", source)
        self.assertNotIn("Chat is read-only", source)
        self.assertNotIn("No automatic builds", source)
        self.assertIn('if line.startswith("[CortexStage]")', source)
        self.assertIn('if line.startswith("[CortexLive]")', source)

    def test_agent_runtime_no_longer_spams_no_response_heartbeat(self) -> None:
        source = (CONTROL / "CortexAgentRuntime.py").read_text(encoding="utf-8")
        self.assertIn("_execution_live_status_for_owner", source)
        self.assertIn("[CortexLive]", source)
        self.assertNotIn("no completed response yet", source)


    def test_patch_apply_is_safe_from_embedded_command_registry(self) -> None:
        pcc = (CONTROL / "CortexPCC.py").read_text(encoding="utf-8")
        gui = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn("Patch apply requires explicit --yes when stdin is non-interactive.", pcc)
        self.assertIn("except EOFError:", pcc)
        self.assertIn('if row.key == "patch-apply":', gui)
        self.assertIn('self._start_command(row.key, ["--yes"], label=row.label)', gui)

    def test_storage_policy_has_no_fixed_windows_drive_and_registry_is_runtime_state(self) -> None:
        policy = json.loads((ROOT / "config/cortex/storage_policy.v1.json").read_text(encoding="utf-8"))
        self.assertEqual(policy.get("default_library_root_windows"), "")
        self.assertEqual(policy["vault"]["portable_registry"], ".cortex/registry/project_registry.json")
        self.assertNotIn("D:\\", json.dumps({
            "default": policy.get("default_library_root_windows"),
            "registry": policy["vault"]["portable_registry"],
        }))

    def test_canonical_layout_and_pcc_hygiene_share_one_authority(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "Cortex"
            policy_dir = root / "config" / "cortex"
            policy_dir.mkdir(parents=True)
            shutil.copy2(ROOT / "config/cortex/repository_layout.v1.json", policy_dir / "repository_layout.v1.json")
            (root / "APPLY_OLD_FIX.ps1").write_text("# old", encoding="utf-8")
            workspace = Path(td) / "project"
            workspace.mkdir()
            legacy = root / "data" / "conversations" / "pcc" / workspace_key(workspace) / "legacy.json"
            legacy.parent.mkdir(parents=True)
            legacy.write_text(
                json.dumps({
                    "schema": "cortex.python_bridge.conversation.v2",
                    "conversationId": "legacy",
                    "updatedUnixMs": 1,
                    "messages": [{"role": "user", "content": "hello"}],
                }),
                encoding="utf-8",
            )
            canonical = layout.scan_repository_layout(root)
            pcc = maintenance.scan_root_hygiene(root)
            self.assertFalse(canonical["clean"])
            self.assertFalse(pcc["clean"])
            repaired = maintenance.repair_root_hygiene(root)
            self.assertEqual(repaired["after"]["violationCount"], 0)
            self.assertEqual(repaired["runtimeMigration"]["migrated"], 1)
            self.assertTrue((root / ".cortex" / "conversations" / "pcc" / workspace_key(workspace) / "legacy.json").is_file())
            self.assertTrue(any(row["from"] == "APPLY_OLD_FIX.ps1" for row in repaired["moved"]))
            self.assertTrue(layout.scan_repository_layout(root)["clean"])


    def test_generated_target_is_allowed_and_legacy_volume_runtime_is_migrated(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            volume = Path(td)
            root = volume / "Cortex"
            policy_dir = root / "config" / "cortex"
            policy_dir.mkdir(parents=True)
            shutil.copy2(ROOT / "config/cortex/repository_layout.v1.json", policy_dir / "repository_layout.v1.json")
            (root / "target" / "debug").mkdir(parents=True)
            (root / "target" / "debug" / "cortex.exe").write_bytes(b"generated")
            for name in ("Memory", "Search", "Tasks"):
                directory = root / name / "nested"
                directory.mkdir(parents=True)
                (directory / f"{name.lower()}.json").write_text(f"{{\"kind\":\"{name}\"}}", encoding="utf-8")

            before = layout.scan_repository_layout(root)
            self.assertEqual(before["violationCount"], 3)
            self.assertTrue(all(row["kind"] == "runtime-in-repository" for row in before["items"]))
            self.assertFalse(any(row["path"].startswith("target") for row in before["items"]))

            repaired = maintenance.repair_root_hygiene(root)
            self.assertEqual(repaired["after"]["violationCount"], 0)
            self.assertEqual(repaired["volumeRuntimeMigration"]["migrated"], 3)
            for name in ("Memory", "Search", "Tasks"):
                self.assertFalse((root / name).exists())
                self.assertTrue((volume / ".cortex" / "data" / name / "nested" / f"{name.lower()}.json").is_file())
            self.assertTrue((root / "target" / "debug" / "cortex.exe").is_file())

    def test_volume_runtime_collision_fails_closed_and_preserves_legacy_source(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            volume = Path(td)
            root = volume / "Cortex"
            policy_dir = root / "config" / "cortex"
            policy_dir.mkdir(parents=True)
            shutil.copy2(ROOT / "config/cortex/repository_layout.v1.json", policy_dir / "repository_layout.v1.json")
            legacy = root / "Memory" / "state.bin"
            legacy.parent.mkdir(parents=True)
            legacy.write_bytes(b"legacy")
            canonical = volume / ".cortex" / "data" / "Memory" / "state.bin"
            canonical.parent.mkdir(parents=True)
            canonical.write_bytes(b"different")

            repaired = maintenance.repair_root_hygiene(root)
            self.assertGreater(repaired["after"]["violationCount"], 0)
            self.assertTrue(legacy.is_file())
            self.assertEqual(legacy.read_bytes(), b"legacy")
            self.assertEqual(canonical.read_bytes(), b"different")
            self.assertIn("error", repaired["volumeRuntimeMigration"])

    def test_pcc_no_longer_offers_native_desktop_as_build_product(self) -> None:
        source = (CONTROL / "CortexPCC.py").read_text(encoding="utf-8")
        start = source.index("    def build_menu(self) -> None:")
        end = source.index("    def diagnostics_menu", start)
        block = source[start:end]
        self.assertNotIn("Build Cortex Desktop package", block)
        self.assertNotIn('self.build(package="cortex_desktop")', block)
        self.assertIn("Launch Cortex GUI / Project Control", block)

    def test_root_readme_describes_current_pcc_and_forge_direction(self) -> None:
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        self.assertIn("PROJECT_CONTROL_CENTER.cmd", readme)
        self.assertIn("Forge Rust", readme)
        self.assertIn("Live execution visibility", readme)
        self.assertNotIn("Cortex PCC Build-Gate Recovery R1", readme)


if __name__ == "__main__":
    unittest.main()
