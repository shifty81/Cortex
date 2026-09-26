"""R8J-B1-W4: opt-in snapshot acceptance, no source or catalog mutation."""
from __future__ import annotations
from contextlib import closing
import importlib.util
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools/control'))
import CortexPersistentVolumeInventory as inventory
import CortexInventoryIndexAudit as audit


class InventoryIndexAuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.drive = Path(self.temp.name)/'drive'
        self.drive.mkdir()
        self.id = 'test-drive'
        (self.drive/'.cortex-volume.json').write_text(json.dumps({'schema':'cortex.volume.v1','volumeId':self.id}))
        self.catalog = self.drive/'vault_catalog.db'
        self.catalog.write_bytes(b'PRESERVE VAULT')
        (self.drive/'projects').mkdir()
        (self.drive/'projects'/'one.txt').write_text('project data')
        self.db = self.drive/'.cortex/inventory'/self.id/'inventory.sqlite3'
        inventory.run_scan(self.drive, self.db, volume_id=self.id)

    def test_read_only_audit_accepts_existing_index_and_counts(self):
        before = self.db.read_bytes()
        result = audit.audit_index(self.db, self.drive, volume_id=self.id,
                                   verify_counts=True, quick_check=True)
        self.assertTrue(result['passed'], result)
        self.assertTrue(result['counts_verified'])
        self.assertTrue(result['sqlite_quick_check'])
        self.assertEqual(result['actual_entries'], result['recorded_entries'])
        self.assertEqual(self.db.read_bytes(), before)
        self.assertEqual(self.catalog.read_bytes(), b'PRESERVE VAULT')

    def test_audit_fails_closed_on_wrong_volume_or_missing_index(self):
        with self.assertRaisesRegex(ValueError, 'Volume marker'):
            audit.audit_index(self.db, self.drive, volume_id='another-volume')
        missing = self.drive/'nonexistent.sqlite3'
        with self.assertRaises(FileNotFoundError):
            audit.audit_index(missing, self.drive, volume_id=self.id)
        self.assertFalse(missing.exists())

    def test_incomplete_scan_is_diagnostic_not_green_acceptance(self):
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute("UPDATE scan_state SET state='paused',completed_utc=NULL WHERE id=1")
            conn.execute("UPDATE directories SET finished=0 WHERE path='projects'")
            conn.commit()
        result = audit.audit_index(self.db, self.drive, volume_id=self.id,
                                   verify_counts=True, quick_check=True)
        self.assertFalse(result['passed'])
        self.assertFalse(result['completed'])
        self.assertEqual(result['state'], 'paused')
        self.assertEqual(result['pending_scan_directories'], 1)
        self.assertTrue(result['counts_verified'])
        self.assertTrue(result['sqlite_quick_check'])

    def test_unfinished_recursive_job_cannot_look_fully_accepted(self):
        with closing(sqlite3.connect(self.db)) as conn:
            inventory._recursive_tables(conn)
            conn.commit()
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute("INSERT INTO recursive_refresh(id,root,state,owner_pid,started_utc,updated_utc) "
                         "VALUES(1,'projects','paused',NULL,'now','now')")
            conn.execute("INSERT INTO recursive_refresh_queue(path,finished) VALUES('projects',0)")
            conn.commit()
        result = audit.audit_index(self.db, self.drive, volume_id=self.id, verify_counts=True)
        self.assertFalse(result['passed'])
        self.assertFalse(result['completed'])
        self.assertEqual(result['recursive_job']['pending'], 1)

    def test_explicit_counts_reveal_stale_denormalized_counter(self):
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute('UPDATE scan_state SET indexed_files=indexed_files+1 WHERE id=1')
            conn.commit()
        fast = audit.audit_index(self.db, self.drive, volume_id=self.id)
        self.assertTrue(fast['passed'])
        self.assertIsNone(fast['counts_verified'])  # quick mode does not claim a count audit
        deep = audit.audit_index(self.db, self.drive, volume_id=self.id, verify_counts=True)
        self.assertFalse(deep['passed'])
        self.assertFalse(deep['counts_verified'])
        self.assertNotEqual(deep['actual_entries'], deep['recorded_entries'])


if __name__ == '__main__':
    unittest.main()
