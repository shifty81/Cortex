"""Preflight is read-only and does not substitute discovery for a successful build."""
from __future__ import annotations
import json
import sys
import tempfile
import unittest
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONTROL))
from UniversalPCCPlan import inspect_gate


def manifest(*, command='python', rollback='none'):
    return {'schema':'forge.project.v1','project':{'id':'fixture'},
            'commands':[{'key':'build','program':command,'risk':'local_mutation','rollback':rollback}],
            'quality_gates':[{'key':'full','stages':['build']}]}


class UniversalPlanTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def write(self, data):
        (self.root/'project.control.json').write_text(json.dumps(data),encoding='utf-8')

    def test_valid_plan_is_resolved_but_not_certified(self):
        self.write(manifest(command=Path(sys.executable).name))
        out = inspect_gate(self.root)
        # Python executable itself is available on PATH in typical environments,
        # but executable discovery is not asserted as a cross-platform assumption.
        self.assertIn(out['status'], ('RESOLVED_NOT_TESTED','BLOCKED'))
        self.assertEqual(out['certification'], 'UNKNOWN')
        self.assertFalse(out['execution_performed'])
        self.assertFalse(out['stages'][0]['verified_working'])
        self.assertFalse((self.root/'artifacts').exists())

    def test_missing_executable_explicit_blocker(self):
        self.write(manifest(command='definitely_missing_upcc_executable_9f2e7d4e'))
        out = inspect_gate(self.root)
        self.assertEqual(out['status'], 'BLOCKED')
        self.assertIn('not on PATH', out['errors'][0])

    def test_missing_gate_stage_never_infers_command(self):
        m = manifest();m['quality_gates'][0]['stages'].append('missing-stage');self.write(m)
        out=inspect_gate(self.root)
        self.assertEqual(out['status'],'BLOCKED')
        self.assertIn('unresolved required stage',' '.join(out['errors']))

    def test_unknown_rollback_fails_rust_preflight(self):
        self.write(manifest(rollback='git_or_snapshot'))
        out=inspect_gate(self.root,rust_consumer=True)
        self.assertEqual(out['status'],'BLOCKED')
        self.assertIn('git_or_snapshot',' '.join(out['errors']))
        self.assertEqual(json.loads((self.root/'project.control.json').read_text())['commands'][0]['rollback'],'git_or_snapshot')

    def test_legacy_rollback_must_be_visible_even_when_python_plan_resolves(self):
        self.write(manifest(command=sys.executable,rollback='process_stop'))
        out=inspect_gate(self.root,rust_consumer=False)
        self.assertEqual(out['status'],'RESOLVED_NOT_TESTED')
        self.assertTrue(any('process_stop' in w for w in out['warnings']))

    def test_relative_escape_executable_fails_closed(self):
        self.write(manifest(command='../escape.exe'))
        out=inspect_gate(self.root)
        self.assertEqual(out['status'],'BLOCKED')
        self.assertTrue(any('escapes project root' in e for e in out['errors']))

    def test_invalid_cwd_fails_before_executable_check(self):
        m=manifest();m['commands'][0]['cwd']='../elsewhere';self.write(m)
        out=inspect_gate(self.root)
        self.assertEqual(out['status'],'BLOCKED')
        self.assertIn('cwd escapes',' '.join(out['errors']))

    def test_missing_manifest_is_honest(self):
        out=inspect_gate(self.root)
        self.assertEqual(out['status'],'BLOCKED')
        self.assertFalse(out['execution_performed'])


if __name__ == '__main__':unittest.main()
