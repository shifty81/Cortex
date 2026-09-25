from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import PCCBuildDoctor as doctor
import PCCSharedEnvironment as shared


class SelfHostingRecoveryTests(unittest.TestCase):
    def test_build_doctor_classifies_msvc_environment_failure(self):
        payload = doctor.classify_failure(
            "[FAIL] Windows linker toolchain: MSVC amd64 developer environment is not active (link.exe/cl.exe missing).",
            stage="windows-linker-toolchain",
        )
        self.assertEqual(payload["category"], "ENVIRONMENT")
        self.assertEqual(payload["repairScope"], "toolchain_environment")
        self.assertEqual(payload["capability"], "windows.msvc.amd64")
        self.assertFalse(payload["sourceImplicated"])

    def test_build_doctor_classifies_git_trust_separately(self):
        payload = doctor.classify_failure("fatal: detected dubious ownership in repository at 'G:/Cortex'")
        self.assertEqual(payload["category"], "GIT")
        self.assertEqual(payload["code"], "git_checkout_trust")
        self.assertFalse(payload["sourceImplicated"])

    def test_latest_failure_reads_existing_logs_without_writing_source(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            logs = root / "artifacts" / "logs"
            logs.mkdir(parents=True)
            (logs / "sample.log").write_text(
                "[FAIL] QUICK PROJECT GATE FAILED at windows-linker-toolchain: link.exe/cl.exe missing\n",
                encoding="utf-8",
            )
            payload = doctor.latest_failure(root)
            self.assertEqual(payload["status"], "FAILURE_CLASSIFIED")
            self.assertEqual(payload["classification"]["category"], "ENVIRONMENT")

    def test_toolchain_broker_is_bounded_and_returns_report(self):
        with tempfile.TemporaryDirectory() as td:
            env, report = shared.resolve_toolchain_environment(Path(td), {}, include_msvc=False, timeout=0.1)
            self.assertEqual(report["schema"], "pcc.toolchain_resolution.v1")
            self.assertIn("elapsedMs", report)
            self.assertIn("tools", report)
            self.assertEqual(env.get("PYTHONUNBUFFERED"), "1")

    def test_python_bridge_does_not_invoke_cargo_for_runtime_discovery(self):
        source = (TOOLS / "CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertNotIn('"cargo", "metadata"', source)
        self.assertNotIn('"cargo", "run"', source)
        self.assertIn('mode in {"chat", "inspect", "plan"}', source)
        self.assertIn("PCCBuildDoctor", source)

    def test_python_bridge_conversations_are_isolated_by_explicit_id(self):
        source = (TOOLS / "CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertIn('parser.add_argument("--conversation-id"', source)
        self.assertIn("load_messages", source)
        self.assertIn("save_messages", source)

    def test_python_bridge_fallback_does_not_prompt_model_to_lead_with_limitations(self):
        source = (TOOLS / "CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertIn("Do not lead with capability disclaimers", source)
        self.assertIn("do not discuss hidden/system prompts", source)
        self.assertIn("Current user request", source)

    def test_gui_exposes_newchat_and_passes_conversation_id(self):
        source = (TOOLS / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn('"--conversation-id", self._cortex_conversation_id()', source)
        self.assertIn('/newchat', source)
        self.assertIn('def _new_cortex_conversation', source)

    def test_patch_apply_no_longer_auto_confirms(self):
        source = (TOOLS / "PCCAutoAdapter.py").read_text(encoding="utf-8")
        self.assertNotIn('assume_yes or command == "patch-apply"', source)


if __name__ == "__main__":
    unittest.main()
