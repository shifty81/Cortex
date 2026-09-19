"""A05 independent semantic-contract migration fixtures: no project source touched."""
from __future__ import annotations

import contextlib
import hashlib
import json
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

CONTROL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONTROL))
import CortexContractMigration as migration
import UniversalPCCAudit as audit


def fixture(root: Path, *, fmt_rollback='git_or_snapshot', run_rollback='process_stop') -> Path:
    payload = {'schema': 'forge.project.v1', 'project': {'id': 'cortex'}, 'commands': [
        {'key': 'fmt.apply', 'label': 'Apply formatting', 'program': 'cargo',
         'args': ['fmt', '--all'], 'risk': 'local_mutation',
         'side_effects': ['source_files'], 'permissions': ['workspace_write'],
         'cancellation': 'bounded_kill', 'rollback': fmt_rollback},
        {'key': 'forge.rust.run', 'label': 'Run Forge', 'program': 'cargo',
         'args': ['run', '--manifest-path', 'products/forge-rust/Cargo.toml', '-p', 'forge-rs'],
         'risk': 'local_mutation', 'side_effects': ['build_outputs', 'process_launch'],
         'cancellation': 'bounded_kill', 'rollback': run_rollback},
    ], 'quality_gates': [{'key': 'full', 'stages': ['fmt.apply']}]}
    path = root / 'project.control.json'
    path.write_text(json.dumps(payload, indent=2) + '\n', encoding='utf-8')
    return path


class ContractMigrationTests(unittest.TestCase):
    def test_preview_is_pure_and_incompatibility_remains_detectable(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            before = path.read_bytes()
            report, original, proposed = migration.plan(root)
            self.assertEqual(original, before)
            self.assertEqual(len(report['changes']), 2)
            self.assertNotEqual(proposed, before)
            self.assertEqual(path.read_bytes(), before)
            self.assertFalse((root / 'artifacts').exists())
            self.assertEqual(audit.inspect_project(root, rust_consumer=True)['status'], 'INVALID')

    def test_applies_only_two_tokens_and_preserves_exact_backup(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            original = path.read_bytes()
            stub = types.SimpleNamespace(OperationLock=lambda *_: contextlib.nullcontext())
            with patch.dict(sys.modules, {'CortexPCCMaintenance': stub}):
                result = migration.apply(root, migration.sha(original))
            self.assertEqual(result['status'], 'APPLIED_SOURCE_NEEDS_NEW_FULL_GATE')
            self.assertEqual(Path(result['originalBackup']).read_bytes(), original)
            self.assertEqual(result['backupSha256'], migration.sha(original))
            self.assertEqual(audit.inspect_project(root, rust_consumer=True)['status'], 'DECLARED')
            candidate = original.replace(b'"git_or_snapshot"', b'"none"').replace(b'"process_stop"', b'"none"')
            self.assertEqual(path.read_bytes(), candidate)
            self.assertEqual(json.loads(Path(result['originalBackup']).with_name('receipt.json').read_text())['status'], result['status'])

    def test_expected_preimage_conflict_is_nonmutating(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            before = path.read_bytes()
            with self.assertRaisesRegex(migration.MigrationError, 'preimage mismatch'):
                migration.plan(root, '0'*64)
            self.assertEqual(path.read_bytes(), before)
            self.assertFalse((root / 'artifacts').exists())

    def test_other_project_or_changed_semantics_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            original = path.read_text()
            path.write_text(original.replace('"cortex"', '"ember"'))
            with self.assertRaisesRegex(migration.MigrationError, 'not exactly cortex'):
                migration.plan(root)
            path.write_text(original.replace('"fmt",', '"build",'))
            with self.assertRaisesRegex(migration.MigrationError, 'command semantics changed'):
                migration.plan(root)

    def test_unexpected_rollback_and_duplicate_tokens_fail_closed(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            path.write_text(path.read_text().replace('git_or_snapshot', 'provider_owned'))
            with self.assertRaisesRegex(migration.MigrationError, 'unexpected rollback'):
                migration.plan(root)
            fixture(root)
            path.write_text(path.read_text().replace('"schema":', '"schema":"duplicate", "schema":', 1))
            with self.assertRaisesRegex(migration.MigrationError, 'duplicate JSON key'):
                migration.plan(root)

    def test_idempotent_when_already_migrated(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root, fmt_rollback='none', run_rollback='none')
            report, original, proposed = migration.plan(root)
            self.assertEqual(report['changes'], [])
            self.assertEqual(original, proposed)
            self.assertEqual(path.read_bytes(), original)

    def test_cli_defaults_preview_and_never_creates_backup(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            before = path.read_bytes()
            cp = subprocess.run([sys.executable, str(CONTROL / 'CortexContractMigration.py'), '--root', str(root)],
                                capture_output=True, text=True, check=False)
            self.assertEqual(cp.returncode, 0, cp.stderr)
            self.assertEqual(json.loads(cp.stdout)['status'], 'PREVIEW_ONLY')
            self.assertEqual(path.read_bytes(), before)
            self.assertFalse((root / 'artifacts').exists())

    def test_parity_inventory_no_longer_flags_migrated_values(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = fixture(root)
            report, original, candidate = migration.plan(root)
            path.write_bytes(candidate)
            import ForgePYParity
            inventory = ForgePYParity.inventory(root)
            self.assertFalse(inventory['blockingContractIssues'])
            self.assertFalse(inventory['parityCertified'])

    def test_full_gate_registers_a05_regression_fixture(self):
        source = (CONTROL / 'CortexPCC.py').read_text(encoding='utf-8')
        self.assertIn('root / "tools/control/tests/test_cortex_upcc_a05_contract.py"', source)
        self.assertIn('if cmd == "contract-migrate":', source)


if __name__ == '__main__':
    unittest.main()
