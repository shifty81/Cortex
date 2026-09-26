"""Explicit online inventory backup and non-destructive recovery rehearsal."""
from pathlib import Path
import json
import sqlite3
import sys
import tempfile
import unittest

CONTROL = Path(__file__).resolve().parents[1] / 'tools' / 'control'
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPersistentVolumeInventory as inv
import CortexInventoryBackup as backup


class InventoryBackupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.volume = self.root / 'volume'; self.volume.mkdir()
        (self.volume / 'project.txt').write_text('unchanged', encoding='utf-8')
        self.db = self.root / 'state' / 'inventory.sqlite3'
        self.output = self.root / 'artifacts' / 'inventory-backups'
        self.output.parent.mkdir()
        self.vid = 'stable-backup-volume'
        inv.run_scan(self.volume, self.db, volume_id=self.vid)

    def test_backup_integrity_receipt_and_no_live_replacement(self):
        before = inv.index_status(self.db, self.volume, volume_id=self.vid)
        result = backup.backup_inventory(self.db, volume_id=self.vid, output_dir=self.output)
        self.assertFalse(result['live_database_replaced'])
        self.assertTrue(result['verification']['sqlite_quick_check'])
        self.assertEqual(result['verification']['actual_entry_count'], before['entries'])
        self.assertEqual((self.volume / 'project.txt').read_text(), 'unchanged')
        source_after = inv.index_status(self.db, self.volume, volume_id=self.vid)
        self.assertEqual(source_after, before)
        copy = Path(result['backup'])
        receipt = json.loads(copy.with_name(copy.name+'.receipt.json').read_text())
        self.assertEqual(receipt['sha256'], result['sha256'])
        drill = backup.restore_rehearsal(copy, volume_id=self.vid, expected_sha256=result['sha256'])
        self.assertFalse(drill['restore_applied'])
        self.assertEqual(drill['actual_entry_count'], before['entries'])

    def test_wrong_volume_rejected_and_live_index_preserved(self):
        with self.assertRaisesRegex(ValueError, 'identity mismatch'):
            backup.backup_inventory(self.db, volume_id='incorrect', output_dir=self.output)
        self.assertTrue(self.db.is_file())
        self.assertEqual(list(self.output.glob('*.sqlite3')), [])

    def test_copy_digest_mismatch_fails_closed(self):
        result = backup.backup_inventory(self.db, volume_id=self.vid, output_dir=self.output)
        with self.assertRaisesRegex(ValueError, 'SHA-256'):
            backup.restore_rehearsal(result['backup'], volume_id=self.vid, expected_sha256='0'*64)

    def test_no_accidental_backup_in_active_index_directory(self):
        with self.assertRaisesRegex(ValueError, 'live database directory'):
            backup.backup_inventory(self.db, volume_id=self.vid, output_dir=self.db.parent)


if __name__ == '__main__':
    unittest.main()
