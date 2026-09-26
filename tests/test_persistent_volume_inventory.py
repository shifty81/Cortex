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

    def test_access_gaps_are_paged_searchable_literal_and_read_only(self):
        self._scan()
        diagnostics = [
            (f"projects/blocked/item_{i:03d}", "scandir" if i % 2 else "stat",
             f"PermissionError: sample_{i:03d}") for i in range(278)
        ]
        diagnostics.extend([
            ("projects/100%_literal", "scandir", "literal matching"),
            (r"Git/back\slash", "stat", "escaped path"),
        ])
        with closing(sqlite3.connect(self.index_file)) as conn:
            conn.executemany("INSERT INTO errors(path,operation,message) VALUES(?,?,?)", diagnostics)
            conn.commit()
        before = (self.drive / "vault_catalog.db").read_bytes()
        page1 = index.query_access_gaps(self.index_file, volume_id=self.volume_id, page_size=70)
        self.assertEqual(page1["total"], 280)
        self.assertEqual(len(page1["rows"]), 70)
        self.assertTrue(page1["has_next"])
        last = index.query_access_gaps(self.index_file, volume_id=self.volume_id, page=3, page_size=70)
        self.assertEqual(len(last["rows"]), 70)
        self.assertFalse(last["has_next"])
        escaped = index.query_access_gaps(self.index_file, volume_id=self.volume_id, query="100%_literal")
        self.assertEqual(escaped["total"], 1)
        self.assertEqual(escaped["rows"][0]["path"], "projects/100%_literal")
        slashed = index.query_access_gaps(self.index_file, volume_id=self.volume_id, query=r"back\slash")
        self.assertEqual(slashed["total"], 1)
        message = index.query_access_gaps(self.index_file, volume_id=self.volume_id, query="SAMPLE_002")
        self.assertEqual(message["total"], 1)
        self.assertEqual(message["rows"][0]["operation"], "stat")
        self.assertEqual((self.drive / "vault_catalog.db").read_bytes(), before)
        self.assertEqual(index.index_status(self.index_file, self.drive, volume_id=self.volume_id)["errors"], 280)
        with self.assertRaises(ValueError):
            index.query_access_gaps(self.index_file, volume_id="another")
        with self.assertRaises(ValueError):
            index.query_access_gaps(self.index_file, volume_id=self.volume_id, page_size=501)
        with self.assertRaises(ValueError):
            index.query_access_gaps(self.index_file, volume_id=self.volume_id, page=-1)

    def test_root_shortcuts_use_indexed_prefix_and_filter_remains_substring(self):
        project = self.drive / "projects" / "havenwild"
        project.mkdir(parents=True)
        (project / "readme.txt").write_text("x")
        (self.drive / "Git").mkdir()
        (self.drive / "other-projects").mkdir()
        (self.drive / "other-projects" / "readme.txt").write_text("x")
        self._scan()
        scoped = index.query_entries(self.index_file, volume_id=self.volume_id,
                                     query="projects/", match_mode="prefix")
        self.assertEqual(scoped["match_mode"], "prefix")
        self.assertIn("projects/havenwild/readme.txt", [r["path"] for r in scoped["rows"]])
        self.assertNotIn("other-projects/readme.txt", [r["path"] for r in scoped["rows"]])
        substring = index.query_entries(self.index_file, volume_id=self.volume_id,
                                        query="projects/")
        self.assertIn("other-projects/readme.txt", [r["path"] for r in substring["rows"]])
        with closing(sqlite3.connect(self.index_file)) as db:
            plan = db.execute("EXPLAIN QUERY PLAN SELECT COUNT(*) FROM entries "
                              "WHERE path_fold >= ? AND path_fold < ?",
                              ("projects/", "projects/" + chr(0x10ffff))).fetchall()
        self.assertTrue(any("SEARCH entries USING COVERING INDEX idx_inventory_fold" in str(r[3])
                            for r in plan), plan)
        with self.assertRaises(ValueError):
            index.query_entries(self.index_file, volume_id=self.volume_id, match_mode="unsupported")

    def test_interactive_substring_paging_does_not_require_exact_count(self):
        target = self.drive / "projects"
        target.mkdir()
        for n in range(13):
            (target / f"slow_match_{n:03d}.txt").write_text("x")
        self._scan()
        initial = index.query_entries(self.index_file, volume_id=self.volume_id,
                                      query="slow_match", page_size=5, exact_total=False)
        self.assertIsNone(initial["total"])
        self.assertEqual(len(initial["rows"]), 5)
        self.assertTrue(initial["has_next"])
        second = index.query_entries(self.index_file, volume_id=self.volume_id,
                                     query="slow_match", page=1, page_size=5, exact_total=False)
        self.assertIsNone(second["total"])
        self.assertTrue(second["has_next"])
        final = index.query_entries(self.index_file, volume_id=self.volume_id,
                                    query="slow_match", page=2, page_size=5, exact_total=False)
        self.assertEqual(final["total"], 13)
        self.assertEqual(len(final["rows"]), 3)
        self.assertFalse(final["has_next"])
        exact = index.query_entries(self.index_file, volume_id=self.volume_id,
                                    query="slow_match", page_size=5)
        self.assertEqual(exact["total"], 13)
        self.assertTrue(exact["has_next"])
        self.assertEqual((self.drive / "vault_catalog.db").read_bytes(), self.initial_catalog)

    def test_superseded_query_aborts_and_next_access_gap_page_remains_available(self):
        self._scan()
        # Force an actual SQLite VM scan so the progress handler observes cancellation
        # *after* the query started, not only at its initial guard.
        with closing(sqlite3.connect(self.index_file)) as db:
            db.executemany("INSERT INTO entries(path,parent,path_fold,type) VALUES(?,?,?,'file')",
                ((f"fixture/{n:06d}", "fixture", f"fixture/{n:06d}") for n in range(12000)))
            db.execute("INSERT INTO errors(path,operation,message) VALUES('locked','scandir','permission denied')")
            db.commit()
        calls = [0]
        def obsolete():
            calls[0] += 1
            return calls[0] > 1
        with self.assertRaisesRegex(RuntimeError, "superseded"):
            index.query_entries(self.index_file, volume_id=self.volume_id,
                                query="never-appears", cancelled=obsolete)
        self.assertGreater(calls[0], 1)
        with self.assertRaisesRegex(RuntimeError, "superseded"):
            index.query_access_gaps(self.index_file, volume_id=self.volume_id, cancelled=lambda: True)
        result = index.query_access_gaps(self.index_file, volume_id=self.volume_id)
        self.assertEqual(result["total"], 1)
        self.assertEqual(result["rows"][0]["path"], "locked")
        self.assertEqual((self.drive / "vault_catalog.db").read_bytes(), self.initial_catalog)

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
