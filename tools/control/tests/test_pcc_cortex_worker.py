from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import PCCCortexWorker as worker


class CortexWorkerTests(unittest.TestCase):
    def test_candidate_uses_cargo_target_dir(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            env = {"CARGO_TARGET_DIR": str(root / "vault-target")}
            expected = root / "vault-target" / "debug" / ("cortex.exe" if os.name == "nt" else "cortex")
            self.assertEqual(worker._candidate_executable(env, root), expected.resolve())

    def test_status_is_structured(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            payload = worker.worker_status(root)
            self.assertEqual(payload["schema"], "pcc.cortex_worker_status.v1")
            self.assertIn(payload["status"], {"READY", "MISSING", "BROKER_ERROR"})


if __name__ == "__main__":
    unittest.main()
