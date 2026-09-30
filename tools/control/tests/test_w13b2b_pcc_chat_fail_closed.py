from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPythonBridge as bridge
import CortexPCCGui as gui


class W13B2BPCCFailClosed(unittest.TestCase):
    def test_creation_and_approval_require_worker(self) -> None:
        for prompt in (
            "Create a new standalone Rust Hello World project named CortexHelloWorld",
            "please build the project and run the executable",
            "Can you create a new application?",
            "yes", "go ahead", "continue",
        ):
            with self.subTest(prompt=prompt):
                self.assertTrue(bridge._chat_requires_worker(prompt))
        self.assertFalse(bridge._chat_requires_worker("What is a Rust binary?"))
        self.assertFalse(bridge._chat_requires_worker("Explain how cargo works."))

    def test_common_chat_worker_failure_never_uses_python_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as folder, \
             mock.patch.object(bridge, "_worker_request_text", return_value="request"), \
             mock.patch.object(bridge, "run_controller", return_value=SimpleNamespace(returncode=7, stdout="controller error", stderr="")) as run, \
             mock.patch.object(bridge, "_python_model", side_effect=AssertionError("must not fall back")), \
             mock.patch.object(bridge, "save_messages", side_effect=AssertionError("must not save fake response")):
            root = Path(folder)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = bridge._run_worker(root / "cortex.exe", root, root, "test-id", "chat", "Create a new project")
            self.assertEqual(rc, 7)
            self.assertIn("pcc-chat failed", out.getvalue())
            self.assertIn("NOT certified", out.getvalue())
            self.assertIn("pcc-chat", run.call_args.args[0])

    def test_invalid_pcc_chat_payload_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as folder, \
             mock.patch.object(bridge, "_worker_request_text", return_value="request"), \
             mock.patch.object(bridge, "run_controller", return_value=SimpleNamespace(returncode=0, stdout="not-json", stderr="")), \
             mock.patch.object(bridge, "_python_model", side_effect=AssertionError("must not fall back")), \
             mock.patch.object(bridge, "save_messages", side_effect=AssertionError("must not save invalid result")):
            root = Path(folder)
            with contextlib.redirect_stdout(io.StringIO()) as out:
                rc = bridge._run_worker(root / "cortex.exe", root, root, "test-id", "chat", "Hello")
            self.assertEqual(rc, 2)
            self.assertIn("invalid JSON", out.getvalue())

    def test_read_only_inspect_may_retain_nonmutating_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as folder, \
             mock.patch.object(bridge, "_worker_request_text", return_value="request"), \
             mock.patch.object(bridge, "run_controller", return_value=SimpleNamespace(returncode=6, stdout="", stderr="worker missing")), \
             mock.patch.object(bridge, "_python_model", return_value=0) as fallback:
            root = Path(folder)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = bridge._run_worker(root / "cortex.exe", root, root, "test-id", "inspect", "What is this project?")
            self.assertEqual(rc, 0)
            fallback.assert_called_once()

    def test_missing_worker_blocks_project_creation_before_provider_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as folder, \
             mock.patch.object(bridge, "_ensure_provider_ready", return_value=True), \
             mock.patch.object(bridge, "_resolve_runtime", return_value=None), \
             mock.patch.object(bridge, "_python_model", side_effect=AssertionError("must not fall back")), \
             mock.patch.object(sys, "argv", ["CortexPythonBridge.py", "--cortex-root", folder,
                "--workspace", folder, "--conversation-id", "id1", "chat",
                "Create a new standalone Rust Hello World project"]):
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = bridge.main()
            self.assertEqual(rc, 3)
            self.assertIn("worker is unavailable", out.getvalue())
            self.assertIn("No project was created", out.getvalue())

    def test_chat_ui_identifies_governed_mode_truthfully(self) -> None:
        messages = []
        fake = SimpleNamespace(
            chat_input=SimpleNamespace(get=lambda *_: "Create a new Rust project", delete=lambda *_: None),
            chat_mode_var=SimpleNamespace(get=lambda: "Chat"),
            root_path=Path("project"),
            _append_chat_message=lambda *args: messages.append(args),
            _start_cortex_cli=lambda *args, **kw: messages.append(("launch", args, kw)),
        )
        self.assertEqual(gui.CortexPCCGui._submit_chat_input(fake), "break")
        self.assertIn("common governed developer controller", messages[1][1])
        self.assertNotIn("non-mutating", messages[1][1])


if __name__ == "__main__":
    unittest.main()
