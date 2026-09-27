"""W11 black-box regressions from the Havenwild false-empty repair, synthetic only."""
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

import CortexPythonBridge as bridge
import PCCBuildDoctor as doctor


class W11RepairGroundingTests(unittest.TestCase):
    def test_project_local_pcc_logs_are_discoverable_without_artifacts_folder(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            log = root / ".pcc" / "logs" / "pcc-session.log"
            log.parent.mkdir(parents=True)
            log.write_text("FULL GATE: FAIL\nEND cargo-check: FAIL (101)\nerror[E0308]: mismatched types\nerror: could not compile `wgpu-hal`\n", encoding="utf-8")
            report = doctor.latest_failure(root)
            self.assertEqual(report['status'], 'FAILURE_CLASSIFIED')
            self.assertEqual(report['classification']['category'], 'SOURCE_COMPILE')
            self.assertIn('cargo-check', report['classification']['stage'])
            self.assertTrue(any('E0308' in row for row in report['excerpt']))

    def test_normal_full_pcc_success_is_not_stale_build_failure(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            logs = root / ".pcc" / "logs"
            logs.mkdir(parents=True)
            failed = logs / "old.log"
            passed = logs / "new-pcc-session.log"
            failed.write_text("FULL GATE: FAIL\nerror[E0308]: mismatched types\n", encoding="utf-8")
            passed.write_text("FULL GATE: PASS\n", encoding="utf-8")
            os.utime(failed, (100, 100))
            os.utime(passed, (200, 200))
            report = doctor.latest_failure(root)
            self.assertEqual(report['status'], 'NO_ACTIVE_FAILURE')
            self.assertEqual(report['latestGate']['status'], 'PASS')

    def test_missing_source_manifest_never_erases_real_root_markers(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'src').mkdir()
            (root / 'Cargo.toml').write_text('[package]\nname = "fixture"\n', encoding='utf-8')
            (root / 'PCC.cmd').write_text('@echo off\n', encoding='utf-8')
            evidence = bridge._workspace_root_evidence(root)
            self.assertIn('Cargo.toml', evidence)
            self.assertIn('src', evidence)
            self.assertIn('PCC.cmd', evidence)
            self.assertIn('do NOT mean this folder is empty', evidence)

    def test_context_retains_authoritative_root_even_when_diagnostics_are_large(self):
        with tempfile.TemporaryDirectory() as td, mock.patch('PCCBuildDoctor.latest_failure') as latest:
            root = Path(td)
            (root / 'Cargo.toml').write_text('[package]\nname = "fixture"\n', encoding='utf-8')
            latest.return_value = {'status': 'FAILURE_CLASSIFIED', 'excerpt': ['error[E0308]'] * 5000}
            evidence = bridge._workspace_context(root, include_logs=True)
            self.assertTrue(evidence.startswith(f'Workspace: {root}'))
            self.assertIn('Verified root markers', evidence)
            self.assertIn('BuildDoctor', evidence)
            self.assertLessEqual(len(evidence), bridge.CONTEXT_TEXT_LIMIT + 2)

    def test_readonly_chat_stays_fast_without_doctor_or_recursion(self):
        with tempfile.TemporaryDirectory() as td, \
                mock.patch('PCCBuildDoctor.latest_failure', side_effect=AssertionError('no diagnostic traversal')):
            root = Path(td)
            (root / 'Cargo.toml').write_text('[package]\n', encoding='utf-8')
            evidence = bridge._workspace_context(root, include_logs=False)
            self.assertIn('Cargo.toml', evidence)

    def test_long_conversation_is_bounded_without_truncating_current_request(self):
        with mock.patch.object(bridge, 'load_messages', return_value=[{'role':'assistant','content':'x'*100000}]*20), \
                mock.patch.object(bridge, '_workspace_context', return_value='root evidence'):
            text = bridge._worker_request_text(Path('cortex'), Path('project'), 'cid', 'repair', 'repair exactly this file')
            self.assertIn('Current user request:\nrepair exactly this file', text)
            self.assertLessEqual(len(text), 7000)


if __name__ == '__main__':
    unittest.main()
