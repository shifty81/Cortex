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
