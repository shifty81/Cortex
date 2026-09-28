from __future__ import annotations

from pathlib import Path
import unittest

CONTROL = Path(__file__).resolve().parents[1]
ROOT = CONTROL.parents[1]


class W13B2PCCOnlyCortexTests(unittest.TestCase):
    def test_chat_does_not_pre_route_natural_create_to_repair(self) -> None:
        source = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        block = source[source.index("def _chat_execution_intent"):source.index("class CortexPCCGui") ]
        self.assertNotIn('return "repair", value', block)
        self.assertIn("common Cortex developer controller", block)

    def test_bridge_uses_common_pcc_chat_command(self) -> None:
        source = (CONTROL / "CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertIn('"pcc-chat", prompt, "--json"', source)
        self.assertIn('[CortexWorkspace]', source)
        self.assertIn('print(f"[CortexConversation] {conversation_id}"', source)

    def test_console_does_not_offer_legacy_desktop_launch(self) -> None:
        source = (CONTROL / "CortexPCCConsole.py").read_text(encoding="utf-8")
        self.assertNotIn("Launch Cortex Desktop", source)
        self.assertIn("PCC / CLI-first", source)

    def test_launch_gui_alias_targets_pcc_not_native_desktop(self) -> None:
        source = (CONTROL / "CortexPCC.py").read_text(encoding="utf-8")
        start = source.index("    def launch_gui(self) -> int:")
        end = source.index("    def startup(self) -> None:", start)
        block = source[start:end]
        self.assertIn("CortexPCCGui.py", block)
        self.assertNotIn('self.binary_path("cortex_desktop"', block)
        self.assertNotIn("cargo build -p cortex_desktop", block)

    def test_cli_exposes_pcc_chat(self) -> None:
        source = (ROOT / "crates" / "cortex_cli" / "src" / "lib.rs").read_text(encoding="utf-8")
        self.assertIn('"pcc-chat" => pcc_chat_command', source)
        self.assertIn("DesktopController::open_embedded", source)


if __name__ == "__main__":
    unittest.main()
