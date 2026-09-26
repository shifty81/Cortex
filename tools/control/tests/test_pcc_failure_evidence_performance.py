from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


class FailureEvidencePerformanceTests(unittest.TestCase):
    def test_gate_failure_evidence_does_not_run_deep_vault_walks(self) -> None:
        source = (ROOT / "CortexPCC.py").read_text(encoding="utf-8")
        start = source.index("class EvidenceBuilder:")
        end = source.index("\n\nclass CortexPCC:", start)
        evidence = source[start:end]
        self.assertNotIn("vault_storage.storage_health(self.ctx.root)", evidence)
        self.assertNotIn("vault_storage.snapshot_retention_plan(self.ctx.root)", evidence)
        self.assertNotIn("vault_storage.cas_gc_plan(self.ctx.root)", evidence)
        self.assertIn('"mode": "LIGHTWEIGHT"', evidence)
        self.assertIn("Evidence: collecting lightweight Vault/storage summary", evidence)

    def test_runtime_versions_make_restart_obvious(self) -> None:
        host = (ROOT / "PCCOperationHost.py").read_text(encoding="utf-8")
        shared = (ROOT / "PCCSharedEnvironment.py").read_text(encoding="utf-8")
        gui = (ROOT / "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn('VERSION = "PCC-OPERATION-HOST-0.4"', host)
        self.assertIn('BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.4"', shared)
        self.assertIn("PCC-GUI-0.15.1", gui)


if __name__ == "__main__":
    unittest.main()
