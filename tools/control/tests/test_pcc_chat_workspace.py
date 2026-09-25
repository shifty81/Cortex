from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

from PCCChatStore import list_conversations, load_messages, save_messages


class ChatWorkspaceTests(unittest.TestCase):
    def test_chat_store_isolated_by_workspace_and_conversation(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            cortex = Path(td) / "cortex"
            a = Path(td) / "project-a"
            b = Path(td) / "project-b"
            a.mkdir()
            b.mkdir()
            save_messages(
                cortex, a, "one",
                [{"role": "user", "content": "alpha"}, {"role": "assistant", "content": "A"}],
                updated_unix_ms=1,
            )
            save_messages(
                cortex, a, "two",
                [{"role": "user", "content": "beta"}, {"role": "assistant", "content": "B"}],
                updated_unix_ms=2,
            )
            save_messages(
                cortex, b, "one",
                [{"role": "user", "content": "gamma"}, {"role": "assistant", "content": "C"}],
                updated_unix_ms=3,
            )
            self.assertEqual(load_messages(cortex, a, "one")[0]["content"], "alpha")
            self.assertEqual(load_messages(cortex, b, "one")[0]["content"], "gamma")
            rows = list_conversations(cortex, a)
            self.assertEqual([row["conversationId"] for row in rows], ["two", "one"])

    def test_gui_has_first_class_chat_tab_and_single_bridge_path(self) -> None:
        source = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn('for name in ("Projects", "Chat", "Project Workspace")', source)
        self.assertIn("def _build_chat_tab", source)
        self.assertIn('surface="chat"', source)
        self.assertIn('event_channel="chat"', source)
        self.assertIn("def _chat_history_selected", source)
        self.assertIn("Build Worker", source)

    def test_projects_refresh_does_not_reregister_every_project(self) -> None:
        source = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        start = source.index("    def _refresh_projects")
        end = source.index("    def _selected_project", start)
        body = source[start:end]
        self.assertNotIn("self.registry.register(registered.root", body)

    def test_direct_worker_chat_uses_same_portable_conversation_store(self) -> None:
        source = (CONTROL / "CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertIn("history = load_messages(cortex_root, workspace, conversation_id)", source)
        self.assertIn("save_messages(", source)
        self.assertIn("_run_worker(runtime, cortex_root, workspace, args.conversation_id", source)


if __name__ == "__main__":
    unittest.main()
