"""Offline fixture tests. No real project writes, no Cargo/model invocation."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import W12_GREEN_APPLY as w12

FIXTURES = Path(__file__).parent / "fixtures"


class W12GreenSourceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='cortex-w12-fixture-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.sources = {name: (FIXTURES / name).read_bytes() for name in w12.GREEN_SOURCE_BLOBS}
        for name, data in self.sources.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)

    def compat(self):
        return patch.dict(w12.GREEN_SOURCE_BLOBS,
                          {name: w12.git_blob_hash(data) for name,data in self.sources.items()})

    def test_w11_repair_fixture_generates_survival_guard_and_failure_receipt(self):
        changed = w12.apply_transforms(self.sources)
        cli = changed['crates/cortex_cli/src/lib.rs'].decode()
        core = changed['crates/cortex_core/src/lib.rs'].decode()
        self.assertIn('candidate_not_active_after_model', cli)
        self.assertIn('no project FULL was invoked', cli)
        self.assertLess(cli.index('candidate_precheck_status ='), cli.index('let verification = if checkpoint_skip_reason'))
        self.assertIn('"schema_version": 3', cli)
        self.assertIn('unexecuted_tool_call_in_model_report', cli)
        self.assertIn('failure_class: Option<String>', core)
        self.assertIn('rollback_unchanged', core)
        self.assertNotIn('secret-value-in-source; found 0".into()', core)

    def test_hash_mismatch_blocks_all_changes_before_backup(self):
        with self.assertRaisesRegex(ValueError,'GREEN preimage mismatch'):
            w12.install(self.root)
        for name,data in self.sources.items():
            self.assertEqual((self.root/name).read_bytes(), data)
        self.assertFalse((self.root/'artifacts').exists())

    def test_install_backs_up_both_source_files_and_rejects_second_application(self):
        with self.compat():
            destination = w12.install(self.root)
            for name, original in self.sources.items():
                self.assertEqual((destination/name).read_bytes(), original)
                self.assertNotEqual((self.root/name).read_bytes(), original)
            manifest = destination/'manifest.json'
            self.assertTrue(manifest.is_file())
            changed_before = {n: (self.root/n).read_bytes() for n in self.sources}
            with self.assertRaisesRegex(ValueError,'preimage mismatch'):
                w12.install(self.root)
            self.assertEqual(changed_before, {n: (self.root/n).read_bytes() for n in self.sources})

    def test_changed_anchor_fails_before_any_file_write(self):
        f = self.root/'crates/cortex_cli/src/lib.rs'
        altered = f.read_bytes().replace(b'fn project_agent_command(', b'fn project_agent_command_missing(')
        f.write_bytes(altered)
        with patch.dict(w12.GREEN_SOURCE_BLOBS, {n:w12.git_blob_hash((self.root/n).read_bytes()) for n in self.sources}):
            with self.assertRaisesRegex(ValueError,'precheck helper'):
                w12.install(self.root)
        self.assertEqual(f.read_bytes(),altered)
        self.assertFalse((self.root/'artifacts').exists())


if __name__=='__main__':
    unittest.main()
