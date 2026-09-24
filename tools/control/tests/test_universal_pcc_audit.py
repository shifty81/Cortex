"""The audit is read-only, rejects lossy contract conversion and never certifies builds."""
from __future__ import annotations
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

MODULE = Path(__file__).resolve().parents[1] / 'UniversalPCCAudit.py'
spec = importlib.util.spec_from_file_location('UniversalPCCAudit', MODULE)
assert spec and spec.loader
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def contract():
    return {'schema': 'forge.project.v1', 'project': {'id': 'fixture'},
            'commands': [{'key': 'build', 'program': 'cargo', 'args': ['build'], 'risk': 'local_mutation', 'rollback': 'none'}],
            'quality_gates': [{'key': 'full', 'stages': ['build']}]}


class AuditTests(unittest.TestCase):
    def test_valid_registry_preserves_stage_graph(self):
        d = m.validate_contract(contract())
        self.assertEqual(d['status'], 'DECLARED')
        self.assertEqual(d['gates'][0]['stages'], ['build'])
        self.assertFalse(d['source_certified'])

    def test_missing_required_stage_fails_closed(self):
        c = contract(); c['quality_gates'][0]['stages'] += ['migration-regressions', 'module-tests', 'integration-smoke']
        d = m.validate_contract(c)
        self.assertEqual(d['status'], 'INVALID')
        self.assertEqual(sum('unresolved required stage' in e for e in d['errors']), 3)

    def test_duplicate_command_casefold_rejected(self):
        c = contract(); c['commands'].append({'key':'BUILD','program':'cargo'})
        self.assertTrue(any('duplicate command' in e for e in m.validate_contract(c)['errors']))

    def test_rust_legacy_rollback_rejected_without_fake_normalization(self):
        c = contract(); c['commands'][0]['rollback'] = 'git_or_snapshot'
        d = m.validate_contract(c)
        self.assertEqual(d['status'], 'INVALID')
        self.assertEqual(d['commands'][0]['rollback'], 'git_or_snapshot')

    def test_legacy_mode_warns_not_verified(self):
        c = contract(); c['commands'][0]['rollback'] = 'process_stop'
        d = m.validate_contract(c, rust_consumer=False)
        self.assertEqual(d['status'], 'DECLARED')
        self.assertTrue(d['warnings'])
        self.assertFalse(d['commands'][0]['verified_executable'])

    def test_legacy_risk_aliases_are_canonicalized_without_rewriting_source(self):
        for declared, canonical in [('read', 'read_only'), ('write', 'local_mutation'), ('confirm', 'local_mutation')]:
            c = contract(); c['commands'][0]['risk'] = declared
            before = json.loads(json.dumps(c))
            out = m.validate_contract(c)
            self.assertEqual(out['status'], 'DECLARED')
            self.assertEqual(out['commands'][0]['risk'], canonical)
            self.assertTrue(any('legacy risk' in warning for warning in out['warnings']))
            self.assertEqual(c, before)

    def test_unknown_risk_blocks_mutation(self):
        c = contract(); c['commands'][0]['risk'] = 'magic_admin'
        self.assertIn('unsupported risk', ' '.join(m.validate_contract(c)['errors']))

    def test_numeric_schema_compatibility_does_not_overwrite_original(self):
        c = contract(); c.pop('schema'); c['schema_version'] = 1
        out = m.validate_contract(c)
        self.assertEqual(out['schema'], 'cortex.v1')
        self.assertNotIn('schema', c)

    def test_unsupported_schema_fails(self):
        c = contract(); c['schema'] = 'unknown.v8'
        self.assertEqual(m.validate_contract(c)['status'], 'INVALID')

    def test_duplicate_json_fields_are_rejected(self):
        with tempfile.TemporaryDirectory() as t:
            path = Path(t)/'project.control.json'; path.write_text('{"a":1,"a":2}')
            self.assertIn('duplicate JSON key', m._read_manifest(path)[2])

    def test_inventory_does_not_touch_project(self):
        with tempfile.TemporaryDirectory() as t:
            r = Path(t); (r/'project.control.json').write_text(json.dumps(contract()))
            before = {p.name:p.read_bytes() for p in r.iterdir()}
            out = m.inspect_project(r)
            after = {p.name:p.read_bytes() for p in r.iterdir()}
            self.assertEqual(before, after)
            self.assertEqual(out['status'], 'DECLARED')

    def test_bounded_scanning_reports_depth_incomplete(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t); deep = root/'a'/'b'; deep.mkdir(parents=True)
            (deep/'project.control.json').write_text(json.dumps(contract()))
            scan = m.discover(root, max_depth=0, max_dirs=100, max_projects=100)
            self.assertTrue(scan['incomplete'])
            self.assertEqual(scan['discovered_roots'], [])

    def test_scan_prunes_generated_directories(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t); (root/'target').mkdir(); (root/'normal').mkdir()
            (root/'target'/'project.control.json').write_text(json.dumps(contract()))
            (root/'normal'/'project.control.json').write_text(json.dumps(contract()))
            scan = m.discover(root, max_depth=2, max_dirs=100, max_projects=100)
            self.assertEqual(scan['discovered_roots'], [str(root/'normal')])
            self.assertFalse(scan['incomplete'])

    def test_unmanaged_project_is_not_called_ready(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t); (root/'Cargo.toml').write_text('[package]\nname="a"\n')
            self.assertEqual(m.inspect_project(root)['status'], 'UNMANAGED')

    def test_malformed_gate_structure_fails(self):
        c = contract(); c['quality_gates'] = {'full': {'stages': 'build'}}
        self.assertEqual(m.validate_contract(c)['status'], 'INVALID')

    def test_malformed_cwd_rejected(self):
        c = contract(); c['commands'][0]['cwd'] = '../escape'
        self.assertEqual(m.validate_contract(c)['status'], 'INVALID')


if __name__ == '__main__':
    unittest.main()
