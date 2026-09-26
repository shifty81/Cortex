"""R8J-B1-W3: cross-PCC authority tests discovered by the mandatory FULL gate.

Tests do not provision external volumes, change global Git settings, or modify user projects.
"""
from __future__ import annotations

import inspect
from contextlib import closing
import json
import sqlite3
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
ROOT = CONTROL.parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPCC as pcc
import PCCSurfaceCommon as surface
import CortexPersistentVolumeInventory as inventory


class ProjectWideAuthorityTests(unittest.TestCase):
    def test_portable_registry_id_is_volume_scoped_not_drive_letter_scoped(self):
        with mock.patch.object(surface, 'portable_relative_to_runtime_volume', return_value='Source/Example'):
            with mock.patch.object(surface, 'resolve_volume_context', return_value=types.SimpleNamespace(volume_id='volume-one')):
                first = surface.ProjectRegistry._registry_id(Path('G:/Source/Example'))
                second = surface.ProjectRegistry._registry_id(Path('D:/Source/Example'))
            with mock.patch.object(surface, 'resolve_volume_context', return_value=types.SimpleNamespace(volume_id='volume-two')):
                different = surface.ProjectRegistry._registry_id(Path('G:/Source/Example'))
        self.assertEqual(first, second)
        self.assertNotEqual(first, different)

    def test_reading_registry_is_not_a_legacy_registry_migration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            portable = root / 'portable.json'
            old = root / 'legacy.json'
            portable.write_text(json.dumps({'projects': [{
                'registryId': 'portable-1', 'projectId': 'example', 'name': 'Example',
                'root': 'G:/Source/Example', 'portableRelativeRoot': 'Source/Example',
            }]}), encoding='utf-8')
            old.write_text(json.dumps({'projects': [{
                'registryId': 'legacy-1', 'projectId': 'example', 'name': 'Example',
                'root': 'D:/Source/Example',
            }]}), encoding='utf-8')
            before = (portable.read_bytes(), old.read_bytes())
            with mock.patch.object(surface.ProjectRegistry, 'default_path', return_value=portable), \
                 mock.patch.object(surface.ProjectRegistry, 'legacy_default_path', return_value=old), \
                 mock.patch.object(surface, 'resolve_portable_volume_path', return_value=Path('G:/Source/Example')):
                rows = surface.ProjectRegistry().entries()
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0].registry_id, 'portable-1')
            self.assertEqual((portable.read_bytes(), old.read_bytes()), before)

    def test_w4_shared_writer_reservation_precedes_scan_and_refresh_authority_reads(self):
        # Prevent a future code reshuffle from reintroducing cross-process stale
        # reads of recursive job ownership before the SQLite writer reservation.
        scan = inspect.getsource(inventory.run_scan)
        refresh = inspect.getsource(inventory.refresh_directory)
        self.assertLess(scan.index('conn.execute("BEGIN IMMEDIATE")'),
                        scan.index('SELECT state FROM recursive_refresh'))
        self.assertLess(refresh.index('conn.execute("BEGIN IMMEDIATE")'),
                        refresh.index('SELECT state,owner_pid FROM recursive_refresh'))
        self.assertIn("complete with access gaps", (CONTROL / 'CortexPCCGui.py').read_text(encoding='utf-8'))

    def test_mutating_repair_and_trust_are_not_read_only(self):
        for command in ('repair-current', 'repair-current-full', 'git-trust', 'git-fetch', 'debug-bundle', 'source-rollup', 'full', 'fast'):
            with self.subTest(command=command):
                self.assertEqual(surface._surface_risk(command), 'local_mutation')
        self.assertEqual(pcc.build_parser().parse_args(['git-trust']).command, 'git-trust')

    def test_quick_gate_rejects_false_git_readiness(self):
        with tempfile.TemporaryDirectory() as directory:
            authority = Path(directory) / 'CortexGitAuthority.py'
            authority.write_text('# fixture')
            gate = object.__new__(pcc.GateEngine)
            gate.git = types.SimpleNamespace(
                script=authority,
                action=lambda *args, **kwargs: types.SimpleNamespace(
                    ok=True, stdout='{"gitReady": false, "error": "untrusted checkout"}\n',
                    stderr='', returncode=0),
            )
            self.assertEqual(gate._git_authority(), ('FAIL', 'untrusted checkout'))

    def test_full_gate_requires_both_discovery_roots(self):
        src = inspect.getsource(pcc.run_universal_python_regressions)
        self.assertIn('("all-control-tests", root / "tools/control/tests")', src)
        self.assertIn('("all-inventory-tests", root / "tests")', src)
        self.assertIn('("nested-project-control-tests", root / "tests/control")', src)
        self.assertIn('("root-staging-tests", root)', src)
        self.assertIn('"test_stage_candidate.py"', src)
        self.assertIn('missing or empty', src)
        self.assertIn('"unittest", "discover"', src)
        self.assertIn('stream=False', src)

    def test_abandon_reads_owner_only_after_writer_reservation(self):
        src = inspect.getsource(inventory.abandon_recursive_refresh)
        self.assertLess(src.index('conn.execute("BEGIN IMMEDIATE")'),
                        src.index('row = conn.execute("SELECT state,owner_pid FROM recursive_refresh'))

    def test_full_gate_recovery_promise_matches_implemented_source(self):
        # An independently available Vault command is not evidence that FULL
        # automatically produced and deep-verified a recovery mirror.
        source = inspect.getsource(pcc.GateEngine.full)
        contract = json.loads((ROOT / 'project.control.json').read_text(encoding='utf-8'))
        full = next(item for item in contract['quality_gates'] if item['key'] == 'full')
        doc = (ROOT / 'docs/QUALITY_GATE_CONTRACT.md').read_text(encoding='utf-8')
        if 'mirror_project' not in source and 'vault-mirror' not in source:
            self.assertNotIn('adds Vault + GREEN certification', full['label'])
            self.assertIn('does not currently prove a deep-verified Vault recovery point', doc)
        self.assertIn('mark-green', source)

    def test_live_recursive_job_cannot_be_discarded(self):
        with tempfile.TemporaryDirectory() as directory:
            volume = Path(directory) / 'drive'
            volume.mkdir()
            (volume / 'contents').mkdir()
            (volume / 'contents' / 'before.txt').write_text('preserve', encoding='utf-8')
            vid = 'w3-job-owner-test'
            db = volume / '.cortex' / 'inventory' / vid / 'inventory.sqlite3'
            self.assertEqual(inventory.run_scan(volume, db, volume_id=vid)['state'], 'complete')
            paused = inventory.refresh_tree(volume, db, volume_id=vid, relative='contents', cancelled=lambda: True)
            self.assertEqual(paused['state'], 'paused')
            with closing(sqlite3.connect(db)) as conn:
                conn.execute("UPDATE recursive_refresh SET state='running',owner_pid=424242 WHERE id=1")
                conn.commit()
            with mock.patch.object(inventory, '_owner_alive', return_value=True):
                with self.assertRaisesRegex(RuntimeError, 'live recursive refresh'):
                    inventory.abandon_recursive_refresh(db, volume_id=vid)
            self.assertEqual(inventory.recursive_refresh_status(db, volume_id=vid)['pending'], 1)
            with mock.patch.object(inventory, '_owner_alive', return_value=False):
                abandoned = inventory.abandon_recursive_refresh(db, volume_id=vid)
            self.assertEqual(abandoned['state'], 'abandoned')
            entries = inventory.query_entries(db, volume_id=vid, query='before.txt')['rows']
            self.assertTrue(any(e['path'] == 'contents/before.txt' for e in entries))


if __name__ == '__main__':
    unittest.main()
