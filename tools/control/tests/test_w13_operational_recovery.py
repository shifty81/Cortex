"""W13: executable PCC command contract and non-destructive portable layout checks."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CONTROL = Path(__file__).resolve().parents[1]
SOURCE = CONTROL.parents[1]
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

import CortexPCC as pcc
import PCCStoragePaths as paths
import PCCVaultStorage as storage


class W13OperationalRecovery(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "Cortex"
        self.root.mkdir()
        self.volume = self.base / "drive"
        self.volume.mkdir()
        self.env_patch = mock.patch.dict(os.environ, {
            "CORTEX_VAULT_ROOT": str(self.volume),
            "PCC_VAULT_ROOT": str(self.volume),
        })
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)
        # These tests intentionally exercise isolated, non-portable temporary
        # Vaults.  The actual Cortex checkout may be on a marked Windows Vault;
        # never let that live installation policy hijack the fixture's root.
        policy_patch = mock.patch.object(paths, "_portable_drive_root_policy_enabled", return_value=False)
        policy_patch.start()
        self.addCleanup(policy_patch.stop)

    def test_all_storage_parser_choices_have_callable_providers(self):
        pcc.validate_storage_command_contract()
        choices = pcc.build_parser()._actions[1].choices
        self.assertTrue(set(pcc.STORAGE_COMMAND_TARGETS).issubset(set(choices)))
        self.assertEqual(pcc.STORAGE_COMMAND_TARGETS["vault-retention-plan"], "snapshot_retention_plan")
        self.assertEqual(pcc.STORAGE_COMMAND_TARGETS["vault-mirror-all"], "mirror_all_registered")

    def test_one_missing_provider_cannot_break_unrelated_dispatch(self):
        with mock.patch.object(storage, "snapshot_retention_plan", None):
            with self.assertRaises(pcc.PCCError):
                pcc.validate_storage_command_contract()
            with mock.patch.object(storage, "storage_health", return_value={"status": "PASS"}) as healthy:
                self.assertEqual(pcc.dispatch_storage_command("storage-status", self.root), {"status": "PASS"})
                healthy.assert_called_once_with(self.root)

    def test_registered_command_contract_references_existing_parser_commands(self):
        contract = json.loads((SOURCE / "project.control.json").read_text(encoding="utf-8"))
        choices = set(pcc.build_parser()._actions[1].choices)
        commands = contract.get("commands", [])
        verified = 0
        for entry in commands:
            if not isinstance(entry, dict):
                continue
            argv = entry.get("args") or []
            if argv and str(argv[0]).replace("\\", "/").endswith("CortexPCC.py"):
                self.assertGreater(len(argv), 1)
                self.assertIn(argv[1], choices, entry.get("key"))
                verified += 1
        self.assertGreaterEqual(verified, 9)

    def test_vault_mirror_metadata_never_provisions_new_source_project_in_projects(self):
        target = paths.project_vault_dir(self.root)
        self.assertEqual(target.parent, self.volume / "Vault" / "ProjectMirrors")
        storage.ensure_layout(self.root)
        self.assertFalse((self.volume / "projects").exists())
        self.assertTrue(target.is_dir())
        env = storage.dependency_environment(self.root)
        self.assertEqual(env["CORTEX_PROJECTS_ROOT"], str(self.volume / "projects"))
        self.assertEqual(env["CORTEX_LOCAL_GIT_ROOT"], str(self.volume / "Git"))

    def test_existing_legacy_mirror_is_not_moved_or_deleted(self):
        legacy = self.volume / "projects" / paths.project_key(self.root)
        legacy.mkdir(parents=True)
        (legacy / "project.json").write_text('{"schema":"pcc.vault_project.v1"}', encoding="utf-8")
        self.assertEqual(paths.project_vault_dir(self.root), legacy)
        storage.ensure_layout(self.root)
        self.assertTrue((legacy / "project.json").is_file())
        self.assertFalse((self.volume / "Vault" / "ProjectMirrors" / legacy.name).exists())

    def test_normal_source_project_json_does_not_impersonate_vault_mirror(self):
        source = self.volume / "projects" / paths.project_key(self.root)
        source.mkdir(parents=True)
        (source / "project.json").write_text('{"schema":"game.project.v1"}', encoding="utf-8")
        self.assertEqual(paths.project_vault_dir(self.root).parent, self.volume / "Vault" / "ProjectMirrors")
        self.assertNotIn(source, storage._mirror_project_directories(self.volume))

    def test_multi_project_operations_require_real_registered_authority(self):
        missing = storage.mirror_all_registered(self.root)
        self.assertEqual(missing["status"], "FAIL")
        self.assertEqual(missing["registered"], 0)
        missing_plan = storage.reclaim_all_registered_plan(self.root)
        self.assertEqual(missing_plan["status"], "FAIL")
        self.assertFalse(missing_plan["applied"])

    def test_marked_volume_bootstrap_handoff_outside_checkout_wins(self):
        marker = self.volume / ".cortex-volume.json"
        marker.write_text('{"schema":"cortex.volume.v1","volumeId":"test"}', encoding="utf-8")
        preferred = self.volume / ".cortex" / "bootstrap-env.cmd"
        preferred.parent.mkdir(parents=True)
        preferred.write_text('set "CORTEX_VAULT_ROOT=' + str(self.volume) + '"\n', encoding="utf-8")
        source_stale = self.root / ".cortex" / "bootstrap-env.cmd"
        source_stale.parent.mkdir(parents=True)
        source_stale.write_text('set "CORTEX_VAULT_ROOT=D:\\"\n', encoding="utf-8")
        with mock.patch.object(paths, "runtime_volume_root", return_value=self.volume):
            with mock.patch.dict(os.environ, {"CORTEX_RUNTIME_ROOT": str(self.root)}, clear=False):
                self.assertEqual(paths._bootstrap_vault_root(), self.volume)

    def test_mirror_all_runs_only_registered_projects_on_marked_volume(self):
        cortex = self.volume / "Cortex"
        (cortex / "config" / "cortex").mkdir(parents=True)
        source_layout = SOURCE / "config" / "cortex" / "volume_layout.v2.json"
        (cortex / "config" / "cortex" / "volume_layout.v2.json").write_bytes(source_layout.read_bytes())
        (self.volume / ".cortex-volume.json").write_text(
            '{"schema":"cortex.volume.v1","volumeId":"fixture-volume"}', encoding="utf-8"
        )
        one = self.volume / "projects" / "One"
        two = self.volume / "Source" / "Two"
        for project in (one, two):
            project.mkdir(parents=True)
            (project / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="0.1.0"\n', encoding="utf-8")
        registry = self.volume / ".cortex" / "registry" / "project_registry.json"
        registry.parent.mkdir(parents=True)
        registry.write_text(json.dumps({"projects": [
            {"root": "D:/projects/One", "portableRelativeRoot": "projects/One"},
            {"root": "D:/Source/Two", "portableRelativeRoot": "Source/Two"},
            {"root": str(one)},
            {"root": "D:/missing", "portableRelativeRoot": "projects/Missing"},
        ]}), encoding="utf-8")
        result = storage.mirror_all_registered(cortex)
        self.assertEqual(result["status"], "PASS", result)
        self.assertEqual(result["registered"], 2)
        self.assertEqual(result["mirrored"], 2)
        self.assertTrue(all(item["status"] == "PASS" for item in result["results"]))
        self.assertTrue(all(str(item["projectRoot"]).startswith(str(self.volume)) for item in result["results"]))
        self.assertTrue((one / "Cargo.toml").is_file())
        self.assertTrue((two / "Cargo.toml").is_file())

    def test_cli_smoke_does_not_fail_in_function_dispatch(self):
        env = os.environ.copy()
        # The subprocess must discover the synthetic unmarked runtime root,
        # not re-import the installed G:/Cortex portable policy from the host.
        env["CORTEX_RUNTIME_ROOT"] = str(self.root)
        for name in ("storage-status", "storage-health", "vault-retention-plan", "vault-gc-plan", "storage-reclaim-plan-all"):
            with self.subTest(command=name):
                run = subprocess.run(
                    [sys.executable, "-B", str(CONTROL / "CortexPCC.py"), name, "--root", str(self.root)],
                    cwd=str(SOURCE), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    text=True, timeout=15,
                )
                self.assertNotIn("AttributeError", run.stderr)
                self.assertNotIn("invalid choice", run.stderr)
                self.assertIn(run.returncode, (0, 2), run.stderr)
                self.assertIn("schema", run.stdout)


if __name__ == "__main__":
    unittest.main()
