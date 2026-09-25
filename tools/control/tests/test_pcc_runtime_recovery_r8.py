from __future__ import annotations

import json
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPatchAuthority as patch
import PCCBuildDoctor as doctor


class RuntimeRecoveryR8Tests(unittest.TestCase):
    def test_neutral_zip_with_non_patch_manifest_is_not_patch_transport(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / ".cortex.zip"
            with zipfile.ZipFile(path, "w") as zf:
                zf.writestr("PATCH_MANIFEST.json", json.dumps({"schema": "cortex.pcc.buildgate_recovery_r1.v1"}))
                zf.writestr("tools/control/example.py", "print('x')\n")
            self.assertFalse(patch.looks_like_patch(path))

    def test_explicit_patch_named_zip_remains_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "root-patch-bad.zip"
            with zipfile.ZipFile(path, "w") as zf:
                zf.writestr("PATCH_MANIFEST.json", json.dumps({"schema": "wrong"}))
            self.assertTrue(patch.looks_like_patch(path))
            with self.assertRaises(patch.PatchError):
                patch.validate_patch(path, require_sidecar=False)

    def test_build_doctor_prioritizes_patch_authority_stage(self) -> None:
        payload = doctor.classify_failure(
            "[FAIL] QUICK PROJECT GATE FAILED at patch-authority: Unsupported patch schema",
            stage="patch-authority",
        )
        self.assertEqual(payload["category"], "PATCH")
        self.assertEqual(payload["repairScope"], "patch_queue")
        self.assertFalse(payload["sourceImplicated"])

    def test_build_doctor_reports_multiple_blockers(self) -> None:
        rows = doctor._blockers(
            "Unsupported patch schema\nMSVC amd64 developer environment is not active (link.exe/cl.exe missing)"
        )
        codes = {row["code"] for row in rows}
        self.assertIn("patch_transport_classification", codes)
        self.assertIn("windows_msvc_inactive", codes)

    def test_worker_receives_logs_and_can_fallback_for_non_mutating_modes(self) -> None:
        source = (CONTROL / "CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertIn("def _worker_request_text", source)
        self.assertIn("include_logs=True", source)
        self.assertIn("Falling back to the non-mutating Python provider path", source)
        self.assertIn('if mode in {"chat", "inspect", "plan"}:', source)
        self.assertIn("Mutating modes remain fail-closed", source)

    def test_msvc_capture_uses_temp_cmd_wrapper_and_host_surfaces_error(self) -> None:
        shared = (CONTROL / "PCCSharedEnvironment.py").read_text(encoding="utf-8")
        host = (CONTROL / "PCCOperationHost.py").read_text(encoding="utf-8")
        self.assertIn("tempfile.mkstemp", shared)
        self.assertIn('BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.4"', shared)
        self.assertIn('VERSION = "PCC-OPERATION-HOST-0.4"', host)
        self.assertIn("MSVC activation error", host)


if __name__ == "__main__":
    unittest.main()
