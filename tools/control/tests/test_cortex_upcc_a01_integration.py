"""Integration proof: Cortex PCC's read-only inventory command bypasses PCC initialization."""
from __future__ import annotations
import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

CONTROL = Path(__file__).resolve().parents[1]


def load_pcc():
    spec = importlib.util.spec_from_file_location('_cortex_audit_pcc_test', CONTROL/'CortexPCC.py')
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    # The real controller imports maintenance/rollup at import time; neither may
    # run during an inventory. Stubs deliberately have no APIs to prove bypass.
    with patch.dict(sys.modules, {'CortexPCCMaintenance': types.ModuleType('CortexPCCMaintenance'),
                                  'CortexSourceRollup': types.ModuleType('CortexSourceRollup')}):
        sys.modules[str(CONTROL)] = sys.modules.get(str(CONTROL), types.ModuleType(str(CONTROL)))
        if str(CONTROL) not in sys.path:
            sys.path.insert(0, str(CONTROL))
        # Dataclass registration is required while the spec is executed.
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
    return module


class CortexAuditIntegration(unittest.TestCase):
    def test_dispatch_before_controller_construction(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest = {'schema': 'forge.project.v1', 'project': {'id': 'test'},
                        'commands': [{'key': 'test', 'program': 'python', 'risk': 'read_only'}],
                        'quality_gates': [{'key': 'full', 'stages': ['test']}]}
            (root/'project.control.json').write_text(json.dumps(manifest))
            original = (root/'project.control.json').read_bytes()
            out = io.StringIO()
            with patch.object(pcc, 'CortexPCC', side_effect=AssertionError('audit initialized operational controller')):
                with contextlib.redirect_stdout(out):
                    rc = pcc.main(['universal-audit', '--root', str(root)])
            self.assertEqual(rc, 0)
            data = json.loads(out.getvalue())
            self.assertEqual(data['projects'][0]['status'], 'DECLARED')
            self.assertEqual((root/'project.control.json').read_bytes(), original)
            self.assertFalse((root/'artifacts').exists())

    def test_legacy_rollbacks_are_reported_not_rewritten(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'project.control.json').write_text(json.dumps({
                'schema': 'forge.project.v1', 'project': {'id':'test'},
                'commands': [{'key':'fmt.apply','program':'cargo','rollback':'git_or_snapshot'}],
                'quality_gates': [{'key':'full','stages':['fmt.apply']}],
            }))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = pcc.main(['universal-audit', '--root', str(root)])
            self.assertEqual(rc, 2)
            data = json.loads(out.getvalue())
            self.assertEqual(data['projects'][0]['status'], 'INVALID')
            self.assertEqual(data['projects'][0]['contract']['commands'][0]['rollback'], 'git_or_snapshot')


if __name__ == '__main__':
    unittest.main()
