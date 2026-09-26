"""R8J-A: approved-directory refresh on the existing v1 SQLite index."""
from __future__ import annotations
from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
module_spec = importlib.util.spec_from_file_location("r8j_incremental", ROOT / "tools/control/CortexPersistentVolumeInventory.py")
index = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(index)


class IncrementalRefreshTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.drive = Path(self.tmp.name) / "drive"
        self.drive.mkdir()
        self.vid = "portable-volume-test"
        (self.drive / ".cortex-volume.json").write_text(
            json.dumps({"schema": "cortex.volume.v1", "volumeId": self.vid}), encoding="utf8")
        (self.drive / "vault_catalog.db").write_bytes(b"UNTOUCHED-CATALOG")
        self.db = self.drive / ".cortex/inventory" / self.vid / "inventory.sqlite3"
        (self.drive / "projects" / "havenwild").mkdir(parents=True)
        (self.drive / "projects" / "havenwild" / "old.txt").write_text("first")
        (self.drive / "projects" / "havenwild" / "stay.txt").write_text("one")
        (self.drive / "Git").mkdir()
        self.assertEqual(index.run_scan(self.drive, self.db, volume_id=self.vid)["state"], "complete")

    def status(self):
        return index.index_status(self.db, self.drive, volume_id=self.vid)

    def rows(self, search=""):
        return index.query_entries(self.db, volume_id=self.vid, query=search)["rows"]

    def test_one_directory_reconciles_add_modify_delete_and_preserves_other_roots(self):
        target = self.drive / "projects" / "havenwild"
        (target / "old.txt").unlink()
        (target / "stay.txt").write_text("two two")
        (target / "added.txt").write_text("new")
        before = self.status()["entries"]
        result = index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative="projects/havenwild")
        self.assertEqual(result["refresh"]["added"], 1)
        self.assertEqual(result["refresh"]["removed"], 1)
        self.assertEqual(result["refresh"]["changed"], 1)
        self.assertEqual(result["status"]["entries"], before)
        self.assertEqual(result["status"]["state"], "complete")
        found = {row["path"]: row for row in self.rows("projects/havenwild/")}
        self.assertNotIn("projects/havenwild/old.txt", found)
        self.assertEqual(found["projects/havenwild/stay.txt"]["size_bytes"], 7)
        self.assertIn("projects/havenwild/added.txt", found)
        self.assertIn("Git", [x["path"] for x in self.rows()])
        self.assertEqual((self.drive / "vault_catalog.db").read_bytes(), b"UNTOUCHED-CATALOG")

    def test_removing_directory_reconciles_descendants_and_records(self):
        target = self.drive / "projects" / "havenwild"
        for item in target.iterdir(): item.unlink()
        target.rmdir()
        before = self.status()["entries"]
        outcome = index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative="projects")
        self.assertEqual(outcome["refresh"]["removed"], 1)
        self.assertEqual(outcome["status"]["entries"], before - 3)
        self.assertEqual(outcome["status"]["pending_directories"], 0)
        self.assertEqual(index.query_entries(self.db, volume_id=self.vid, query="projects/havenwild")["total"], 0)

    def test_new_directory_queues_resumable_subtree_not_full_volume(self):
        created = self.drive / "projects" / "newbranch"
        created.mkdir()
        (created / "child.txt").write_text("hello")
        outcome = index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative="projects")
        self.assertEqual(outcome["refresh"]["new_directories"], 1)
        self.assertEqual(outcome["status"]["state"], "paused")
        self.assertEqual(outcome["status"]["pending_directories"], 1)
        completed = index.run_scan(self.drive, self.db, volume_id=self.vid)
        self.assertEqual(completed["state"], "complete")
        self.assertEqual(completed["pending_directories"], 0)
        self.assertEqual(index.query_entries(self.db, volume_id=self.vid, query="projects/newbranch/child.txt")["total"], 1)

    def test_cancel_rolls_back_and_invalid_paths_are_rejected(self):
        target = self.drive / "projects" / "havenwild"
        (target / "added.txt").write_text("new")
        baseline = self.status()
        with self.assertRaises(InterruptedError):
            index.refresh_directory(self.drive, self.db, volume_id=self.vid,
                                    relative="projects/havenwild", cancelled=lambda: True)
        self.assertEqual(self.status()["entries"], baseline["entries"])
        self.assertEqual(index.query_entries(self.db, volume_id=self.vid, query="added.txt")["total"], 0)
        for unsafe in ("../projects", "projects/../Git", "G:/projects", "/projects", "projects//havenwild"):
            with self.subTest(unsafe=unsafe), self.assertRaises(ValueError):
                index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative=unsafe)
        with self.assertRaises(ValueError):
            index.refresh_directory(self.drive, self.db, volume_id="wrong-id")

    def test_unreadable_directory_preserves_existing_records(self):
        target = self.drive / "projects" / "havenwild"
        before = self.status()
        original_scandir = os.scandir
        def refuse(path):
            if Path(path) == target: raise PermissionError("fixture access denied")
            return original_scandir(path)
        with mock.patch.object(index.os, "scandir", side_effect=refuse):
            report = index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative="projects/havenwild")
        self.assertIn("unreadable", report["refresh"])
        self.assertEqual(report["status"]["entries"], before["entries"])
        self.assertEqual(report["status"]["errors"], before["errors"] + 1)
        self.assertEqual(index.query_entries(self.db, volume_id=self.vid, query="old.txt")["total"], 1)

    def test_replaced_directory_link_is_not_followed(self):
        folder = self.drive / "projects" / "havenwild"
        destination = Path(self.tmp.name) / "outside"
        destination.mkdir()
        (destination / "secret.txt").write_text("outside", encoding="utf8")
        for item in folder.iterdir(): item.unlink()
        folder.rmdir()
        try:
            os.symlink(destination, folder, target_is_directory=True)
        except (OSError, NotImplementedError):
            self.skipTest("directory symlink creation unavailable")
        with self.assertRaisesRegex(ValueError, "cannot traverse"):
            index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative="projects/havenwild")
        self.assertEqual(index.query_entries(self.db, volume_id=self.vid, query="secret.txt")["total"], 0)
        self.assertEqual((destination / "secret.txt").read_text(encoding="utf8"), "outside")

    def test_completed_index_version_and_concurrent_scan_guard(self):
        with closing(sqlite3.connect(self.db)) as conn:
            self.assertEqual(conn.execute("PRAGMA user_version").fetchone()[0], 1)
            conn.execute("UPDATE scan_state SET state='running' WHERE id=1")
            conn.commit()
        with self.assertRaisesRegex(RuntimeError, "completed inventory"):
            index.refresh_directory(self.drive, self.db, volume_id=self.vid, relative="projects")

if __name__ == "__main__": unittest.main()
