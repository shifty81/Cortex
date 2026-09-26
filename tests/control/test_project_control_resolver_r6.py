import sys
import tempfile
import unittest
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[2] / "tools" / "control"
sys.path.insert(0, str(CONTROL))
from PCCProjectControlResolver import resolve_project_operation


class ProjectControlResolverR6Tests(unittest.TestCase):
    def _contract(self, program, args):
        return {
            "commands": [{"key": "gate.full", "program": program, "args": args}],
            "_pccDiscovery": {"source": "native-project-pcc", "authority": "native-project-pcc"},
        }

    def test_rejects_invented_full_from_legacy_wrapper(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "PROJECT_CONTROL_CENTER.cmd").write_text('@powershell -File "tools\\pcc\\ForgeGuiControl.ps1" -Operation %*\n')
            (root / "tools" / "pcc").mkdir(parents=True)
            (root / "tools" / "pcc" / "ForgeGuiControl.ps1").write_text('switch ($Operation) {\n "startup" { }\n "build" { }\n default { throw "Unknown operation $Operation" }\n}\n')
            result = resolve_project_operation(root, self._contract("PROJECT_CONTROL_CENTER.cmd", ["full"]), "full")
            self.assertEqual("BLOCKED", result["status"])
            self.assertEqual("CORTEX-PC-1004", result["errorCode"])
            self.assertFalse(result["sourceImplicated"])

    def test_accepts_full_when_provider_declares_it(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "PROJECT_CONTROL_CENTER.cmd").write_text('@powershell -File "Control.ps1" -Operation %*\n')
            (root / "Control.ps1").write_text('switch ($Operation) {\n "full" { }\n "build" { }\n}\n')
            result = resolve_project_operation(root, self._contract("PROJECT_CONTROL_CENTER.cmd", ["full"]), "full")
            self.assertEqual("READY", result["status"])
            self.assertEqual("gate.full", result["capability"])

    def test_explicit_non_pcc_command_remains_usable(self):
        root = Path.cwd()
        data = {"commands": [{"key": "gate.full", "program": "cargo", "args": ["test"]}], "_pccDiscovery": {"source": "project.control.json"}}
        result = resolve_project_operation(root, data, "full")
        self.assertEqual("READY", result["status"])

    def test_missing_capability_is_control_error(self):
        result = resolve_project_operation(Path.cwd(), {"commands": []}, "full")
        self.assertEqual("BLOCKED", result["status"])
        self.assertEqual("CORTEX-PC-1001", result["errorCode"])


if __name__ == "__main__":
    unittest.main()
