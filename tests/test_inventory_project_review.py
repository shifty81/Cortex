"""Metadata-only project review; never changes an existing inventory or registry."""
from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest

CONTROL = Path(__file__).resolve().parents[1] / 'tools' / 'control'
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPersistentVolumeInventory as inv
import CortexInventoryProjectReview as review


class IndexedProjectReviewTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.volume = self.root / 'external'; self.volume.mkdir()
        self.db = self.root / 'state' / 'inventory.sqlite3'
        self.vid = 'stable-portable-volume'

    def scan(self):
        return inv.run_scan(self.volume, self.db, volume_id=self.vid)

    def test_projects_and_nested_components_are_proposals_not_registry_changes(self):
        for name in ('projects/Game/.git', 'projects/Game/crates/tool',
                     'Source/Game', 'projects/Game/node_modules/unrelated', 'projects/Game/target/ignored'):
            (self.volume / name).mkdir(parents=True, exist_ok=True)
        for name in ('projects/Game/Cargo.toml', 'projects/Game/crates/tool/Cargo.toml',
                     'Source/Game/Cargo.toml', 'projects/Game/node_modules/unrelated/Cargo.toml',
                     'projects/Game/target/ignored/Cargo.toml'):
            (self.volume / name).write_text('fixture', encoding='utf-8')
        self.scan()
        before = self.db.read_bytes()
        report = review.build_preview(self.db, self.volume, volume_id=self.vid)
        self.assertEqual(report['candidate_count'], 3)
        roots = {c['relative_root']: c for c in report['candidates']}
        self.assertIn('projects/Game', roots)
        self.assertIn('Source/Game', roots)
        self.assertIn('projects/Game/crates/tool', roots)
        self.assertEqual(roots['projects/Game/crates/tool']['parent_candidate_id'], roots['projects/Game']['candidate_id'])
        self.assertEqual(report['same_name_group_count'], 1)  # two separately located Game roots
        self.assertFalse(report['registry_mutated'])
        self.assertFalse(report['source_files_read'])
        self.assertEqual(before, self.db.read_bytes())

    def test_mount_letter_does_not_change_identity(self):
        (self.volume / 'projects' / 'Game').mkdir(parents=True)
        (self.volume / 'projects' / 'Game' / 'project.control.json').write_text('{}')
        self.scan()
        a = review.build_preview(self.db, Path('G:/'), volume_id=self.vid)
        b = review.build_preview(self.db, Path('D:/'), volume_id=self.vid)
        self.assertEqual(a['candidates'][0]['candidate_id'], b['candidates'][0]['candidate_id'])
        self.assertNotEqual(a['mount_root'], b['mount_root'])

    def test_wrong_volume_and_missing_index_fail_without_creating_database(self):
        (self.volume / 'Cargo.toml').write_text('test')
        self.scan()
        with self.assertRaisesRegex(ValueError, 'Volume identity mismatch'):
            review.build_preview(self.db, self.volume, volume_id='different-volume')
        absent = self.root / 'missing.sqlite3'
        with self.assertRaises(FileNotFoundError):
            review.build_preview(absent, self.volume, volume_id=self.vid)
        self.assertFalse(absent.exists())

    def test_paused_scan_rejected_not_misrepresented_as_complete(self):
        (self.volume / 'Cargo.toml').write_text('test')
        inv.run_scan(self.volume, self.db, volume_id=self.vid, cancelled=lambda: True)
        with self.assertRaisesRegex(RuntimeError, 'Complete initial inventory'):
            review.build_preview(self.db, self.volume, volume_id=self.vid)

    def test_export_writes_artifact_only(self):
        report = {'schema': 'fixture', 'candidates': []}
        target = review.export_preview(report, self.root)
        self.assertEqual(json.loads(target.read_text()), report)
        self.assertTrue(target.is_relative_to(self.root / 'artifacts' / 'reviews'))
        self.assertFalse((self.root / '.cortex' / 'registry').exists())


if __name__ == '__main__':
    unittest.main()
