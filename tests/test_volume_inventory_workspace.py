"""R8G: task-oriented read-only Vault view, bounded results, and nonblocking filtering."""
from pathlib import Path
import importlib.util
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tools" / "control" / "CortexVolumeInventoryPresentation.py"
spec = importlib.util.spec_from_file_location("cortex_r8g_inventory_view", SOURCE)
view = importlib.util.module_from_spec(spec)
spec.loader.exec_module(view)


class InventoryWorkspaceTests(unittest.TestCase):
    def setUp(self):
        self.report = {"root": "G:\\", "entries": [
            {"path": "projects", "type": "directory"},
            {"path": "Git", "type": "directory"},
            {"path": "projects/havenwild/README.md", "type": "file"},
            {"path": "Git/Cortex/src/main.rs", "type": "file"},
        ]}

    def test_top_level_default_and_case_preservation(self):
        result = view.select_inventory_entries(self.report)
        self.assertEqual([e["path"] for e in result["rows"]], ["projects", "Git"])
        self.assertEqual(result["total"], 2)

    def test_search_and_bounded_rows(self):
        result = view.select_inventory_entries(self.report, query="GIT/", limit=1)
        self.assertEqual(result["total"], 1)
        self.assertEqual(result["rows"][0]["path"], "Git/Cortex/src/main.rs")
        lots = {"entries": [{"path": f"a/{i}", "type": "file"} for i in range(900)]}
        result = view.select_inventory_entries(lots, query="a/", limit=200)
        self.assertEqual(result["total"], 900)
        self.assertEqual(len(result["rows"]), 200)

    def test_invalid_limit_and_read_only(self):
        snapshot = [row.copy() for row in self.report["entries"]]
        with self.assertRaises(ValueError):
            view.select_inventory_entries(self.report, limit=0)
        view.select_inventory_entries(self.report, query="cortex")
        self.assertEqual(self.report["entries"], snapshot)

    def test_gui_access_gap_view_and_completion_progress_contract(self):
        source = (ROOT / "tools/control/CortexPCCGui.py").read_text(encoding="utf-8")
        backend = (ROOT / "tools/control/CortexPersistentVolumeInventory.py").read_text(encoding="utf-8")
        self.assertIn("def query_access_gaps(", backend)
        self.assertIn('persistent_volume_gap_query if mode == "gaps" else persistent_volume_query', source)
        self.assertIn('self._volume_switch_view("gaps")', source)
        self.assertIn('self.volume_progress.pack_forget()', source)
        self.assertIn('new_stamp < old_stamp', source)
        self.assertIn('self._volume_show_overview()', source)

    def test_gui_uses_dedicated_sections_and_async_filter_events(self):
        text = (ROOT / "tools/control/CortexPCCGui.py").read_text(encoding="utf-8")
        for name in ("Inventory", "Catalog", "Intake", "Storage", "Health"):
            self.assertIn(f'"{name}"', text)
        self.assertIn('self._show_vault_section("Inventory")', text)
        self.assertIn('threading.Thread(target=work, daemon=True).start()', text)
        self.assertIn('"volume-view-done"', text)
        self.assertIn('self._volume_view_generation', text)
        self.assertIn('persistent_volume_scan(volume_root, db_path, volume_id=volume_id,', text)
        self.assertIn('query_fn = persistent_volume_gap_query if mode == "gaps" else persistent_volume_query', text)
        self.assertIn('view = query_fn(db_path, **options)', text)
        self.assertIn('lambda: generation != self._volume_view_generation', text)
        self.assertIn('self._volume_query_match_mode = "prefix" if prefix else "substring"', text)
        self.assertIn('prefix=bool(value)', text)
        self.assertIn('options["exact_total"]', text)
        self.assertIn('self.volume_page_label.configure(', text)
        self.assertIn('self._volume_filter_pending', text)
        self.assertNotIn('value=min(n, 2_000_000)', text)
        self.assertIn('self.volume_progress.start(18)', text)
        self.assertIn('self.volume_progress.start(18)', text)
        self.assertIn('self.volume_progress.stop()', text)
        self.assertNotIn('self._start_volume_inventory()\n        self._build_', text)


if __name__ == "__main__":
    unittest.main()
