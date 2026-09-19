"""Cortex source comparison fixtures; snapshots built in temporary folders."""
import contextlib
import hashlib
import importlib.util
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

CONTROL = Path(__file__).resolve().parents[1]
import sys
sys.path.insert(0, str(CONTROL))
import CortexSourceRollup as rollup
import CortexSourceCompare as compare


class SourceCompareTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        self.left, self.right = base / "a", base / "b"
        for directory in (self.left, self.right):
            directory.mkdir()
            (directory / "Cargo.toml").write_text("[workspace]\n")
            (directory / "project.control.json").write_text('{"schema": 1}\n')
            (directory / "src").mkdir()
            (directory / "src" / "lib.rs").write_text("pub fn original() {}\n")

    def archives(self):
        return rollup.create(self.left), rollup.create(self.right)

    def test_01_identical_content_not_mistaken_for_modified(self):
        l, r = self.archives()
        report = compare.compare(l, r)
        self.assertEqual(report["counts"]["changed"], 0)
        self.assertEqual(report["counts"]["sameContent"], 3)
        self.assertEqual(report["schema"], compare.REPORT_SCHEMA)
        self.assertEqual(report["left"]["sidecar"], "verified")

    def test_02_added_removed_changed_and_sha_evidence(self):
        (self.left / "left.txt").write_text("left")
        (self.right / "right.txt").write_text("right")
        (self.right / "src" / "lib.rs").write_text("pub fn updated() {}\n")
        l, r = self.archives()
        report = compare.compare(l, r)
        self.assertEqual(report["counts"], {"onlyLeft": 1, "onlyRight": 1, "changed": 1,
                                           "sameContent": 2, "leftTotal": 4, "rightTotal": 4})
        changed = report["differences"]["changed"][0]
        self.assertEqual(changed["path"], "src/lib.rs")
        self.assertEqual(changed["leftSha256"], hashlib.sha256(b"pub fn original() {}\n").hexdigest())
        self.assertEqual(changed["rightSha256"], hashlib.sha256(b"pub fn updated() {}\n").hexdigest())
        self.assertEqual(report["differences"]["onlyLeft"][0]["path"], "left.txt")
        self.assertEqual(report["differences"]["onlyRight"][0]["path"], "right.txt")

    def test_03_missing_sidecar_disclosed_not_assumed_verified(self):
        l, r = self.archives()
        Path(str(l) + ".sha256").unlink()
        self.assertEqual(compare.compare(l, r)["left"]["sidecar"], "missing")

    def test_04_modified_sidecar_fails_closed(self):
        l, r = self.archives()
        Path(str(r) + ".sha256").write_text("f" * 64 + "  " + r.name + "\n")
        with self.assertRaisesRegex(compare.CompareError, "sidecar mismatch"):
            compare.compare(l, r)

    def test_05_manifest_tamper_refused_before_reporting_diff(self):
        l, r = self.archives()
        wrong = self.right / "wrong.zip"
        with zipfile.ZipFile(r) as original, zipfile.ZipFile(wrong, "w") as target:
            for name in original.namelist():
                target.writestr(name, b"injected" if name == "src/lib.rs" else original.read(name))
        with self.assertRaisesRegex(rollup.RollupError, "digest/size mismatch"):
            compare.compare(l, wrong)

    def test_06_comparison_never_changes_inputs(self):
        l, r = self.archives()
        before = [(p, hashlib.sha256(p.read_bytes()).hexdigest()) for p in (l, r)]
        compare.compare(l, r)
        self.assertTrue(all(hashlib.sha256(p.read_bytes()).hexdigest() == sha for p, sha in before))

    def test_07_receipts_lineage_disagreement_is_visible(self):
        for root, pid in ((self.left, "CTX-OLD"), (self.right, "CTX-NEW")):
            directory = root / "artifacts" / "patches" / "receipts"
            directory.mkdir(parents=True)
            (directory / "receipt.json").write_text(json.dumps({"patchId": pid, "status": "applied"}))
        l, r = self.archives()
        diff = compare.compare(l, r)["differences"]
        self.assertEqual(diff["onlyLeftAppliedPatchIds"], ["CTX-OLD"])
        self.assertEqual(diff["onlyRightAppliedPatchIds"], ["CTX-NEW"])

    def test_08_preview_is_explicitly_incomplete(self):
        for index in range(5):
            (self.right / f"new{index}.rs").write_text(f"// {index}\n")
        l, r = self.archives()
        report = compare.compare(l, r)
        preview = compare.bounded_report(report, 2)
        self.assertEqual(preview["counts"]["onlyRight"], 5)
        self.assertEqual(len(preview["differences"]["onlyRight"]), 2)
        self.assertFalse(preview["preview"]["complete"])
        self.assertEqual(len(report["differences"]["onlyRight"]), 5)

    def test_09_cli_emits_json_and_success_even_when_different(self):
        (self.right / "new.rs").write_text("new")
        l, r = self.archives()
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            result = compare.main(["--left", str(l), "--right", str(r), "--preview", "0"])
        self.assertEqual(result, 0)
        self.assertEqual(json.loads(buffer.getvalue())["counts"]["onlyRight"], 1)

    def test_10_mismatched_sidecar_name_fails(self):
        l, r = self.archives()
        Path(str(r) + ".sha256").write_text(rollup.sha_file(r) + "  other.zip\n")
        with self.assertRaisesRegex(compare.CompareError, "sidecar mismatch"):
            compare.compare(l, r)

    def test_11_arbitrary_zip_is_not_a_source_archive(self):
        l, r = self.archives()
        bad = self.right / "not-a-rollup.zip"
        with zipfile.ZipFile(bad, "w") as zf:
            zf.writestr("PATCH_MANIFEST.json", "{}")
        with self.assertRaisesRegex(rollup.RollupError, "duplicate or missing manifest"):
            compare.compare(l, bad)

    def test_12_rejects_oversized_preview(self):
        with self.assertRaisesRegex(compare.CompareError, "--preview"):
            compare.bounded_report({"differences": {}}, 101)


if __name__ == "__main__":
    unittest.main()
