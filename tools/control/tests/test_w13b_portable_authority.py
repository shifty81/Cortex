"""W13B1: marked volume is an authority, not a transient drive-letter fallback."""
from __future__ import annotations
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))
import PCCStoragePaths as paths
import CortexPythonBridge as bridge

class PortableAuthorityTests(unittest.TestCase):
    def test_marked_volume_and_bridge_have_same_authority(self):
        with tempfile.TemporaryDirectory() as td:
            volume = Path(td) / "portable"
            volume.mkdir()
            (volume / '.cortex-volume.json').write_text(
                json.dumps({'schema': 'cortex.volume.v1', 'volumeId': 'fixture-id'}), encoding='utf-8')
            with mock.patch.object(paths, '_portable_drive_root_policy_enabled', return_value=True), \
                 mock.patch.object(paths, 'runtime_volume_root', return_value=volume), \
                 mock.patch.object(paths, '_require_portable_vault_volume_label'), \
                 mock.patch.dict(os.environ, {'CORTEX_VAULT_ROOT': '', 'PCC_VAULT_ROOT': ''}):
                self.assertEqual(paths.resolve_vault_root(), volume.resolve())
                self.assertEqual(bridge._portable_library_root(CONTROL.parents[1]), volume.resolve())
                with mock.patch.dict(os.environ, {'CORTEX_VAULT_ROOT': str(Path(td) / 'other-drive')}):
                    with self.assertRaisesRegex(RuntimeError, 'conflicting override'):
                        paths.resolve_vault_root()
                    with self.assertRaisesRegex(RuntimeError, 'conflicting override'):
                        bridge._portable_library_root(CONTROL.parents[1])

    def test_missing_or_invalid_marker_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            volume = Path(td)
            with mock.patch.object(paths, '_portable_drive_root_policy_enabled', return_value=True), \
                 mock.patch.object(paths, 'runtime_volume_root', return_value=volume), \
                 mock.patch.object(paths, '_require_portable_vault_volume_label'), \
                 mock.patch.dict(os.environ, {'CORTEX_VAULT_ROOT': '', 'PCC_VAULT_ROOT': ''}):
                with self.assertRaisesRegex(RuntimeError, 'valid volume identity'):
                    paths.resolve_vault_root()
                (volume / '.cortex-volume.json').write_text('{"schema":"wrong","volumeId":"x"}', encoding='utf-8')
                with self.assertRaisesRegex(RuntimeError, 'valid volume identity'):
                    paths.resolve_vault_root()

    def test_nonportable_override_is_still_supported(self):
        with tempfile.TemporaryDirectory() as td:
            with mock.patch.object(paths, '_portable_drive_root_policy_enabled', return_value=False), \
                 mock.patch.dict(os.environ, {'CORTEX_VAULT_ROOT': td, 'PCC_VAULT_ROOT': ''}):
                self.assertEqual(paths.resolve_vault_root(), Path(td).resolve())

if __name__ == '__main__':
    unittest.main()
