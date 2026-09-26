from __future__ import annotations

import json
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

import PCCStoragePaths as storage  # noqa: E402
import PCCSurfaceCommon as surface  # noqa: E402
import PCCVaultStorage as vault_storage  # noqa: E402


class PortableVolumeTests(unittest.TestCase):
    def test_portable_policy_outranks_stale_bootstrap_drive(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            volume = Path(td) / "portable-volume"
            volume.mkdir()
            stale = Path(td) / "missing-old-volume"
            with mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": "", "PCC_VAULT_ROOT": ""}, clear=False), \
                 mock.patch.object(storage, "_portable_drive_root_policy_enabled", return_value=True), \
                 mock.patch.object(storage, "runtime_volume_root", return_value=volume), \
                 mock.patch.object(storage, "_bootstrap_vault_root", return_value=stale):
                self.assertEqual(storage.resolve_vault_root(ROOT), volume.resolve())

    def test_explicit_vault_override_remains_authoritative(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            explicit = Path(td) / "explicit"
            portable = Path(td) / "portable"
            with mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": str(explicit), "PCC_VAULT_ROOT": ""}, clear=False), \
                 mock.patch.object(storage, "_portable_drive_root_policy_enabled", return_value=True), \
                 mock.patch.object(storage, "runtime_volume_root", return_value=portable):
                self.assertEqual(storage.resolve_vault_root(ROOT), explicit.resolve())

    def test_registry_id_is_stable_for_same_volume_relative_project(self) -> None:
        with mock.patch.object(surface, "portable_relative_to_runtime_volume", return_value="Source/Havenwild_Bevy"):
            first = surface.ProjectRegistry._registry_id(Path("/mnt/letter-e/Source/Havenwild_Bevy"))
            second = surface.ProjectRegistry._registry_id(Path("/mnt/letter-g/Source/Havenwild_Bevy"))
        self.assertEqual(first, second)

    def test_registry_entry_rebinds_from_portable_relative_root(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            registry_path = Path(td) / "registry.json"
            registry_path.write_text(json.dumps({
                "schema": surface.ProjectRegistry.SCHEMA,
                "activeProject": "abc",
                "projects": [{
                    "registryId": "abc",
                    "projectId": "fixture",
                    "name": "Fixture",
                    "kind": "project",
                    "root": "G:/Source/Fixture",
                    "portableRelativeRoot": "Source/Fixture",
                    "lastOpenedUtc": "",
                }],
            }), encoding="utf-8")
            rebound = Path(td) / "E-drive" / "Source" / "Fixture"
            with mock.patch.object(surface, "resolve_portable_volume_path", return_value=rebound):
                entry = surface.ProjectRegistry(registry_path).entries()[0]
            self.assertEqual(entry.root, rebound)

    def test_launcher_uses_its_own_drive_for_shared_python(self) -> None:
        source = (ROOT / "PROJECT_CONTROL_CENTER.cmd").read_text(encoding="utf-8")
        self.assertIn('%~d0\\shared\\toolchains\\python', source)
        self.assertNotIn('for %%D in (E D C)', source)
        self.assertIn('CORTEX_PORTABLE_VOLUME_ROOT=%~d0\\', source)

    def test_dependency_environment_exports_portable_authorities(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            vault = Path(td) / "vault"
            root = Path(td) / "project"
            root.mkdir()
            with mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": str(vault)}, clear=False):
                env = vault_storage.dependency_environment(root)
            self.assertEqual(Path(env["CORTEX_LIBRARY_ROOT"]), vault.resolve())
            self.assertEqual(Path(env["CORTEX_MODELS_ROOT"]), vault.resolve() / "Models")
            self.assertEqual(Path(env["CORTEX_PORTABLE_VOLUME_ROOT"]), vault.resolve())

    def test_model_host_prefers_portable_models_environment(self) -> None:
        source = (ROOT / "apps" / "cortex_model_host" / "src" / "main.rs").read_text(encoding="utf-8")
        self.assertIn('var_os("CORTEX_MODELS_ROOT")', source)
        self.assertLess(source.index('var_os("CORTEX_MODELS_ROOT")'), source.index('var_os("CORTEX_LIBRARY_ROOT")'))

    def test_legacy_old_drive_registration_is_deduped_against_portable_project(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            portable_path = td / "portable.json"
            legacy_path = td / "legacy.json"
            portable_path.write_text(json.dumps({
                "schema": surface.ProjectRegistry.SCHEMA,
                "activeProject": "portable-id",
                "projects": [{
                    "registryId": "portable-id",
                    "projectId": "cortex",
                    "name": "Cortex",
                    "kind": "rust-workspace",
                    "root": "E:/Cortex",
                    "portableRelativeRoot": "Cortex",
                    "lastOpenedUtc": "",
                }],
            }), encoding="utf-8")
            legacy_path.write_text(json.dumps({
                "schema": surface.ProjectRegistry.SCHEMA,
                "activeProject": "legacy-id",
                "projects": [{
                    "registryId": "legacy-id",
                    "projectId": "cortex",
                    "name": "Cortex",
                    "kind": "rust-workspace",
                    "root": "G:/Cortex",
                    "lastOpenedUtc": "",
                }],
            }), encoding="utf-8")
            # The user's *real* checkout can be G:/Cortex while the fixture
            # claims E:/Cortex is its portable location. Do not let this test
            # mistake an existing independent G: checkout for a stale record.
            # Model only the historical G:/Cortex record as absent, leaving
            # every other filesystem existence check authoritative.
            real_exists = Path.exists
            def fixture_exists(path):
                if str(path).replace("\\", "/").casefold() == "g:/cortex":
                    return False
                return real_exists(path)
            with mock.patch.object(surface.ProjectRegistry, "default_path", return_value=portable_path), \
                 mock.patch.object(surface.ProjectRegistry, "legacy_default_path", return_value=legacy_path), \
                 mock.patch.object(surface, "resolve_portable_volume_path", return_value=Path("E:/Cortex")), \
                 mock.patch.object(Path, "exists", autospec=True, side_effect=fixture_exists):
                registry = surface.ProjectRegistry()
                rows = registry.entries()
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0].registry_id, "portable-id")

    def test_live_same_named_legacy_root_is_not_silently_merged(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            portable_path = td / "portable.json"
            legacy_path = td / "legacy.json"
            independent_root = td / "independent-volume" / "Cortex"
            independent_root.mkdir(parents=True)
            portable_path.write_text(json.dumps({"schema": surface.ProjectRegistry.SCHEMA,
                "projects": [{"registryId": "portable-id", "projectId": "cortex", "name": "Cortex",
                              "kind": "rust-workspace", "root": "E:/Cortex", "portableRelativeRoot": "Cortex"}]}), encoding="utf-8")
            legacy_path.write_text(json.dumps({"schema": surface.ProjectRegistry.SCHEMA,
                "projects": [{"registryId": "independent-id", "projectId": "cortex", "name": "Cortex",
                              "kind": "rust-workspace", "root": str(independent_root)}]}), encoding="utf-8")
            with mock.patch.object(surface.ProjectRegistry, "default_path", return_value=portable_path), \
                 mock.patch.object(surface.ProjectRegistry, "legacy_default_path", return_value=legacy_path), \
                 mock.patch.object(surface, "resolve_portable_volume_path", return_value=Path("E:/Cortex")):
                rows = surface.ProjectRegistry().entries()
            self.assertEqual({row.registry_id for row in rows}, {"portable-id", "independent-id"})

    def test_machine_local_registration_is_not_hidden_by_portable_merge(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            portable_path = td / "portable.json"
            legacy_path = td / "legacy.json"
            portable_path.write_text(json.dumps({
                "schema": surface.ProjectRegistry.SCHEMA,
                "projects": [{
                    "registryId": "portable-id",
                    "projectId": "cortex",
                    "name": "Cortex",
                    "kind": "rust-workspace",
                    "root": "E:/Cortex",
                    "portableRelativeRoot": "Cortex",
                }],
            }), encoding="utf-8")
            legacy_path.write_text(json.dumps({
                "schema": surface.ProjectRegistry.SCHEMA,
                "projects": [{
                    "registryId": "local-id",
                    "projectId": "local-fixture",
                    "name": "Local Fixture",
                    "kind": "project",
                    "root": "C:/Users/Test/LocalFixture",
                }],
            }), encoding="utf-8")
            with mock.patch.object(surface.ProjectRegistry, "default_path", return_value=portable_path), \
                 mock.patch.object(surface.ProjectRegistry, "legacy_default_path", return_value=legacy_path), \
                 mock.patch.object(surface, "resolve_portable_volume_path", return_value=Path("E:/Cortex")):
                rows = surface.ProjectRegistry().entries()
            self.assertEqual({row.registry_id for row in rows}, {"portable-id", "local-id"})


if __name__ == "__main__":
    unittest.main()
