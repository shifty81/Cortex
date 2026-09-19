"""Cortex portable source rollup regression fixtures; no real source is edited."""
import importlib.util
import json
import os
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parents[1] / "CortexSourceRollup.py"
SPEC = importlib.util.spec_from_file_location("CortexSourceRollup", HERE)
rollup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rollup)


class SourceRollupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "Cortex"
        self.root.mkdir()
        (self.root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        (self.root / "project.control.json").write_text('{"project":"Cortex"}\n', encoding="utf-8")
        (self.root / "src").mkdir()
        (self.root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")

    def manifest(self, path):
        with zipfile.ZipFile(path) as zf:
            return json.loads(zf.read(rollup.MANIFEST)), zf.namelist()

    def test_01_preserves_untracked_source_and_root_paths(self):
        (self.root / "NEW_SOURCE.txt").write_text("untracked changes", encoding="utf-8")
        archive = rollup.create(self.root)
        manifest, names = self.manifest(archive)
        self.assertEqual(manifest["git"]["repository"], False)
        self.assertIn("src/main.rs", names)
        self.assertIn("NEW_SOURCE.txt", names)
        self.assertNotIn("Cortex/src/main.rs", names)
        self.assertEqual(rollup.verify(archive)["fileCount"], 4)
        self.assertTrue(Path(str(archive) + ".sha256").exists())

    def test_02_excludes_secrets_and_generated_artifacts(self):
        (self.root / ".env").write_text("PRIVATE=1")
        (self.root / "target").mkdir()
        (self.root / "target" / "debug.bin").write_bytes(b"binary")
        (self.root / "source.key").write_text("PRIVATE")
        archive = rollup.create(self.root)
        manifest, names = self.manifest(archive)
        self.assertNotIn(".env", names)
        self.assertNotIn("source.key", names)
        self.assertNotIn("target/debug.bin", names)
        self.assertGreaterEqual(len(manifest["exclusions"]), 3)

    def test_03_does_not_follow_symlink(self):
        external = Path(self.temp.name) / "private.txt"
        external.write_text("outside")
        try:
            (self.root / "symlink.txt").symlink_to(external)
        except (OSError, NotImplementedError):
            self.skipTest("symlink privilege not available")
        archive = rollup.create(self.root)
        manifest, names = self.manifest(archive)
        self.assertNotIn("symlink.txt", names)
        self.assertTrue(any(x["path"] == "symlink.txt" for x in manifest["exclusions"]))

    def test_04_tamper_detection(self):
        archive = rollup.create(self.root)
        tampered = Path(self.temp.name) / "tampered.zip"
        with zipfile.ZipFile(archive) as src, zipfile.ZipFile(tampered, "w") as dest:
            for name in src.namelist():
                dest.writestr(name, b"different" if name == "src/main.rs" else src.read(name))
        with self.assertRaisesRegex(rollup.RollupError, "digest/size mismatch"):
            rollup.verify(tampered)

    def test_05_fails_closed_if_source_over_limit(self):
        with self.assertRaisesRegex(rollup.RollupError, "exceeds configured safety limits"):
            rollup.create(self.root, max_file=2)
        self.assertFalse((self.root / "artifacts").exists())

    def test_06_receipt_lineage_redacted(self):
        receipts = self.root / "artifacts" / "patches" / "receipts"
        receipts.mkdir(parents=True)
        (receipts / "test.json").write_text(json.dumps({"patchId":"CTX-TEST", "status":"applied",
             "archivePath":"C:/private/location", "credentials":"secret"}))
        archive = rollup.create(self.root)
        manifest, names = self.manifest(archive)
        self.assertNotIn("artifacts/patches/receipts/test.json", names)
        self.assertEqual(manifest["patchLineage"]["appliedReceipts"][0]["patchId"], "CTX-TEST")
        self.assertNotIn("C:/private", json.dumps(manifest))
        self.assertNotIn('"credentials"', json.dumps(manifest))

    def test_07_missing_identity_refused(self):
        (self.root / "Cargo.toml").unlink()
        with self.assertRaisesRegex(rollup.RollupError, "not an identifiable"):
            rollup.create(self.root)

    def test_08_transport_detected_without_recursion(self):
        patch = self.root / "Cortex_RootPatch_Fixture.zip"
        with zipfile.ZipFile(patch, "w") as zf:
            zf.writestr("PATCH_MANIFEST.json", json.dumps({"schema":"cortex.root_patch.v1", "patchId":"CTX-X"}))
        archive = rollup.create(self.root)
        manifest, names = self.manifest(archive)
        self.assertNotIn(patch.name, names)
        self.assertEqual(manifest["patchLineage"]["pendingTransports"][0]["patchId"], "CTX-X")
        self.assertNotIn("artifacts/source-rollups/", "\n".join(names))

    def test_09_source_mutation_refused_without_final_zip(self):
        old = rollup.sha_file
        changed = [False]
        def mutate(path):
            if not changed[0] and path.name == "main.rs":
                changed[0] = True
                path.write_text("changed midway", encoding="utf-8")
            return old(path)
        rollup.sha_file = mutate
        try:
            with self.assertRaisesRegex(rollup.RollupError, "source changed during archive"):
                rollup.create(self.root)
        finally:
            rollup.sha_file = old
        self.assertFalse(list((self.root / "artifacts" / "source-rollups").glob("*.zip")))

    def test_10_rejects_modified_faux_zip_with_extra_entry(self):
        archive = rollup.create(self.root)
        bad = Path(self.temp.name) / "extra.zip"
        with zipfile.ZipFile(archive) as zf, zipfile.ZipFile(bad, "w") as out:
            for name in zf.namelist():
                out.writestr(name, zf.read(name))
            out.writestr("extra.txt", b"surprise")
        with self.assertRaisesRegex(rollup.RollupError, "unexpected ZIP files"):
            rollup.verify(bad)

    def test_11_rejects_linked_operational_ancestor(self):
        outside = Path(self.temp.name) / "outside"
        outside.mkdir()
        try:
            (self.root / "artifacts").symlink_to(outside, target_is_directory=True)
        except (OSError, NotImplementedError):
            self.skipTest("directory symlink privilege unavailable")
        with self.assertRaisesRegex(rollup.RollupError, "patch receipt path|output directory"):
            rollup.create(self.root)
        self.assertFalse(list(outside.glob("*.zip")))

    def test_12_excludes_git_worktree_pointer(self):
        (self.root / ".git").write_text("gitdir: C:/private/path", encoding="utf-8")
        archive = rollup.create(self.root)
        manifest, names = self.manifest(archive)
        self.assertNotIn(".git", names)
        self.assertTrue(any(x["path"] == ".git" for x in manifest["exclusions"]))


if __name__ == "__main__":
    unittest.main()
