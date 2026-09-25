from __future__ import annotations

import os
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

TOOLS = Path(__file__).resolve().parents[1]
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import PCCBuildDoctor as doctor
import PCCRepairCoordinator as repair
import PCCSurfaceCommon as surface


class RepairCoordinatorTests(unittest.TestCase):
    def test_newer_green_gate_supersedes_older_failure(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            logs = root / "artifacts" / "logs"
            logs.mkdir(parents=True)
            old = logs / "old.log"
            old.write_text("[FAIL] QUICK PROJECT GATE FAILED at cargo-build: could not compile\n", encoding="utf-8")
            now = time.time()
            os.utime(old, (now - 20, now - 20))
            green = logs / "new.log"
            green.write_text("[PASS] FULL QUALITY GATE GREEN / SOURCE CERTIFIED\n=== END full: PASS ===\n", encoding="utf-8")
            os.utime(green, (now, now))
            payload = doctor.latest_failure(root)
            self.assertEqual(payload["status"], "NO_ACTIVE_FAILURE")
            self.assertEqual(payload["latestGate"]["status"], "PASS")

    def test_green_marker_newer_than_failure_suppresses_stale_repair(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            logs = root / "artifacts" / "logs"
            logs.mkdir(parents=True)
            fail = logs / "failure.log"
            fail.write_text("[FAIL] QUICK PROJECT GATE FAILED at cargo-build: could not compile\n", encoding="utf-8")
            now = time.time()
            os.utime(fail, (now - 20, now - 20))
            marker = root / ".cortex" / "last-green-quality-gate.json"
            marker.parent.mkdir(parents=True)
            marker.write_text("{}", encoding="utf-8")
            os.utime(marker, (now, now))
            payload = doctor.latest_failure(root)
            self.assertEqual(payload["status"], "NO_ACTIVE_FAILURE")
            self.assertTrue(payload["greenMarkerNewerThanFailure"])

    def test_repair_plan_is_noop_when_no_active_failure(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            with mock.patch.object(repair, "latest_failure", return_value={
                "status": "NO_ACTIVE_FAILURE",
                "classification": {"category": "UNKNOWN"},
            }):
                payload = repair.repair_plan(root, certify=True)
            self.assertEqual(payload["status"], "NO_REPAIR_NEEDED")
            self.assertFalse(payload["mutatesSource"])
            self.assertFalse(payload["requiresApproval"])

    def test_source_repair_plan_requires_worker_and_approval(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            with mock.patch.object(repair, "latest_failure", return_value={
                "status": "FAILURE_CLASSIFIED",
                "classification": {"category": "SOURCE_COMPILE", "code": "compile_failure"},
            }), mock.patch.object(repair, "worker_status", return_value={"status": "READY", "selected": "X:/cortex.exe"}):
                payload = repair.repair_plan(root, certify=True)
            self.assertEqual(payload["status"], "READY")
            self.assertTrue(payload["mutatesSource"])
            self.assertTrue(payload["requiresApproval"])
            self.assertTrue(payload["certifyFull"])

    def test_sensitive_non_source_failure_is_not_auto_repaired(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            with mock.patch.object(repair, "latest_failure", return_value={
                "status": "FAILURE_CLASSIFIED",
                "classification": {"category": "AUTH", "code": "source_control_auth"},
            }):
                payload = repair.repair_plan(root)
            self.assertEqual(payload["status"], "BLOCKED_FOR_REVIEW")
            self.assertFalse(payload["mutatesSource"])

    def test_surface_marks_plan_read_only_and_apply_mutating(self):
        self.assertEqual(surface._surface_risk("repair-plan"), "read_only")
        self.assertEqual(surface._surface_risk("repair-plan-json"), "read_only")
        self.assertEqual(surface._surface_risk("repair-current"), "local_mutation")
        self.assertEqual(surface._surface_risk("repair-current-full"), "local_mutation")

    def test_gui_has_explicit_repair_plan_and_full_buttons(self):
        text = (TOOLS / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn('"Repair Plan", self._start_repair_plan', text)
        self.assertIn('"Repair + FULL", self._start_repair_current_full', text)
        self.assertIn('backend.popen("repair-current-full", ["--yes"])', text)
        self.assertIn('Build/gate repair request detected', text)


if __name__ == "__main__":
    unittest.main()
