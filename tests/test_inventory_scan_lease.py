"""R8J-B1-W6: exclusive initial-scanner ownership on the existing SQLite index."""
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from contextlib import closing
import sqlite3
import tempfile
import threading
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('r8j_w6_inventory_scan', ROOT / 'tools/control/CortexPersistentVolumeInventory.py')
index = importlib.util.module_from_spec(spec)
spec.loader.exec_module(index)


class ScannerLeaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.drive = Path(self.temp.name) / 'drive'
        self.drive.mkdir()
        (self.drive / 'one.txt').write_text('unchanged', encoding='utf-8')
        self.vid = 'w6-volume'
        self.db = self.drive / '.cortex' / 'inventory' / self.vid / 'inventory.sqlite3'

    def owner(self):
        with closing(sqlite3.connect(self.db)) as conn:
            return conn.execute('SELECT owner_pid,token FROM inventory_scan_owner WHERE id=1').fetchone()

    def test_second_worker_cannot_scan_same_pending_volume(self):
        entered, release = threading.Event(), threading.Event()
        original = index.os.scandir
        first_thread = None
        outcomes = []

        def blocked(path):
            if threading.current_thread() is first_thread:
                entered.set()
                if not release.wait(8):
                    raise RuntimeError('Primary scan timed out in fixture')
            return original(path)

        def first_worker():
            try:
                outcomes.append(index.run_scan(self.drive, self.db, volume_id=self.vid))
            except BaseException as exc:
                outcomes.append(exc)

        first_thread = threading.Thread(target=first_worker, daemon=True)
        with mock.patch.object(index.os, 'scandir', side_effect=blocked):
            first_thread.start()
            try:
                self.assertTrue(entered.wait(5), 'Primary scanner did not acquire lease')
                with self.assertRaisesRegex(RuntimeError, 'already active'):
                    index.run_scan(self.drive, self.db, volume_id=self.vid)
                with self.assertRaisesRegex(RuntimeError, 'already active'):
                    index.run_scan(self.drive, self.db, volume_id=self.vid, restart=True)
                self.assertIsNotNone(self.owner())
            finally:
                release.set()
                first_thread.join(8)
        self.assertFalse(first_thread.is_alive())
        self.assertEqual(len(outcomes), 1)
        self.assertIsInstance(outcomes[0], dict, outcomes)
        self.assertEqual(outcomes[0]['state'], 'complete')
        self.assertIsNone(self.owner())
        self.assertEqual(index.query_entries(self.db, volume_id=self.vid, query='one.txt')['rows'][0]['path'], 'one.txt')
        self.assertEqual((self.drive / 'one.txt').read_text(encoding='utf-8'), 'unchanged')

    def test_cancel_releases_lease_and_pending_queue_resumes(self):
        paused = index.run_scan(self.drive, self.db, volume_id=self.vid, cancelled=lambda: True)
        self.assertEqual(paused['state'], 'paused')
        self.assertIsNone(self.owner())
        self.assertEqual(index.run_scan(self.drive, self.db, volume_id=self.vid)['state'], 'complete')
        self.assertIsNone(self.owner())

    def test_stale_dead_worker_is_reclaimable(self):
        index.run_scan(self.drive, self.db, volume_id=self.vid, cancelled=lambda: True)
        with closing(sqlite3.connect(self.db)) as conn:
            conn.execute('INSERT INTO inventory_scan_owner(id,owner_pid,token,updated_utc) VALUES(1,99999999,?,?)',
                         ('stale-test-token', 'earlier'))
            conn.commit()
        with mock.patch.object(index, '_owner_alive', return_value=False):
            done = index.run_scan(self.drive, self.db, volume_id=self.vid)
        self.assertEqual(done['state'], 'complete')
        self.assertIsNone(self.owner())

    def test_unexpected_failure_releases_lease_and_marks_paused(self):
        with mock.patch.object(index.os, 'scandir', side_effect=RuntimeError('fixture worker failure')):
            with self.assertRaisesRegex(RuntimeError, 'fixture worker failure'):
                index.run_scan(self.drive, self.db, volume_id=self.vid)
        self.assertIsNone(self.owner())
        status = index.index_status(self.db, self.drive, volume_id=self.vid)
        self.assertEqual(status['state'], 'paused')
        self.assertGreater(status['pending_directories'], 0)
        self.assertEqual(index.run_scan(self.drive, self.db, volume_id=self.vid)['state'], 'complete')

    def test_noop_on_completed_index_does_not_claim_or_clear_records(self):
        first = index.run_scan(self.drive, self.db, volume_id=self.vid)
        self.assertEqual(first['state'], 'complete')
        with closing(sqlite3.connect(self.db)) as conn:
            before = (conn.execute('SELECT COUNT(*) FROM entries').fetchone()[0],
                      conn.execute('SELECT COUNT(*) FROM directories').fetchone()[0],
                      conn.execute('SELECT COUNT(*) FROM errors').fetchone()[0])
        again = index.run_scan(self.drive, self.db, volume_id=self.vid)
        self.assertEqual(again['entries'], first['entries'])
        self.assertIsNone(self.owner())
        with closing(sqlite3.connect(self.db)) as conn:
            after = (conn.execute('SELECT COUNT(*) FROM entries').fetchone()[0],
                     conn.execute('SELECT COUNT(*) FROM directories').fetchone()[0],
                     conn.execute('SELECT COUNT(*) FROM errors').fetchone()[0])
        self.assertEqual(before, after)


if __name__ == '__main__':
    unittest.main()
