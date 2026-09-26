from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
ROOT = CONTROL.parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPythonBridge as bridge  # noqa: E402


class PCCConsoleChatRuntimeTests(unittest.TestCase):
    def test_native_health_url_targets_server_health_not_v1_health(self) -> None:
        self.assertEqual(
            bridge._provider_health_url("http://127.0.0.1:12400/v1"),
            "http://127.0.0.1:12400/health",
        )

    def test_model_host_candidate_follows_transactional_worker_target_dir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            worker = td / "debug" / "cortex.exe"
            worker.parent.mkdir(parents=True)
            worker.write_bytes(b"")
            with mock.patch.object(bridge, "_resolve_runtime", return_value=worker), \
                 mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": ""}, clear=False):
                candidates = bridge._candidate_model_host_paths(ROOT)
            self.assertIn((worker.parent / "cortex_model_host.exe").resolve(), candidates)

    def test_portable_library_root_prefers_vault_environment(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            vault = Path(td) / "vault"
            vault.mkdir()
            with mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": str(vault)}, clear=False):
                self.assertEqual(bridge._portable_library_root(ROOT), vault.resolve())

    def test_gui_passes_stable_owner_pid_to_bridge(self) -> None:
        source = (CONTROL / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn('"--owner-pid", str(os.getpid())', source)
        self.assertIn('PCC-GUI-0.15.1', source)
        self.assertIn('!<shell command> runs the project shell', source)
        self.assertIn('if lower in {"!", "!command"}:', source)

    def test_full_gate_buffers_successful_negative_fixture_output(self) -> None:
        source = (CONTROL / "CortexPCC.py").read_text(encoding="utf-8")
        self.assertIn('"unittest", "-v", "-b"', source)
        self.assertIn('debug bundle (lightweight evidence path)', source)
        self.assertIn('stream=False, phase="gate:python-pcc-tests"', source)
        self.assertIn('Python PCC regressions:', source)
        self.assertNotIn('debug bundle (lightweight failure path)', source)


if __name__ == "__main__":
    unittest.main()
