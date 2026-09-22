from __future__ import annotations
import json
import tempfile
import unittest
from pathlib import Path
from stage_candidate import ADDITIONS, OLD_LAUNCH, OLD_TEST, git_blob, stage


class StagingTests(unittest.TestCase):
    def make_root(self, base: Path) -> tuple[Path, bytes]:
        root = base / 'Cortex'
        (root / 'apps/cortex_desktop').mkdir(parents=True)
        (root / 'apps/cortex_desktop/Cargo.toml').write_text('[package]\nname="cortex_desktop"\n')
        (root / 'project.control.json').write_text(json.dumps({'project': {'id': 'cortex'}}))
        source = root / 'tools/control/CortexPCC.py'
        source.parent.mkdir(parents=True)
        original = ('# fixture\n' + OLD_LAUNCH + '\n' + OLD_TEST + '\n').encode()
        source.write_bytes(original)
        return root, original

    def test_stage_is_review_only_and_preserves_checkout(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            root, original = self.make_root(base)
            result = stage(root, base / 'staging', expected_blob=git_blob(original))
            self.assertFalse(result['sourceCheckoutModified'])
            self.assertEqual((root / 'tools/control/CortexPCC.py').read_bytes(), original)
            self.assertIn('WINDOW_VISIBLE', (base / 'staging/tools/control/CortexPCC.py').read_text())
            self.assertIn('test_cortex_desktop_readiness.py', (base / 'staging/tools/control/CortexPCC.py').read_text())
            self.assertTrue((base / 'staging/review/CortexPCC.diff').is_file())
            for path in ADDITIONS:
                self.assertFalse((root / path).exists())
                self.assertTrue((base / 'staging' / path).is_file())

    def test_wrong_preimage_refused_with_no_output(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            root, _ = self.make_root(base)
            with self.assertRaisesRegex(ValueError, 'preimage mismatch'):
                stage(root, base / 'stage')
            self.assertFalse((base / 'stage').exists())

    def test_refuse_output_inside_source(self):
        with tempfile.TemporaryDirectory() as d:
            root, original = self.make_root(Path(d))
            with self.assertRaisesRegex(ValueError, 'outside'):
                stage(root, root / 'stage', expected_blob=git_blob(original))
            self.assertFalse((root / 'stage').exists())

    def test_refuse_existing_output(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            root, original = self.make_root(base)
            (base / 'stage').mkdir()
            with self.assertRaisesRegex(ValueError, 'already exists'):
                stage(root, base / 'stage', expected_blob=git_blob(original))

    def test_refuse_existing_module(self):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d)
            root, original = self.make_root(base)
            (root / ADDITIONS[0]).write_text('# local newer module')
            with self.assertRaisesRegex(ValueError, 'already exists'):
                stage(root, base / 'stage', expected_blob=git_blob(original))


if __name__ == '__main__':
    unittest.main()
