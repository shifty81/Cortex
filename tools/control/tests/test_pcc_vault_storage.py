from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

TOOLS = Path(__file__).resolve().parents[1]
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import PCCStoragePaths as paths
import PCCVaultStorage as storage
import PCCSurfaceCommon as surface
import PCCOperationHost as operation_host


class VaultStorageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.base = Path(self.tmp.name)
        self.root = self.base / "project"
        self.vault = self.base / "vault"
        self.root.mkdir()
        self.env = mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": str(self.vault)}, clear=False)
        self.env.start()

    def tearDown(self) -> None:
        self.env.stop()
        self.tmp.cleanup()

    def test_dependency_paths_are_shared_but_rust_target_is_project_namespaced(self) -> None:
        p = paths.dependency_paths(self.root)
        self.assertEqual(p["cargo_home"], self.vault / "shared" / "dependencies" / "rust" / "cargo-home")
        self.assertIn(paths.project_key(self.root), str(p["cargo_target_dir"]))
        self.assertEqual(p["npm_cache"], self.vault / "shared" / "dependencies" / "node" / "npm-cache")
        env = storage.dependency_environment(self.root)
        self.assertEqual(env["CARGO_HOME"], str(p["cargo_home"]))
        self.assertEqual(env["CARGO_TARGET_DIR"], str(p["cargo_target_dir"]))
        self.assertEqual(env["PIP_CACHE_DIR"], str(p["pip_cache"]))

    def test_ensure_layout_creates_shared_and_project_namespaces(self) -> None:
        result = storage.ensure_layout(self.root)
        self.assertTrue(Path(result["cargo_home"]).is_dir())
        self.assertTrue(Path(result["cargo_target_dir"]).is_dir())
        self.assertTrue(paths.project_vault_dir(self.root).is_dir())
        self.assertTrue(paths.object_store_dir(self.root).is_dir())

    def test_project_mirror_is_content_addressed_and_excludes_rebuildable_trees(self) -> None:
        (self.root / "src").mkdir()
        (self.root / "assets").mkdir()
        (self.root / "target" / "debug").mkdir(parents=True)
        (self.root / "node_modules" / "pkg").mkdir(parents=True)
        payload = b"same-content"
        (self.root / "src" / "a.rs").write_bytes(payload)
        (self.root / "assets" / "copy.txt").write_bytes(payload)
        (self.root / "target" / "debug" / "huge.bin").write_bytes(b"build")
        (self.root / "node_modules" / "pkg" / "index.js").write_bytes(b"dependency")

        snap = storage.mirror_project(self.root, label="test")
        mirrored = {item["path"] for item in snap["files"]}
        self.assertIn("src/a.rs", mirrored)
        self.assertIn("assets/copy.txt", mirrored)
        self.assertNotIn("target/debug/huge.bin", mirrored)
        self.assertNotIn("node_modules/pkg/index.js", mirrored)
        self.assertEqual(snap["stats"]["files"], 2)
        # Identical source/content is one global object even though two project
        # paths reference it.
        objects = list(paths.object_store_dir(self.root).rglob("*"))
        object_files = [p for p in objects if p.is_file()]
        self.assertEqual(len(object_files), 1)


    def test_project_mirror_excludes_operational_transport_and_links(self) -> None:
        (self.root / "src").mkdir()
        (self.root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        for rel in (".cortex", ".project_control", "artifacts/logs", "updates", "handoffs"):
            folder = self.root / rel
            folder.mkdir(parents=True, exist_ok=True)
            (folder / "noise.txt").write_text("operational", encoding="utf-8")
        (self.root / "transport.zip").write_bytes(b"transport")
        (self.root / "transport.zip.sha256").write_text("0" * 64, encoding="ascii")
        link_supported = True
        try:
            (self.root / "outside-link.txt").symlink_to(self.base / "outside.txt")
            (self.base / "outside.txt").write_text("outside", encoding="utf-8")
        except (OSError, NotImplementedError):
            link_supported = False

        snap = storage.mirror_project(self.root, label="boundary")
        mirrored = {item["path"] for item in snap["files"]}
        self.assertEqual(mirrored, {"src/main.rs"})
        classes = {item["classification"] for item in snap["excludedDirectories"]}
        self.assertIn("OPERATIONAL", classes)
        self.assertIn("TRANSPORT", classes)
        if link_supported:
            self.assertIn("LINK", classes)

    def test_existing_same_size_corrupt_cas_object_is_repaired(self) -> None:
        source = self.root / "state.bin"
        source.write_bytes(b"abcd")
        digest = storage._sha256(source)
        obj = paths.object_store_dir(self.root) / digest[:2] / digest[2:4] / digest
        obj.parent.mkdir(parents=True, exist_ok=True)
        obj.write_bytes(b"wxyz")
        snap = storage.mirror_project(self.root, label="repair-cas")
        self.assertEqual(storage._sha256(obj), digest)
        self.assertEqual(snap["stats"]["newObjects"], 1)
        self.assertEqual(storage.verify_latest_mirror(self.root, deep_hash=True)["status"], "PASS")

    def test_certification_rejects_green_fingerprint_mismatch(self) -> None:
        (self.root / "README.md").write_text("green\n", encoding="utf-8")
        snap = storage.mirror_project(
            self.root,
            label="full-green-candidate",
            source_fingerprint="a" * 64,
            source_path_count=1,
        )
        marker = self.root / ".cortex" / "last-green-quality-gate.json"
        marker.parent.mkdir(parents=True)
        marker.write_text(json.dumps({"schema":"cortex.green_quality_gate.v1","fingerprint":"b" * 64}), encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "does not match GREEN marker"):
            storage.certify_snapshot(self.root, snap["snapshotId"], gate="full", source_marker="GREEN")

    def test_second_snapshot_reuses_unchanged_fingerprints_and_objects(self) -> None:
        (self.root / "src").mkdir()
        (self.root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        first = storage.mirror_project(self.root, label="one")
        second = storage.mirror_project(self.root, label="two")
        self.assertEqual(first["stats"]["files"], 1)
        self.assertGreaterEqual(second["stats"]["reusedFingerprints"], 1)
        self.assertEqual(second["stats"]["newObjects"], 0)
        self.assertGreaterEqual(second["stats"]["reusedObjects"], 1)

    def test_verify_detects_missing_object(self) -> None:
        (self.root / "src").mkdir()
        (self.root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        snap = storage.mirror_project(self.root, label="verify")
        self.assertEqual(storage.verify_latest_mirror(self.root)["status"], "PASS")
        digest = snap["files"][0]["sha256"]
        obj = paths.object_store_dir(self.root) / digest[:2] / digest[2:4] / digest
        obj.unlink()
        result = storage.verify_latest_mirror(self.root)
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(result["missing"], ["src/main.rs"])

    def test_project_metadata_references_shared_dependency_store(self) -> None:
        (self.root / "README.md").write_text("project\n", encoding="utf-8")
        storage.mirror_project(self.root, label="metadata")
        meta = json.loads((paths.project_vault_dir(self.root) / "project.json").read_text(encoding="utf-8"))
        self.assertEqual(meta["projectKey"], paths.project_key(self.root))
        self.assertIn("cargo_home", meta["dependencyStore"])
        self.assertTrue((paths.project_vault_dir(self.root) / "snapshots" / "latest.json").is_file())

    def test_reclaim_plan_is_read_only_and_sizes_local_build_cache(self) -> None:
        (self.root / "target" / "debug").mkdir(parents=True)
        payload = b"x" * 2048
        local = self.root / "target" / "debug" / "artifact.bin"
        local.write_bytes(payload)
        plan = storage.reclaim_plan(self.root)
        self.assertFalse(plan["deleteApplied"])
        self.assertGreaterEqual(plan["reclaimBytes"], len(payload))
        self.assertTrue(local.is_file())
        self.assertIn("target", {item["relativePath"] for item in plan["candidates"]})


    def test_full_green_snapshot_certification_is_separate_metadata(self) -> None:
        (self.root / "README.md").write_text("green\n", encoding="utf-8")
        snap = storage.mirror_project(self.root, label="full-green-candidate")
        cert = storage.certify_snapshot(self.root, snap["snapshotId"], gate="full", source_marker="GREEN")
        self.assertEqual(cert["snapshotId"], snap["snapshotId"])
        cert_dir = paths.project_vault_dir(self.root) / "certifications"
        self.assertTrue((cert_dir / f"full-{snap['snapshotId']}.json").is_file())
        self.assertTrue((cert_dir / "latest-full.json").is_file())

    def test_retention_prunes_only_old_snapshot_metadata(self) -> None:
        src = self.root / "state.txt"
        src.write_text("one", encoding="utf-8")
        first = storage.mirror_project(self.root, label="working")
        src.write_text("two", encoding="utf-8")
        second = storage.mirror_project(self.root, label="working")
        src.write_text("three", encoding="utf-8")
        third = storage.mirror_project(self.root, label="working")
        with mock.patch.object(storage, "storage_policy", return_value={"vault": {"snapshot_retention": {"certified_keep": 1, "working_keep": 1}}}):
            plan = storage.snapshot_retention_plan(self.root)
            self.assertEqual(plan["pruneCount"], 2)
            result = storage.apply_snapshot_retention(self.root)
        self.assertTrue(result["applied"])
        self.assertTrue((paths.project_vault_dir(self.root) / "snapshots" / f"snapshot-{third['snapshotId']}.json").is_file())
        self.assertFalse((paths.project_vault_dir(self.root) / "snapshots" / f"snapshot-{first['snapshotId']}.json").exists())
        self.assertFalse((paths.project_vault_dir(self.root) / "snapshots" / f"snapshot-{second['snapshotId']}.json").exists())
        # Retention changes metadata only. CAS objects remain until governed GC.
        self.assertGreaterEqual(len([p for p in paths.object_store_dir(self.root).rglob("*") if p.is_file()]), 3)

    def test_cas_gc_stages_restores_and_purges_only_unreferenced_objects(self) -> None:
        src = self.root / "state.txt"
        src.write_text("old", encoding="utf-8")
        first = storage.mirror_project(self.root, label="working")
        old_digest = first["files"][0]["sha256"]
        src.write_text("new", encoding="utf-8")
        storage.mirror_project(self.root, label="working")
        with mock.patch.object(storage, "storage_policy", return_value={"vault": {"snapshot_retention": {"certified_keep": 1, "working_keep": 1}}}):
            storage.apply_snapshot_retention(self.root)
        plan = storage.cas_gc_plan(self.root)
        garbage = {row["sha256"] for row in plan["garbage"]}
        self.assertIn(old_digest, garbage)
        old_obj = paths.object_store_dir(self.root) / old_digest[:2] / old_digest[2:4] / old_digest
        staged = storage.stage_cas_gc(self.root)
        self.assertFalse(old_obj.exists())
        self.assertEqual(staged["transaction"]["status"], "STAGED")
        restored = storage.restore_cas_gc(self.root)
        self.assertEqual(restored["status"], "RESTORED")
        self.assertTrue(old_obj.is_file())
        staged_again = storage.stage_cas_gc(self.root)
        self.assertFalse(old_obj.exists())
        purged = storage.purge_cas_gc(self.root)
        self.assertEqual(purged["status"], "PURGED")
        self.assertGreaterEqual(purged["purgedObjects"], 1)
        self.assertFalse(Path(staged_again["transaction"]["objects"][0]["quarantinePath"]).exists())

    def test_storage_health_reports_cas_shared_and_reclaim_accounting(self) -> None:
        (self.root / "README.md").write_text("health\n", encoding="utf-8")
        (self.root / "target" / "debug").mkdir(parents=True)
        (self.root / "target" / "debug" / "x.bin").write_bytes(b"x" * 64)
        storage.mirror_project(self.root, label="health")
        health = storage.storage_health(self.root)
        self.assertIn(health["status"], {"PASS", "WARN"})
        self.assertEqual(health["projects"], 1)
        self.assertGreaterEqual(health["snapshots"], 1)
        self.assertGreaterEqual(health["cas"]["files"], 1)
        self.assertGreaterEqual(health["currentProjectReclaimBytes"], 64)
        self.assertIn("cargo_home", health["sharedStores"])


    def test_universal_backend_inherits_shared_dependency_environment(self) -> None:
        client = object.__new__(surface.BackendClient)
        client.root = self.root
        env = client._embedded_env()
        expected = paths.dependency_paths(self.root)
        self.assertEqual(env["CARGO_HOME"], str(expected["cargo_home"]))
        self.assertEqual(env["CARGO_TARGET_DIR"], str(expected["cargo_target_dir"]))
        self.assertEqual(env["npm_config_cache"], str(expected["npm_cache"]))
        self.assertEqual(env["PIP_CACHE_DIR"], str(expected["pip_cache"]))


    def test_compatible_projects_share_dependency_caches_but_keep_build_outputs_isolated(self) -> None:
        other = self.base / "other-project"
        other.mkdir()
        lock = 'version = 4\n[[package]]\nname = "serde"\nversion = "1.0.229"\n'
        (self.root / "Cargo.lock").write_text(lock, encoding="utf-8")
        (other / "Cargo.lock").write_text(lock, encoding="utf-8")
        left = paths.dependency_paths(self.root)
        right = paths.dependency_paths(other)
        self.assertEqual(left["cargo_home"], right["cargo_home"])
        self.assertEqual(left["sccache_dir"], right["sccache_dir"])
        self.assertEqual(left["npm_cache"], right["npm_cache"])
        self.assertNotEqual(left["cargo_target_dir"], right["cargo_target_dir"])
        self.assertEqual(paths.dependency_compatibility(self.root)["fingerprint"], paths.dependency_compatibility(other)["fingerprint"])

    def test_dependency_version_change_changes_compatibility_identity_without_splitting_global_cache(self) -> None:
        other = self.base / "other-project"
        other.mkdir()
        (self.root / "Cargo.lock").write_text('serde 1.0.228\n', encoding="utf-8")
        (other / "Cargo.lock").write_text('serde 1.0.229\n', encoding="utf-8")
        left = paths.dependency_paths(self.root)
        right = paths.dependency_paths(other)
        self.assertEqual(left["cargo_home"], right["cargo_home"])
        self.assertNotEqual(paths.dependency_compatibility(self.root)["fingerprint"], paths.dependency_compatibility(other)["fingerprint"])
        status = storage.dependency_status(self.root)
        self.assertEqual(status["reusePolicy"]["policy"], "share-compatible-isolate-conflicts")
        self.assertIn("rust", status["ecosystems"])

    def test_dependency_environment_exposes_reuse_policy_and_compatibility_fingerprint(self) -> None:
        (self.root / "Cargo.lock").write_text('fixture-lock\n', encoding="utf-8")
        env = storage.dependency_environment(self.root)
        self.assertEqual(env["CORTEX_DEPENDENCY_REUSE_POLICY"], "share-compatible-isolate-conflicts")
        self.assertEqual(env["CORTEX_DEPENDENCY_COMPATIBILITY"], paths.dependency_compatibility(self.root)["fingerprint"])


    def test_shared_toolchains_are_prepended_for_native_project_children(self) -> None:
        python_dir = self.vault / "shared" / "toolchains" / "python" / "3.14.7"
        git_dir = self.vault / "shared" / "toolchains" / "git" / "mingit" / "cmd"
        cargo_dir = self.vault / "shared" / "dependencies" / "rust" / "cargo-home" / "bin"
        python_dir.mkdir(parents=True)
        git_dir.mkdir(parents=True)
        cargo_dir.mkdir(parents=True)
        (python_dir / "python.exe").write_bytes(b"")
        (git_dir / "git.exe").write_bytes(b"")
        # On non-Windows test hosts the executable suffix follows os.name, while
        # the production portable runtime is Windows.
        cargo_name = "cargo.exe" if os.name == "nt" else "cargo"
        rustc_name = "rustc.exe" if os.name == "nt" else "rustc"
        (cargo_dir / cargo_name).write_bytes(b"")
        (cargo_dir / rustc_name).write_bytes(b"")

        env = storage.dependency_environment(self.root)
        path_parts = env["PATH"].split(os.pathsep)
        self.assertEqual(env["CORTEX_PYTHON_EXE"], str(python_dir / "python.exe"))
        self.assertEqual(path_parts[0], str(python_dir))
        self.assertIn(str(git_dir), path_parts)
        self.assertIn(str(cargo_dir), path_parts)
        self.assertEqual(env["PCC_VAULT_ROOT"], str(self.vault))
        self.assertEqual(env["RUSTUP_HOME"], str(self.vault / "shared" / "toolchains" / "rust" / "rustup-home"))



    def test_machine_bootstrap_vault_authority_survives_project_switch_without_process_override(self) -> None:
        runtime = self.base / "cortex-runtime"
        runtime.mkdir()
        bootstrap = runtime / ".cortex" / "bootstrap-env.cmd"
        bootstrap.parent.mkdir(parents=True)
        machine_vault = self.base / "machine-vault"
        python_dir = machine_vault / "shared" / "toolchains" / "python" / "3.14.7"
        python_dir.mkdir(parents=True)
        (python_dir / "python.exe").write_bytes(b"")
        bootstrap.write_text(
            '@echo off\nset "CORTEX_VAULT_ROOT=' + str(machine_vault) + '"\n',
            encoding="utf-8",
        )
        other = self.base / "havenwild"
        other.mkdir()
        with mock.patch.dict(
            os.environ,
            {
                "CORTEX_VAULT_ROOT": "",
                "PCC_VAULT_ROOT": "",
                "CORTEX_RUNTIME_ROOT": str(runtime),
            },
            clear=False,
        ):
            self.assertEqual(paths.resolve_vault_root(other), machine_vault.resolve())
            env = storage.dependency_environment(other)
        self.assertEqual(env["CORTEX_VAULT_ROOT"], str(machine_vault.resolve()))
        self.assertEqual(env["CORTEX_PYTHON_EXE"], str(python_dir / "python.exe"))
        self.assertEqual(env["PATH"].split(os.pathsep)[0], str(python_dir))

    def test_operation_host_rehydrates_shared_toolchain_environment_for_native_pcc(self) -> None:
        python_dir = self.vault / "shared" / "toolchains" / "python" / "3.14.7"
        python_dir.mkdir(parents=True)
        (python_dir / "python.exe").write_bytes(b"")
        with mock.patch.object(operation_host.subprocess, "Popen") as popen:
            proc = popen.return_value
            proc.wait.return_value = 0
            rc = operation_host._run(["PCC.cmd", "full"], self.root)
        self.assertEqual(rc, 0)
        env = popen.call_args.kwargs["env"]
        self.assertEqual(env["CORTEX_PYTHON_EXE"], str(python_dir / "python.exe"))
        self.assertEqual(env["CORTEX_VAULT_ROOT"], str(self.vault))
        self.assertEqual(env["PCC_VAULT_ROOT"], str(self.vault))
        self.assertEqual(env["PATH"].split(os.pathsep)[0], str(python_dir))
        self.assertEqual(env["PCC_OPERATION_HOST_ACTIVE"], "1")

    def test_backend_can_bind_cortex_runtime_environment_separately_from_active_project(self) -> None:
        cortex_root = self.base / "cortex-runtime"
        cortex_root.mkdir()
        client = object.__new__(surface.BackendClient)
        client.root = self.root
        active_env = client._embedded_env()
        cortex_env = client._embedded_env(cortex_root)
        self.assertNotEqual(active_env["CARGO_TARGET_DIR"], cortex_env["CARGO_TARGET_DIR"])
        self.assertIn(paths.project_key(self.root), active_env["CARGO_TARGET_DIR"])
        self.assertIn(paths.project_key(cortex_root), cortex_env["CARGO_TARGET_DIR"])



    def test_project_key_is_stable_for_same_vault_relative_path_after_drive_root_change(self) -> None:
        first_vault = self.base / "portable-a"
        second_vault = self.base / "portable-b"
        first = first_vault / "Source" / "Example"
        second = second_vault / "Source" / "Example"
        first.mkdir(parents=True)
        second.mkdir(parents=True)
        with mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": str(first_vault)}, clear=False):
            first_key = paths.project_key(first)
        with mock.patch.dict(os.environ, {"CORTEX_VAULT_ROOT": str(second_vault)}, clear=False):
            second_key = paths.project_key(second)
        self.assertEqual(first_key, second_key)

    def test_dependency_environment_keeps_portable_state_models_and_projects_on_vault(self) -> None:
        env = storage.dependency_environment(self.root)
        self.assertEqual(env["CORTEX_HOME"], str(self.vault / ".cortex" / "home"))
        self.assertEqual(env["CORTEX_PROJECTS_ROOT"], str(self.vault / "Source"))
        self.assertEqual(env["CORTEX_MODELS_ROOT"], str(self.vault / "Models"))
        self.assertEqual(env["CORTEX_STATE_MODE"], "portable")

    def test_legacy_absolute_project_namespaces_migrate_without_duplication(self) -> None:
        inside = self.vault / "Source" / "PortableProject"
        inside.mkdir(parents=True)
        old_key = paths.legacy_project_key(inside)
        new_key = paths.project_key(inside)
        self.assertNotEqual(old_key, new_key)
        old_project = self.vault / "projects" / old_key
        old_target = self.vault / "shared" / "build" / "rust" / "targets" / old_key
        old_project.mkdir(parents=True)
        old_target.mkdir(parents=True)
        (old_project / "sentinel.txt").write_text("project", encoding="utf-8")
        (old_target / "sentinel.txt").write_text("target", encoding="utf-8")
        storage.ensure_layout(inside)
        self.assertFalse(old_project.exists())
        self.assertFalse(old_target.exists())
        self.assertEqual((self.vault / "projects" / new_key / "sentinel.txt").read_text(), "project")
        self.assertEqual((self.vault / "shared" / "build" / "rust" / "targets" / new_key / "sentinel.txt").read_text(), "target")


if __name__ == "__main__":
    unittest.main(verbosity=2)
