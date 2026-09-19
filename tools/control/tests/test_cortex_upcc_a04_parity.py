"""Focused, no-network parity inventory tests; no source mutation outside temporary fixtures."""
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "ForgePYParity.py"
SPEC = importlib.util.spec_from_file_location("ForgePYParity", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


class ForgePYParityTests(unittest.TestCase):
    def make_root(self, parent: Path, name: str, paths: set[str] | None = None) -> Path:
        root = parent / name
        root.mkdir()
        for rel in paths or set():
            file = root / rel
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text("source\n", encoding="utf-8")
        return root

    def test_never_certifies_parity_from_file_presence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            a = {p for _, _, _, _, names in MODULE.CAPABILITIES for p in names}
            b = {p for _, _, _, names, _ in MODULE.CAPABILITIES for p in names}
            cortex = self.make_root(root, "cortex", a)
            forge = self.make_root(root, "forge", b)
            report = MODULE.inventory(cortex, forge)
            self.assertEqual(report["sourceAnchorMissing"], 0)
            self.assertFalse(report["parityCertified"])
            self.assertTrue(all(not row["behaviorCertified"] for row in report["capabilities"]))
            self.assertTrue(all(row["status"] == "SOURCE_ANCHORS_PRESENT_BEHAVIOR_UNVERIFIED" for row in report["capabilities"]))

    def test_no_donor_never_assumes_forgepy_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = MODULE.inventory(self.make_root(Path(tmp), "cortex"))
            self.assertFalse(report["forgepy"]["supplied"])
            self.assertIsNone(report["capabilities"][0]["donorSourcePresent"])

    def test_missing_donor_is_distinguished_from_missing_consumer(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            consumer_paths = {p for _, _, _, _, names in MODULE.CAPABILITIES for p in names}
            report = MODULE.inventory(self.make_root(root, "cortex", consumer_paths), self.make_root(root, "forge"))
            self.assertEqual(report["sourceAnchorMissing"], 0)
            self.assertEqual(report["capabilities"][0]["status"], "DONOR_ANCHOR_MISSING")

    def test_legacy_rollbacks_are_detected_not_rewritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self.make_root(Path(tmp), "cortex")
            raw = '{"commands":[{"key":"fmt.apply","rollback":"git_or_snapshot"}]}'
            (root / "project.control.json").write_text(raw, encoding="utf-8")
            report = MODULE.inventory(root)
            self.assertEqual(len(report["blockingContractIssues"]), 1)
            self.assertEqual((root / "project.control.json").read_text(encoding="utf-8"), raw)

    def test_reports_manifest_build_without_certifying_candidate(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cortex = self.make_root(root, "cortex")
            forge = self.make_root(root, "forge")
            (forge / "project.control.json").write_text(json.dumps({"project": {"id": "forgepy", "candidateBuild": "LOCAL-TEST"}}))
            report = MODULE.inventory(cortex, forge)
            self.assertEqual(report["forgepy"]["declaredBuild"], "LOCAL-TEST")
            self.assertFalse(report["parityCertified"])

    def test_cli_strict_returns_two_for_missing_capability(self):
        with tempfile.TemporaryDirectory() as tmp:
            project = self.make_root(Path(tmp), "cortex")
            cp = subprocess.run([sys.executable, str(SCRIPT), "--root", str(project), "--strict"], capture_output=True, text=True, check=False)
            self.assertEqual(cp.returncode, 2)
            self.assertFalse(json.loads(cp.stdout)["parityCertified"])

    def test_root_controller_parity_dispatch_is_prebootstrap(self):
        # Import the exact cumulative controller with dependency stubs. If a
        # controller object is constructed, the operation would create logs.
        import contextlib
        import io
        import types
        from unittest.mock import patch
        controller = SCRIPT.with_name("CortexPCC.py")
        with tempfile.TemporaryDirectory() as tmp:
            root = self.make_root(Path(tmp), "cortex")
            spec = importlib.util.spec_from_file_location("A04Controller", controller)
            module = importlib.util.module_from_spec(spec)
            assert spec and spec.loader
            with patch.dict(sys.modules, {"CortexPCCMaintenance": types.ModuleType("CortexPCCMaintenance"), "CortexSourceRollup": types.ModuleType("CortexSourceRollup"), "A04Controller": module}):
                with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
                    spec.loader.exec_module(module)
                    with patch.object(module, "CortexPCC", side_effect=AssertionError("controller constructed")):
                        capture = io.StringIO()
                        with contextlib.redirect_stdout(capture):
                            rc = module.main(["forgepy-parity", "--root", str(root)])
            self.assertEqual(rc, 0)
            self.assertEqual(json.loads(capture.getvalue())["schema"], MODULE.SCHEMA)
            self.assertFalse((root / "artifacts").exists())

    def test_cli_default_only_reads(self):
        with tempfile.TemporaryDirectory() as tmp:
            project = self.make_root(Path(tmp), "cortex")
            initial = sorted(p.relative_to(project).as_posix() for p in project.rglob("*"))
            cp = subprocess.run([sys.executable, str(SCRIPT), "--root", str(project)], capture_output=True, text=True, check=False)
            self.assertEqual(cp.returncode, 0)
            self.assertEqual(initial, sorted(p.relative_to(project).as_posix() for p in project.rglob("*")))


if __name__ == "__main__":
    unittest.main()
