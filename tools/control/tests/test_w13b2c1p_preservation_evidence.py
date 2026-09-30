from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
ROOT = CONTROL.parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexAgentCLI as cli
import CortexAgentEvidence as evidence
import CortexCapabilityGuard as guard
from PCCChatStore import save_messages


class CortexPreservationEvidenceTests(unittest.TestCase):
    def test_current_source_retains_declared_cli_and_pcc_entry_points(self) -> None:
        manifest = json.loads((ROOT / guard.MANIFEST).read_text(encoding="utf-8"))
        report = guard.audit(ROOT, manifest)
        self.assertEqual(report["status"], "PASS", report["failures"])
        self.assertGreaterEqual(report["checkedFiles"], 10)

    def test_guard_fails_closed_on_removed_entry_point_or_file(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            module = root / "existing.py"
            module.write_text("def retained(): pass\n", encoding="utf-8")
            report = guard.audit(root, {"checks": [
                {"path": "existing.py", "definitions": ["retained", "missing"]},
                {"path": "absent.py", "definitions": ["anything"]},
            ]})
            self.assertEqual(report["status"], "FAIL")
            self.assertEqual(len(report["failures"]), 2)

    def test_pcc_cache_listing_and_history_are_read_only(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            project = root / "project"
            project.mkdir()
            save_messages(root, project, "gui-session-1", [
                {"role": "user", "content": "Help with this project"},
                {"role": "assistant", "content": "Status only"},
            ], updated_unix_ms=1234)
            results = evidence.conversations(root, project)
            self.assertEqual(results[0]["conversationId"], "gui-session-1")
            before = (root / "data" / "conversations").rglob("*.json")
            paths_before = {p: p.read_bytes() for p in before}
            record = evidence.history(root, project, "gui-session-1")
            self.assertEqual(record["messages"][0]["content"], "Help with this project")
            self.assertIsNone(evidence.history(root, project, "../gui-session-1"))
            self.assertIsNone(evidence.history(root, project, "does-not-exist"))
            self.assertEqual(paths_before, {p: p.read_bytes() for p in paths_before})

    def test_operation_trace_reports_actual_finished_exit_code(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            folder = evidence.operation_directory(root)
            folder.mkdir(parents=True)
            identifier = "a" * 32
            path = folder / f"20260929T120000Z-{identifier}.jsonl"
            rows = [
                {"schema": "cortex.agent.operation.v1", "utc": "2026-09-29T12:00:00Z",
                 "operationId": identifier, "kind": "started", "command": "pcc-chat", "workspace": "G:/Cortex"},
                {"schema": "cortex.agent.operation.v1", "utc": "2026-09-29T12:00:01Z",
                 "operationId": identifier, "kind": "finished", "exitCode": 7, "elapsedSeconds": 1},
            ]
            path.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
            self.assertEqual(evidence.recent_operations(root)[0]["exitCode"], 7)
            self.assertIn("finished exit=7", evidence.summarize_operation(evidence.find_operation(root, identifier)))
            self.assertIsNone(evidence.find_operation(root, "../../secrets"))

    def test_incomplete_trace_is_never_presented_as_success(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            folder = evidence.operation_directory(root)
            folder.mkdir(parents=True)
            identifier = "b" * 32
            path = folder / f"20260929T120001Z-{identifier}.jsonl"
            path.write_text(json.dumps({"schema": "cortex.agent.operation.v1", "utc": "now",
                "operationId": identifier, "kind": "started"}) + "\n", encoding="utf-8")
            self.assertIn("completion not recorded", evidence.summarize_operation(evidence.find_operation(root, identifier)))
            path.write_bytes(b"x" * (evidence.MAX_TRACE_BYTES + 1))
            self.assertIsNone(evidence.find_operation(root, identifier))

    def test_terminal_slash_inspection_never_dispatches_agent_work(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workspace = root / "project"
            workspace.mkdir()
            bridge_path = root / "tools" / "control" / "CortexPythonBridge.py"
            bridge_path.parent.mkdir(parents=True)
            bridge_path.write_text("# fixture", encoding="utf-8")
            save_messages(root, workspace, "gui-x", [{"role": "user", "content": "A preview"}], updated_unix_ms=123)
            shown: list[str] = []
            sequence = iter(["/conversations", "/history gui-x", "/operations", "/operation invalid", "/exit"])
            with mock.patch.object(cli.bridge, "_resolve_runtime", return_value=None), \
                 mock.patch.object(cli.bridge, "_provider_is_ready", return_value=False), \
                 mock.patch.object(cli, "dispatch", side_effect=AssertionError("must be read-only")):
                result = cli.main(["--cortex-root", str(root), "--workspace", str(workspace)],
                    input_fn=lambda _: next(sequence), reporter=shown.append)
            self.assertEqual(result, 0)
            self.assertTrue(any("gui-x | A preview" in item for item in shown))
            self.assertTrue(any("[USER] A preview" in item for item in shown))
            self.assertTrue(any("completion" not in item and "Operation ID not found" in item for item in shown))
            self.assertTrue(any("not yet certified" in item for item in shown))


if __name__ == "__main__":
    unittest.main()
