"""Mandatory R8H fixtures: bounded paging, durable resume, volume authority and source safety."""
from __future__ import annotations
import importlib.util
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tools/control/CortexPersistentVolumeInventory.py"
spec = importlib.util.spec_from_file_location("r8h_inventory", SOURCE)
index = importlib.util.module_from_spec(spec)
spec.loader.exec_module(index)


class PersistentVolumeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.drive = Path(self.tmp.name) / "drive"
        self.drive.mkdir()
        self.volume_id = "fixture-volume-1"
        (self.drive / ".cortex-volume.json").write_text(
            json.dumps({"schema": "cortex.volume.v1", "volumeId": self.volume_id}), encoding="utf-8")
        self.index_file = self.drive / ".cortex" / "inventory" / self.volume_id / "inventory.sqlite3"
        (self.drive / "vault_catalog.db").write_bytes(b"SOURCE_CATALOG_NOT_TO_BE_TOUCHED")
        self.initial_catalog = (self.drive / "vault_catalog.db").read_bytes()

    def _scan(self, **options):
        return index.run_scan(self.drive, self.index_file, volume_id=self.volume_id, **options)

    def test_many_files_no_entry_cap_paged_query_and_literal_filter(self):
        target = self.drive / "projects" / "havenwild"
        target.mkdir(parents=True)
        for n in range(2150):
            (target / f"item_{n:04}.txt").write_text("ok", encoding="utf-8")
        (self.drive / "Git").mkdir()
        (target / "100%_literal.txt").write_text("literal", encoding="utf-8")
        report = self._scan(batch_size=128)
        self.assertEqual(report["state"], "complete")
        self.assertGreater(report["entries"], 2150)
        self.assertEqual(report["pending_directories"], 0)
        top = index.query_entries(self.index_file, volume_id=self.volume_id)
        self.assertIn("projects", [row["path"] for row in top["rows"]])
        self.assertIn("Git", [row["path"] for row in top["rows"]])
        page = index.query_entries(self.index_file, volume_id=self.volume_id,
                                   query="projects/havenwild/item", page=2, page_size=70)
        self.assertEqual(len(page["rows"]), 70)
        self.assertEqual(page["total"], 2150)
        self.assertEqual(page["page"], 2)
        literal = index.query_entries(self.index_file, volume_id=self.volume_id, query="100%_literal")
        self.assertEqual(literal["total"], 1)
        self.assertEqual((self.drive / "vault_catalog.db").read_bytes(), self.initial_catalog)
        self.assertTrue(all(str(row["path"]).find("inventory.sqlite3") == -1 for row in page["rows"]))
        # A second ordinary Start/Resume never resets an already complete index.
        self.assertEqual(self._scan()["entries"], report["entries"])

    def test_cancel_then_resume_preserves_partial_entries_and_queue(self):
        target = self.drive / "projects"
        target.mkdir()
        for n in range(400):
            (target / f"a{n:04}.txt").write_text("x")
        checks = [0]
        def stop_after_partial():
            checks[0] += 1
            return checks[0] > 95
        partial = self._scan(batch_size=32, cancelled=stop_after_partial)
        self.assertEqual(partial["state"], "paused")
        self.assertGreater(partial["pending_directories"], 0)
        retained = index.index_status(self.index_file, self.drive, volume_id=self.volume_id)
        self.assertEqual(retained["entries"], partial["entries"])
        self.assertGreater(retained["entries"], 70)
        final = self._scan(batch_size=64)
        self.assertEqual(final["state"], "complete")
        rows = index.query_entries(self.index_file, volume_id=self.volume_id, query="projects/a", page_size=500)
        self.assertEqual(rows["total"], 400)
        self.assertEqual(len(rows["rows"]), 400)
        self.assertEqual(final["counts"]["file"], 402)  # marker, Vault catalog + fixture files
        self.assertEqual((self.drive / "vault_catalog.db").read_bytes(), self.initial_catalog)

    def test_volume_identity_guard_and_read_only_missing_database(self):
        self.assertEqual(index.index_status(self.index_file, self.drive)["state"], "not_started")
        self.assertFalse(self.index_file.exists())
        self._scan()
        with self.assertRaises(ValueError):
            index.query_entries(self.index_file, volume_id="different")
        with self.assertRaises(ValueError):
            index.index_status(self.index_file, self.drive, volume_id="different")
        with self.assertRaises(ValueError):
            index.run_scan(self.drive, self.index_file, volume_id="different")
        with self.assertRaises(ValueError):
            index.query_entries(self.index_file, volume_id=self.volume_id, page_size=600)

    def test_inventory_index_excluded_and_source_links_unfollowed(self):
        (self.drive / "projects").mkdir()
        (self.drive / "projects" / "a.txt").write_text("x")
        try:
            os.symlink(self.drive / "projects", self.drive / "shortcut", target_is_directory=True)
        except (OSError, NotImplementedError):
            pass
        report = self._scan()
        self.assertEqual(report["state"], "complete")
        self.assertTrue(self.index_file.is_file())
        own = index.query_entries(self.index_file, volume_id=self.volume_id, query="inventory")
        self.assertFalse(any(row["path"].endswith("inventory.sqlite3") for row in own["rows"]))
        if (self.drive / "shortcut").is_symlink():
            self.assertEqual(report["counts"]["symlink"], 1)
        # sqlite3.Connection.__exit__ commits/rolls back; it does NOT close the handle.
        # Explicit closing is required before TemporaryDirectory cleanup on Windows.
        with closing(sqlite3.connect(self.index_file)) as db:
            boundary = db.execute("SELECT boundary FROM entries WHERE path='.cortex/inventory/fixture-volume-1'").fetchone()
        self.assertEqual(boundary[0], "inventory_index")
        with self.assertRaises(sqlite3.ProgrammingError):
            db.execute("SELECT 1")  # Regression: the temporary DB handle is closed.

    def test_portable_marker_keeps_index_identity_after_mount_rename(self):
        layout = ROOT / "config" / "cortex" / "volume_layout.v2.json"
        repo_config = self.drive / "Cortex" / "config" / "cortex"
        repo_config.mkdir(parents=True)
        (repo_config / "volume_layout.v2.json").write_bytes(layout.read_bytes())
        control = str(ROOT / "tools" / "control")
        sys.path.insert(0, control)
        try:
            mount, located, identity = index.index_location(self.drive / "Cortex")
            self.assertEqual(identity, self.volume_id)
            self.assertEqual(mount, self.drive)
            self.assertEqual(located, self.index_file)
            completed = self._scan()
            renamed = self.drive.parent / "alternate-mount"
            self.drive.rename(renamed)
            self.drive = renamed
            self.index_file = renamed / ".cortex" / "inventory" / self.volume_id / "inventory.sqlite3"
            mount2, located2, identity2 = index.index_location(renamed / "Cortex")
            self.assertEqual(identity2, identity)
            self.assertEqual(located2, self.index_file)
            self.assertEqual(index.index_status(located2, mount2, volume_id=identity)["entries"], completed["entries"])
            self.assertEqual(index.query_entries(located2, volume_id=identity, query="vault_catalog")["total"], 1)
        finally:
            sys.path.remove(control)

    def test_explicit_restart_and_error_recording(self):
        (self.drive / "hello.txt").write_text("hi")
        a = self._scan()
        (self.drive / "another.txt").write_text("added")
        self.assertEqual(self._scan()["entries"], a["entries"])
        b = self._scan(restart=True)
        self.assertEqual(b["entries"], a["entries"] + 1)
        with self.assertRaises(ValueError):
            self._scan(batch_size=0)
        with self.assertRaises(ValueError):
            index.list_errors(self.index_file, volume_id=self.volume_id, limit=101)

if __name__ == "__main__":
    unittest.main()
