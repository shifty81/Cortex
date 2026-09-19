"""UPCC-A03 regression: GUI/wrapper entry must not turn applied self-update into FAIL."""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch
import sys

CONTROL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONTROL))
from test_cortex_upcc_a01_integration import load_pcc


class ImportedRestartTests(unittest.TestCase):
    def test_imported_patch_apply_restart_returns_success(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as tmp:
            controller = Mock()
            controller.patch_apply.side_effect = pcc.PCCRestart('replacement launched')
            with patch.object(pcc, 'CortexPCC', return_value=controller):
                # The parent ProjectControlCenter.py does: int(cortex_main(forwarded)).
                result = int(pcc.main(['patch-apply', '--root', tmp, '--yes']))
            self.assertEqual(result, 0)
            controller.patch_apply.assert_called_once_with(confirm=False)

    def test_interactive_restart_ends_old_interpreter(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as tmp:
            controller = Mock()
            controller.interactive.side_effect = pcc.PCCRestart('replacement launched')
            with patch.object(pcc, 'CortexPCC', return_value=controller):
                self.assertEqual(pcc.main(['interactive', '--root', tmp]), 0)
            controller.interactive.assert_called_once_with()

    def test_normal_apply_failure_stays_failure(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as tmp:
            controller = Mock()
            controller.patch_apply.return_value = 2
            with patch.object(pcc, 'CortexPCC', return_value=controller):
                self.assertEqual(pcc.main(['patch-apply', '--root', tmp]), 2)

    def test_failed_restart_launch_is_not_masked(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as tmp:
            controller = Mock()
            controller.patch_apply.side_effect = OSError('cannot spawn bootstrap')
            with patch.object(pcc, 'CortexPCC', return_value=controller):
                with self.assertRaisesRegex(OSError, 'cannot spawn bootstrap'):
                    pcc.main(['patch-apply', '--root', tmp])

    def test_real_patch_apply_requests_one_restart_and_then_signals(self):
        pcc = load_pcc()
        with tempfile.TemporaryDirectory() as tmp:
            controller = object.__new__(pcc.CortexPCC)
            controller.patch_status = Mock(return_value=(0, {'Invalid': 0, 'Pending': 1}))
            controller.patch = Mock()
            controller.patch.apply.return_value = (0, {'Invalid': 0, 'Applied': 1, 'RestartRequired': True})
            controller.log = Mock()
            controller.restart = Mock()
            with self.assertRaises(pcc.PCCRestart):
                controller.patch_apply(confirm=False)
            controller.restart.assert_called_once_with()
            controller.patch.apply.assert_called_once_with(stream=True)


if __name__ == '__main__':
    unittest.main()
