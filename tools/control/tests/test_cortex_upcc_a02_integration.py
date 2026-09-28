"""Cortex PCC A02 regression: plan bypass, semantic gate, Full Gate and startup truth."""
from __future__ import annotations
import contextlib
import io
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

CONTROL=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(CONTROL))
sys.path.insert(0,str(Path(__file__).resolve().parent))
from test_cortex_upcc_a01_integration import load_pcc


def contract(rollback='none', stage='check'):
    return {'schema':'forge.project.v1','project':{'id':'cortex'},
            'commands':[{'key':'check','program':sys.executable,'rollback':rollback}],
            'quality_gates':[{'key':'full','stages':[stage]}]}


class ControllerA02Tests(unittest.TestCase):
    def test_universal_plan_is_not_a_controller_bootstrap(self):
        pcc=load_pcc()
        with tempfile.TemporaryDirectory() as t:
            root=Path(t)
            (root/'project.control.json').write_text(json.dumps(contract()),encoding='utf-8')
            original=(root/'project.control.json').read_bytes()
            buf=io.StringIO()
            with patch.object(pcc,'CortexPCC',side_effect=AssertionError('controller should not construct')):
                with contextlib.redirect_stdout(buf):
                    code=pcc.main(['universal-plan','--root',str(root)])
            self.assertEqual(code,0)
            outcome=json.loads(buf.getvalue())
            self.assertEqual(outcome['status'],'RESOLVED_NOT_TESTED')
            self.assertFalse(outcome['execution_performed'])
            self.assertEqual((root/'project.control.json').read_bytes(),original)
            self.assertFalse((root/'artifacts').exists())

    def test_json_contract_gate_detects_missing_stages(self):
        pcc=load_pcc()
        with tempfile.TemporaryDirectory() as t:
            path=Path(t)/'project.control.json'
            path.write_text(json.dumps(contract(stage='missing-stage')),encoding='utf-8')
            gate=object.__new__(pcc.GateEngine)
            gate.ctx=types.SimpleNamespace(project_control=path)
            status,reason=gate._json_contracts()
            self.assertEqual(status,'FAIL')
            self.assertIn('unresolved required stage',reason)

    def test_legacy_contract_warns_without_secret_rewrite(self):
        pcc=load_pcc()
        with tempfile.TemporaryDirectory() as t:
            path=Path(t)/'project.control.json'
            path.write_text(json.dumps(contract(rollback='git_or_snapshot')),encoding='utf-8')
            before=path.read_bytes()
            gate=object.__new__(pcc.GateEngine)
            gate.ctx=types.SimpleNamespace(project_control=path)
            status,reason=gate._json_contracts()
            self.assertEqual(status,'WARN')
            self.assertIn('git_or_snapshot',reason)
            self.assertEqual(path.read_bytes(),before)

    def test_full_gate_never_marks_green_after_python_failure(self):
        pcc=load_pcc()
        gate=object.__new__(pcc.GateEngine)
        gate.quick=Mock(return_value=(True,[]))
        gate.ctx=types.SimpleNamespace(root=Path('/tmp/unused-a02'))
        gate.runner=Mock()
        gate.log=Mock()
        gate.cargo_step=Mock(return_value=True)
        gate.git=Mock()
        with patch.object(pcc,'run_universal_python_regressions',return_value=2) as run:
            result=gate.full()
        self.assertFalse(result)
        self.assertEqual(gate.failed_stage,'python-pcc-regressions')
        run.assert_called_once()
        gate.cargo_step.assert_not_called()
        gate.git.action.assert_not_called()

    def test_missing_python_test_fails_closed(self):
        pcc=load_pcc()
        with tempfile.TemporaryDirectory() as t:
            out=io.StringIO()
            with contextlib.redirect_stdout(out):
                code=pcc.run_universal_python_regressions(Path(t))
            self.assertEqual(code,2)
            self.assertIn('Missing mandatory Python PCC tests',out.getvalue())

    def test_launch_gui_compatibility_alias_launches_pcc_not_desktop(self):
        pcc=load_pcc()
        with tempfile.TemporaryDirectory() as t:
            root=Path(t)
            gui=root/'tools'/'control'/'CortexPCCGui.py'
            gui.parent.mkdir(parents=True)
            gui.write_text('print("fixture")\n',encoding='utf-8')
            controller=object.__new__(pcc.CortexPCC)
            controller.ctx=types.SimpleNamespace(root=root)
            controller.log=Mock()
            proc=types.SimpleNamespace(pid=4321)
            with patch.object(pcc.subprocess,'Popen',return_value=proc) as spawn:
                code=controller.launch_gui()
            self.assertEqual(code,0)
            spawn.assert_called_once()
            argv=spawn.call_args.args[0]
            self.assertIn(str(gui),argv)
            self.assertNotIn('cortex_desktop.exe',' '.join(argv))
            self.assertIn('legacy Desktop launch path retired',str(controller.log.emit.call_args_list[-1]))


if __name__ == '__main__':unittest.main()
