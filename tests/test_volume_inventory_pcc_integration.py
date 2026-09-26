"""Verify inventory fixtures are mandatory in the authoritative full gate."""
from pathlib import Path
import unittest


class InventoryFullGateContractTests(unittest.TestCase):
    def test_inventory_fixtures_are_mandatory(self):
        source = (Path(__file__).resolve().parents[1] / "tools/control/CortexPCC.py").read_text(encoding="utf-8")
        start = source.index("def run_universal_python_regressions(")
        end = source.index("\ndef run_self_tests(", start)
        gate = source[start:end]
        self.assertIn('root / "tests/test_volume_inventory.py",', gate)
        self.assertIn('root / "tests/test_volume_inventory_gate.py",', gate)
        self.assertIn('root / "tests/test_volume_inventory_workspace.py",', gate)
        self.assertIn('root / "tests/test_persistent_volume_inventory.py",', gate)
        self.assertIn('root / "tests/test_inventory_incremental_refresh.py",', gate)
        self.assertIn('missing = [str(p.relative_to(root))', gate)


if __name__ == "__main__":
    unittest.main()

class RepositoryPlaceholderConvergenceTests(unittest.TestCase):
    """Authoritative FULL-gate fixtures: no tracked source may be removed by convergence."""

    def _fixture(self, root):
        import json
        import subprocess
        repo = root / "Cortex"
        (repo / "config" / "cortex").mkdir(parents=True)
        log = repo / "logs" / "arbiter_engine"
        log.mkdir(parents=True)
        (log / ".gitkeep").write_text("", encoding="utf-8")
        policy = {
            "root_files": [], "root_directories": ["config"],
            "runtime_state": {"logs": ".cortex/logs"},
            "preserve_runtime_placeholders": ["logs/arbiter_engine/.gitkeep"],
            "archive_root_globs": []
        }
        (repo / "config" / "cortex" / "repository_layout.v1.json").write_text(
            json.dumps(policy), encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
        subprocess.run(["git", "-C", str(repo), "add", "logs/arbiter_engine/.gitkeep"],
                       check=True, capture_output=True)
        subprocess.run(["git", "-C", str(repo), "-c", "user.name=Fixture",
                        "-c", "user.email=fixture@example.invalid", "commit", "-qm", "baseline"],
                       check=True, capture_output=True)
        return repo, log

    def _run(self, script, repo, *flags):
        import subprocess
        import sys
        return subprocess.run([sys.executable, "-B", str(Path(__file__).resolve().parents[1] /
                               "tools" / "control" / script), "--root", str(repo), *flags],
                              capture_output=True, text=True)

    def test_runtime_migration_preserves_gitkeep_and_is_idempotent(self):
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            repo, log = self._fixture(Path(tmp))
            (log / "session.log").write_text("diagnostic", encoding="utf-8")
            result = self._run("CortexRepositoryConvergence.py", repo, "--apply")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((log / ".gitkeep").exists())
            self.assertFalse((log / "session.log").exists())
            self.assertEqual((Path(tmp) / ".cortex" / "logs" / "arbiter_engine" /
                              "session.log").read_text(encoding="utf-8"), "diagnostic")
            status = subprocess.run(["git", "-C", str(repo), "status", "--short", "--",
                                     "logs/arbiter_engine/.gitkeep"], capture_output=True, text=True)
            self.assertEqual(status.stdout.strip(), "")
            layout = self._run("CortexRepositoryLayout.py", repo)
            self.assertEqual(layout.returncode, 0, layout.stdout)
            again = self._run("CortexRepositoryConvergence.py", repo, "--apply")
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertIn("Nothing to migrate", again.stdout)

    def test_collision_fails_without_deleting_source(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            repo, log = self._fixture(Path(tmp))
            (log / "session.log").write_text("original", encoding="utf-8")
            target = Path(tmp) / ".cortex" / "logs" / "arbiter_engine" / "session.log"
            target.parent.mkdir(parents=True)
            target.write_text("different", encoding="utf-8")
            result = self._run("CortexRepositoryConvergence.py", repo, "--apply")
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue((log / "session.log").exists())
            self.assertTrue((log / ".gitkeep").exists())
            self.assertEqual(target.read_text(encoding="utf-8"), "different")

    def test_layout_rejects_nonplaceholder_runtime_content(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            repo, log = self._fixture(Path(tmp))
            self.assertEqual(self._run("CortexRepositoryLayout.py", repo).returncode, 0)
            (log / "still-here.log").write_text("x", encoding="utf-8")
            self.assertNotEqual(self._run("CortexRepositoryLayout.py", repo).returncode, 0)

    def test_gui_drive_scan_is_explicit_and_uses_drive_anchor(self):
        source = (Path(__file__).resolve().parents[1] / "tools" / "control" /
                  "CortexPCCGui.py").read_text(encoding="utf-8")
        self.assertIn('"Start / Resume Index", self._start_volume_inventory', source)
        self.assertIn('persistent_volume_location(self.root_path)', source)
        self.assertIn('cancelled=lambda: self._vault_cancel', source)
        self.assertIn('query_fn = persistent_volume_gap_query if mode == "gaps" else persistent_volume_query', source)
        self.assertIn('view = query_fn(db_path, **options)', source)
        self.assertIn('volume-view-done', source)
        self.assertIn('self._show_vault_section("Inventory")', source)
