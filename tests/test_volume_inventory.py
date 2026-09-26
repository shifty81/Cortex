"""R8A read-only inventory contract tests."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'tools' / 'control' / 'CortexVolumeInventory.py'
spec = importlib.util.spec_from_file_location('cortex_volume_inventory_r8a', MODULE)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class VolumeInventoryTests(unittest.TestCase):
    def test_preserves_lowercase_projects_and_top_level_git(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'projects' / 'havenwild').mkdir(parents=True)
            (root / 'Git' / 'sample').mkdir(parents=True)
            (root / 'projects' / 'havenwild' / 'readme.txt').write_text('hi')
            result = module.inventory(root)
            paths = {entry['path'] for entry in result['entries']}
            self.assertIn('projects/havenwild/readme.txt', paths)
            self.assertIn('Git/sample', paths)
            self.assertFalse(result['errors'])
            self.assertFalse(result['truncated'])
            self.assertEqual((root / 'projects' / 'havenwild' / 'readme.txt').read_text(), 'hi')
            self.assertEqual(result['counts']['file'], 1)

    def test_limit_reports_partial_without_mutating(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ('a', 'b', 'c'):
                (root / name).write_text(name)
            before = sorted(p.name for p in root.iterdir())
            result = module.inventory(root, max_entries=2)
            self.assertTrue(result['truncated'])
            self.assertEqual(len(result['entries']), 2)
            self.assertEqual(before, sorted(p.name for p in root.iterdir()))

    def test_cli_json_stdout(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = subprocess.run([sys.executable, str(MODULE), tmp], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertTrue(json.loads(run.stdout)['read_only'])

    def test_rejects_missing_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(FileNotFoundError):
                module.inventory(Path(tmp) / 'absent')

    def test_does_not_follow_symlink(self):
        with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as outside:
            root = Path(tmp)
            (Path(outside) / 'secret.txt').write_text('secret')
            try:
                (root / 'external').symlink_to(outside, target_is_directory=True)
            except (OSError, NotImplementedError):
                self.skipTest('symlink creation unavailable')
            result = module.inventory(root)
            self.assertIn('external', {e['path'] for e in result['entries']})
            self.assertNotIn('external/secret.txt', {e['path'] for e in result['entries']})


if __name__ == '__main__':
    unittest.main()

class VolumeInventoryExtendedSafetyTests(unittest.TestCase):
    def test_empty_directory_and_nested_hidden_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'projects' / '.hidden').mkdir(parents=True)
            (root / 'projects' / '.hidden' / 'file.txt').write_text('x')
            (root / 'Git').mkdir()
            result = module.inventory(root)
            paths = {entry['path'] for entry in result['entries']}
            self.assertIn('projects/.hidden/file.txt', paths)
            self.assertIn('Git', paths)
            self.assertFalse(result['truncated'])

    def test_no_output_file_created_by_library_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'a.txt').write_text('x')
            before = {p.relative_to(root).as_posix() for p in root.rglob('*')}
            result = module.inventory(root)
            after = {p.relative_to(root).as_posix() for p in root.rglob('*')}
            self.assertEqual(before, after)
            self.assertTrue(result['read_only'])

    def test_limit_one_is_explicitly_partial(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'a').write_text('a')
            (root / 'b').write_text('b')
            result = module.inventory(root, max_entries=1)
            self.assertEqual(len(result['entries']), 1)
            self.assertTrue(result['truncated'])

    def test_invalid_limit_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                module.inventory(tmp, max_entries=0)

    @unittest.skipUnless(sys.platform == 'win32', 'Windows junction test')
    def test_windows_junction_is_not_traversed(self):
        import os
        with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as outside:
            root = Path(tmp)
            (Path(outside) / 'outside.txt').write_text('outside')
            junction = root / 'junction'
            result = subprocess.run(['cmd', '/d', '/c', 'mklink', '/J', str(junction), str(Path(outside))], capture_output=True, text=True)
            if result.returncode != 0:
                self.skipTest('junction creation unavailable')
            try:
                report = module.inventory(root)
                paths = {entry['path'] for entry in report['entries']}
                self.assertIn('junction', paths)
                self.assertNotIn('junction/outside.txt', paths)
            finally:
                os.rmdir(junction)

class VolumeInventoryInteractiveTests(unittest.TestCase):
    def test_progress_and_cancellation_are_partial_and_read_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for index in range(620):
                (root / f"file_{index:04d}.txt").write_text("x", encoding="utf-8")
            seen = []
            stop = [False]
            def progress(payload):
                seen.append(payload["entries"])
                if payload["entries"] >= 500:
                    stop[0] = True
            result = module.inventory(root, progress=progress, cancelled=lambda: stop[0])
            self.assertTrue(result["cancelled"])
            self.assertTrue(result["truncated"])
            self.assertEqual(len(result["entries"]), 500)
            self.assertIn(500, seen)
            self.assertEqual(len(list(root.iterdir())), 620)

    def test_successful_progress_has_final_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "hello").write_text("hello", encoding="utf-8")
            updates = []
            result = module.inventory(root, progress=updates.append)
            self.assertFalse(result["cancelled"])
            self.assertEqual(updates[-1]["entries"], 1)
            self.assertEqual(updates[-1]["file"], 1)

    def test_bounded_presentation_filters_without_persistence(self):
        import importlib.util as _util
        source = MODULE.with_name("CortexVolumeInventoryPresentation.py")
        spec2 = _util.spec_from_file_location("volume_view_r8f", source)
        view = _util.module_from_spec(spec2)
        spec2.loader.exec_module(view)
        report = {"root": "G:\\\\", "read_only": True,
                  "entries": [{"path": "projects", "type": "directory"},
                              {"path": "Git", "type": "directory"},
                              {"path": "projects/havenwild/readme.txt", "type": "file"}],
                  "counts": {"file": 1, "directory": 2}, "errors": [], "truncated": False}
        top = view.render_inventory(report)
        self.assertIn("[directory] projects", top)
        self.assertNotIn("[file] projects/havenwild/readme.txt", top)
        filtered = view.render_inventory(report, query="havenwild")
        self.assertIn("projects/havenwild/readme.txt", filtered)
        self.assertIn("No files imported, moved, hashed, registered", filtered)
        self.assertEqual(len(report["entries"]), 3)
