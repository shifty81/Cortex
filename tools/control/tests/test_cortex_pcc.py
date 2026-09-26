from __future__ import annotations

import hashlib
import importlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import zipfile
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import CortexPCC as pcc
import CortexPatchAuthority as patch
import CortexGitAuthority as git
import CortexPCCMaintenance as maintenance
import PCCProjectDiscovery as discovery
from PCCSurfaceCommon import BackendClient, ProjectContract, command_catalog


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def init_git(root: Path) -> None:
    subprocess.run(["git", "init", "-b", "main"], cwd=root, check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["git", "config", "user.email", "pcc-tests@example.invalid"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.name", "PCC Tests"], cwd=root, check=True)


def commit_all(root: Path, msg: str = "baseline") -> None:
    subprocess.run(["git", "add", "-A"], cwd=root, check=True)
    subprocess.run(["git", "commit", "-m", msg], cwd=root, check=True, stdout=subprocess.DEVNULL)


def make_patch(root: Path, patch_id: str, files: dict[str, bytes], *, remove=None, depends=None,
               series="T", sequence="1", sidecar=True, extra_files=None, overrides=None) -> Path:
    remove = remove or []
    depends = depends or []
    specs = []
    overrides = overrides or {}
    for rel, data in files.items():
        item = {"path": rel, "sha256": sha(data), "bytes": len(data)}
        item.update(overrides.get(rel, {}))
        specs.append(item)
    manifest = {
        "schema": patch.SCHEMA,
        "project": patch.PROJECT,
        "patchId": patch_id,
        "title": f"Test {patch_id}",
        "series": series,
        "sequence": sequence,
        "applyMode": "transactional",
        "dependsOn": depends,
        "files": specs,
        "remove": remove,
    }
    zp = root / f"{patch_id}_RootPatch.zip"
    with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("PATCH_MANIFEST.json", json.dumps(manifest, indent=2))
        for rel, data in files.items():
            zf.writestr(rel, data)
        for rel, data in (extra_files or {}).items():
            zf.writestr(rel, data)
    if sidecar:
        Path(str(zp) + ".sha256").write_text(f"{patch.sha256_file(zp)}  {zp.name}\n", encoding="utf-8")
    return zp


class TempRoot(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()


class PCC60PassTests(TempRoot):
    # 01
    def test_01_normalize_root_explicit(self):
        self.assertEqual(pcc.normalize_root(self.root), self.root.resolve())

    # 02
    def test_02_parse_authority_result_json(self):
        text = "noise\nPCC_RESULT_JSON={\"Pending\":2}\n"
        self.assertEqual(pcc.JsonAuthorityBridge.parse_result_json(text)["Pending"], 2)

    # 03
    def test_03_session_log_writes_text_and_jsonl(self):
        log = pcc.SessionLog(self.root, quiet=True)
        log.emit("PASS", "hello", phase="test")
        self.assertIn("hello", log.text_path.read_text())
        row = json.loads(log.jsonl_path.read_text().splitlines()[-1])
        self.assertEqual(row["phase"], "test")

    # 04
    def test_04_command_runner_success(self):
        log = pcc.SessionLog(self.root, quiet=True)
        result = pcc.CommandRunner(log).run([sys.executable, "-c", "print('ok')"], cwd=self.root, stream=False)
        self.assertTrue(result.ok)
        self.assertIn("ok", result.stdout)

    # 05
    def test_05_command_runner_failure(self):
        log = pcc.SessionLog(self.root, quiet=True)
        result = pcc.CommandRunner(log).run(
            [sys.executable, "-c", "import sys; print('fmt-like-diff'); print('boom', file=sys.stderr); raise SystemExit(7)"],
            cwd=self.root,
            stream=False,
            phase="test:failure",
        )
        self.assertEqual(result.returncode, 7)
        captures = list(log.command_output_dir.glob("*.txt"))
        self.assertEqual(len(captures), 1)
        captured = captures[0].read_text(encoding="utf-8")
        self.assertIn("fmt-like-diff", captured)
        self.assertIn("boom", captured)

    # 06
    def test_06_command_runner_timeout(self):
        log = pcc.SessionLog(self.root, quiet=True)
        result = pcc.CommandRunner(log).run([sys.executable, "-c", "import time; time.sleep(2)"], cwd=self.root, timeout=0.1, stream=False)
        self.assertTrue(result.timed_out)
        self.assertEqual(result.returncode, 124)

    # 07
    def test_07_project_context_normalized_paths(self):
        ctx = pcc.ProjectContext.create(self.root)
        self.assertEqual(ctx.patch_receipts, self.root / "artifacts" / "patches" / "receipts")

    # 08
    def test_08_binary_path_platform_suffix(self):
        target = self.root / "target"
        name = "cortex.exe" if os.name == "nt" else "cortex"
        exe = target / "release" / name
        exe.parent.mkdir(parents=True)
        exe.write_bytes(b"x")
        self.assertEqual(pcc.CortexPCC.binary_path("cortex", target), exe)

    # 09
    def test_09_python_ast_parse_entrypoint(self):
        ast_text = (TOOLS / "CortexPCC.py").read_text(encoding="utf-8")
        compile(ast_text, "CortexPCC.py", "exec")

    # 10
    def test_10_patch_normalize_relative_path(self):
        self.assertEqual(patch.normalize_rel("tools\\control\\x.py"), "tools/control/x.py")

    # 11
    def test_11_patch_rejects_traversal(self):
        with self.assertRaises(patch.PatchError):
            patch.normalize_rel("../evil.txt")

    # 12
    def test_12_patch_rejects_operational_destination(self):
        with self.assertRaises(patch.PatchError):
            patch.normalize_rel("artifacts/evil.txt")

    # 13
    def test_13_patch_valid_manifest_and_sidecar(self):
        zp = make_patch(self.root, "TEST-013", {"src/a.txt": b"a"})
        value = patch.validate_patch(zp)
        self.assertEqual(value.patch_id, "TEST-013")

    # 14
    def test_14_patch_requires_sidecar(self):
        zp = make_patch(self.root, "TEST-014", {"src/a.txt": b"a"}, sidecar=False)
        with self.assertRaises(patch.PatchError):
            patch.validate_patch(zp)

    # 15
    def test_15_patch_rejects_bad_sidecar(self):
        zp = make_patch(self.root, "TEST-015", {"src/a.txt": b"a"})
        Path(str(zp) + ".sha256").write_text("0" * 64 + "  x.zip\n")
        with self.assertRaises(patch.PatchError):
            patch.validate_patch(zp)

    # 16
    def test_16_patch_rejects_undeclared_extra(self):
        zp = make_patch(self.root, "TEST-016", {"src/a.txt": b"a"}, extra_files={"extra.txt": b"x"})
        with self.assertRaises(patch.PatchError):
            patch.validate_patch(zp)

    # 17
    def test_17_patch_rejects_payload_hash_mismatch(self):
        zp = make_patch(self.root, "TEST-017", {"src/a.txt": b"a"})
        with zipfile.ZipFile(zp, "r") as zf:
            manifest = json.loads(zf.read("PATCH_MANIFEST.json"))
        manifest["files"][0]["sha256"] = "0" * 64
        with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.writestr("PATCH_MANIFEST.json", json.dumps(manifest))
            zf.writestr("src/a.txt", b"a")
        Path(str(zp) + ".sha256").write_text(patch.sha256_file(zp) + "  x\n")
        with self.assertRaises(patch.PatchError):
            patch.validate_patch(zp)

    # 18
    def test_18_patch_rejects_case_duplicate_zip_entries(self):
        zp = self.root / "TEST-018_RootPatch.zip"
        manifest = {"schema": patch.SCHEMA, "project": patch.PROJECT, "patchId": "TEST-018", "title": "x", "series": "T", "sequence": "1", "files": [{"path":"A.txt","sha256":sha(b'a'),"bytes":1}]}
        with zipfile.ZipFile(zp, "w") as zf:
            zf.writestr("PATCH_MANIFEST.json", json.dumps(manifest)); zf.writestr("A.txt", b"a"); zf.writestr("a.TXT", b"a")
        Path(str(zp)+".sha256").write_text(patch.sha256_file(zp)+"  x\n")
        with self.assertRaises(patch.PatchError): patch.validate_patch(zp)

    # 19
    def test_19_patch_rejects_invalid_patch_id(self):
        zp = make_patch(self.root, "TEST-019", {"x.txt": b"x"})
        with zipfile.ZipFile(zp, "r") as zf: m=json.loads(zf.read("PATCH_MANIFEST.json"))
        m["patchId"] = "bad/id"
        with zipfile.ZipFile(zp, "w") as zf: zf.writestr("PATCH_MANIFEST.json", json.dumps(m)); zf.writestr("x.txt", b"x")
        Path(str(zp)+".sha256").write_text(patch.sha256_file(zp)+"  x\n")
        with self.assertRaises(patch.PatchError): patch.validate_patch(zp)

    # 20
    def test_20_patch_rejects_self_dependency(self):
        zp = make_patch(self.root, "TEST-020", {"x.txt": b"x"}, depends=["TEST-020"])
        with self.assertRaises(patch.PatchError): patch.validate_patch(zp)

    # 21
    def test_21_patch_scan_marks_missing_dependency_invalid(self):
        make_patch(self.root, "TEST-021", {"x.txt": b"x"}, depends=["TEST-999"])
        result = patch.scan(self.root)
        self.assertEqual(result["Invalid"], 1)

    # 22
    def test_22_patch_topological_dependency_order(self):
        make_patch(self.root, "TEST-022B", {"b.txt": b"b"}, depends=["TEST-022A"], sequence="1")
        make_patch(self.root, "TEST-022A", {"a.txt": b"a"}, sequence="9")
        result = patch.scan(self.root)
        self.assertEqual([x.patch_id for x in result["_validations"]], ["TEST-022A", "TEST-022B"])

    # 23
    def test_23_patch_dependency_cycle_fails_closed(self):
        make_patch(self.root, "TEST-023A", {"a.txt": b"a"}, depends=["TEST-023B"])
        make_patch(self.root, "TEST-023B", {"b.txt": b"b"}, depends=["TEST-023A"])
        result = patch.scan(self.root)
        self.assertGreaterEqual(result["Invalid"], 2)
        self.assertEqual(result["Pending"], 0)

    # 24
    def test_24_patch_apply_writes_file_and_receipt(self):
        make_patch(self.root, "TEST-024", {"src/new.txt": b"hello"})
        self.assertEqual(patch.do_apply(self.root), 0)
        self.assertEqual((self.root / "src/new.txt").read_bytes(), b"hello")
        self.assertTrue((self.root / "artifacts/patches/receipts/TEST-024.json").is_file())

    # 25
    def test_25_patch_apply_removes_file(self):
        target = self.root / "old.txt"; target.write_text("old")
        make_patch(self.root, "TEST-025", {"new.txt": b"new"}, remove=["old.txt"])
        self.assertEqual(patch.do_apply(self.root), 0)
        self.assertFalse(target.exists())

    # 26
    def test_26_patch_preimage_mismatch_preserves_source(self):
        target = self.root / "a.txt"; target.write_text("old")
        make_patch(self.root, "TEST-026", {"a.txt": b"new"}, overrides={"a.txt":{"beforeSha256":"0"*64,"mustExist":True}})
        self.assertNotEqual(patch.do_apply(self.root), 0)
        self.assertEqual(target.read_text(), "old")

    # 27
    def test_26b_patch_preflight_reports_all_conflicts(self):
        (self.root / "a.txt").write_text("actual-a")
        (self.root / "b.txt").write_text("actual-b")
        zp = make_patch(
            self.root,
            "TEST-026B",
            {"a.txt": b"new-a", "b.txt": b"new-b"},
            overrides={
                "a.txt": {"beforeSha256": "0" * 64, "mustExist": True},
                "b.txt": {"beforeSha256": "1" * 64, "mustExist": True},
            },
        )
        validation = patch.validate_patch(zp)
        with self.assertRaises(patch.PatchError) as caught:
            patch.check_preconditions(self.root, validation)
        message = str(caught.exception)
        self.assertIn("2 preimage conflict(s)", message)
        self.assertIn("a.txt", message)
        self.assertIn("b.txt", message)

    def test_26c_preflight_failure_preserves_pending_transport(self):
        target = self.root / "a.txt"
        target.write_text("actual")
        zp = make_patch(
            self.root,
            "TEST-026C",
            {"a.txt": b"new"},
            overrides={"a.txt": {"beforeSha256": "0" * 64, "mustExist": True}},
        )
        sidecar = Path(str(zp) + ".sha256")
        self.assertNotEqual(patch.do_apply(self.root), 0)
        self.assertEqual(target.read_text(), "actual")
        self.assertTrue(zp.is_file())
        self.assertTrue(sidecar.is_file())
        self.assertFalse((self.root / "artifacts" / "patches" / "failed").exists())

    def test_27_patch_replay_protection(self):
        make_patch(self.root, "TEST-027", {"a.txt": b"a"})
        self.assertEqual(patch.do_apply(self.root), 0)
        # Recreate the same patch ID after the receipt exists.
        make_patch(self.root, "TEST-027", {"a.txt": b"a"})
        result = patch.scan(self.root)
        self.assertEqual(result["Invalid"], 1)

    # 28
    def test_28_patch_control_file_requires_restart(self):
        for index, rel in enumerate((
            "tools/control/CortexPCC.py",
            "tools/control/PCCProjectDiscovery.py",
            "tools/control/UniversalPCCAudit.py",
            "project.control.json",
        )):
            zp = make_patch(self.root, f"TEST-028-{index}", {rel: b"x\n"})
            self.assertTrue(patch.validate_patch(zp).restart_required, rel)
            zp.unlink(missing_ok=True)
            zp.with_suffix(zp.suffix + ".sha256").unlink(missing_ok=True)

    # 29
    def test_29_non_patch_zip_is_ignored(self):
        zp = self.root / "Cortex_DebugBundle_test.zip"
        with zipfile.ZipFile(zp, "w") as zf: zf.writestr("x.txt", "x")
        result = patch.scan(self.root)
        self.assertEqual(result["Ignored"], 1)

    # 30
    def test_30_stale_patch_lock_is_reclaimed(self):
        lock = self.root / ".project_control/patch-intake.lock"; lock.parent.mkdir(parents=True)
        lock.write_text(json.dumps({"pid": 99999999, "createdUtc": "old"}))
        with patch.ApplyLock(self.root):
            self.assertTrue(lock.exists())
        self.assertFalse(lock.exists())

    # 31
    def test_31_git_snapshot_excludes_operational_and_zip(self):
        (self.root / "src.txt").write_text("src")
        (self.root / "transport.zip").write_bytes(b"z")
        (self.root / "artifacts").mkdir(); (self.root / "artifacts/a.txt").write_text("a")
        snap = git.snapshot(self.root)
        self.assertEqual(snap["pathCount"], 1)

    # 32
    def test_32_git_mark_green_matches_current_source(self):
        (self.root / "src.txt").write_text("src")
        git.mark_green(self.root)
        ok, _, _ = git.certify_matches(self.root)
        self.assertTrue(ok)

    # 33
    def test_33_git_green_becomes_stale_after_change(self):
        target = self.root / "src.txt"; target.write_text("src")
        git.mark_green(self.root); target.write_text("changed")
        ok, _, _ = git.certify_matches(self.root)
        self.assertFalse(ok)

    # 34
    def test_34_git_stage_governed_excludes_zip_transport(self):
        init_git(self.root); (self.root / "src.txt").write_text("one"); commit_all(self.root)
        (self.root / "src.txt").write_text("two"); (self.root / "patch.zip").write_bytes(b"zip")
        git.stage_governed(self.root)
        names = subprocess.check_output(["git","diff","--cached","--name-only"], cwd=self.root, text=True).splitlines()
        self.assertIn("src.txt", names); self.assertNotIn("patch.zip", names)

    # 35
    def test_35_git_stage_governed_stages_deletion(self):
        init_git(self.root); f=self.root/"src.txt"; f.write_text("one"); commit_all(self.root); f.unlink()
        git.stage_governed(self.root)
        names = subprocess.check_output(["git","diff","--cached","--name-only"], cwd=self.root, text=True).splitlines()
        self.assertIn("src.txt", names)

    # 36
    def test_36_git_green_commit_rejects_non_main_branch(self):
        init_git(self.root); (self.root/"src.txt").write_text("one"); commit_all(self.root); git.mark_green(self.root)
        subprocess.run(["git","checkout","-b","dev"], cwd=self.root, check=True, stdout=subprocess.DEVNULL)
        with self.assertRaises(git.GitError): git.commit_green(self.root, "x")

    # 37
    def test_37_git_status_summary_machine_fields(self):
        init_git(self.root); (self.root/"src.txt").write_text("one"); commit_all(self.root)
        summary = git.status_summary(self.root)
        self.assertTrue(summary["gitReady"]); self.assertEqual(summary["branch"], "main")

    # 38
    def test_38_git_manual_commit_commits_governed_source_only(self):
        init_git(self.root); (self.root/"src.txt").write_text("one"); commit_all(self.root)
        (self.root/"src.txt").write_text("two"); (self.root/"noise.zip").write_bytes(b"x")
        self.assertEqual(git.manual_commit(self.root, "manual"), 0)
        tracked = subprocess.check_output(["git","ls-files"], cwd=self.root, text=True).splitlines()
        self.assertIn("src.txt", tracked); self.assertNotIn("noise.zip", tracked)

    # 39
    def test_39_git_atomic_green_marker_is_valid_json(self):
        (self.root/"src.txt").write_text("one"); git.mark_green(self.root)
        data = json.loads((self.root/".cortex/last-green-quality-gate.json").read_text())
        self.assertEqual(data["schema"], "cortex.green_quality_gate.v1")

    # 40
    def test_40_pcc_argument_parser_exposes_self_test(self):
        args = pcc.build_parser().parse_args(["self-test", "--root", str(self.root)])
        self.assertEqual(args.command, "self-test")

    # 41
    def test_41_session_logs_live_under_artifacts(self):
        log = pcc.SessionLog(self.root, quiet=True)
        self.assertEqual(log.logs_dir, self.root / "artifacts" / "logs" / "sessions")

    # 42
    def test_42_hygiene_detects_root_latest_pointer(self):
        (self.root / "LATEST_DEBUG_BUNDLE.txt").write_text("x")
        report = maintenance.scan_root_hygiene(self.root)
        self.assertFalse(report["clean"]); self.assertEqual(report["violationCount"], 1)

    # 43
    def test_43_hygiene_detects_root_debug_zip(self):
        (self.root / "Cortex_DebugBundle_20260909_FULL_FAIL.zip").write_bytes(b"x")
        report = maintenance.scan_root_hygiene(self.root)
        self.assertEqual(report["violationCount"], 1)

    # 44
    def test_44_hygiene_repair_archives_pointer(self):
        src = self.root / "LATEST_DEBUG_BUNDLE.txt"; src.write_text("legacy")
        result = maintenance.repair_root_hygiene(self.root)
        self.assertFalse(src.exists()); self.assertEqual(len(result["moved"]), 1)
        self.assertTrue((self.root / result["moved"][0]["to"]).is_file())

    # 45
    def test_45_hygiene_repair_moves_debug_zip_to_artifacts_debug(self):
        src = self.root / "Cortex_DebugBundle_20260909_FULL_FAIL.zip"; src.write_bytes(b"zip")
        result = maintenance.repair_root_hygiene(self.root)
        dest = self.root / result["moved"][0]["to"]
        self.assertEqual(dest.parent, self.root / "artifacts" / "debug")

    # 46
    def test_46_hygiene_repair_moves_legacy_session_logs(self):
        old = self.root / "logs" / "sessions" / "cortex-root-old.log"; old.parent.mkdir(parents=True); old.write_text("x")
        report = maintenance.scan_root_hygiene(self.root)
        self.assertEqual(report["advisoryCount"], 1)
        result = maintenance.repair_root_hygiene(self.root)
        dest = self.root / result["moved"][0]["to"]
        self.assertEqual(dest.parent, self.root / "artifacts" / "logs" / "sessions")

    # 47
    def test_47_hygiene_repair_writes_receipt(self):
        (self.root / "LATEST_DEBUG_BUNDLE.txt").write_text("legacy")
        result = maintenance.repair_root_hygiene(self.root)
        self.assertTrue(Path(result["receipt"]).is_file())

    # 48
    def test_48_latest_debug_pointer_is_artifact_local_and_atomic(self):
        debug = self.root / "artifacts" / "debug"; debug.mkdir(parents=True)
        zp = debug / "Cortex_DebugBundle_x.zip"; zp.write_bytes(b"zip")
        verification = {"sha256": maintenance.sha256_file(zp), "bytes": 3, "verified": True}
        textp, jsonp = maintenance.write_latest_debug_pointer(debug, zp, reason="TEST", exit_code=0, failed_stage="", verification=verification)
        self.assertEqual(textp.parent, debug); self.assertEqual(jsonp.parent, debug)
        self.assertFalse((self.root / "LATEST_DEBUG_BUNDLE.txt").exists())

    # 49
    def test_49_debug_manifest_hashes_tree(self):
        work = self.root / "work"; work.mkdir(); (work / "a.txt").write_text("abc")
        manifest = maintenance.debug_manifest_for_tree(work)
        self.assertEqual(manifest["fileCount"], 1); self.assertEqual(manifest["files"][0]["sha256"], sha(b"abc"))

    # 50
    def test_50_debug_bundle_verifier_accepts_manifested_zip(self):
        work = self.root / "work"; work.mkdir(); (work / "a.txt").write_text("abc")
        maintenance.atomic_write_json(work / "MANIFEST.json", maintenance.debug_manifest_for_tree(work))
        zp = self.root / "debug.zip"
        with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED) as zf:
            for f in work.iterdir(): zf.write(f, f.name)
        result = maintenance.verify_debug_bundle(zp)
        self.assertTrue(result["verified"])

    # 51
    def test_51_debug_bundle_verifier_rejects_tamper(self):
        work = self.root / "work"; work.mkdir(); (work / "a.txt").write_text("abc")
        maintenance.atomic_write_json(work / "MANIFEST.json", maintenance.debug_manifest_for_tree(work))
        zp = self.root / "debug.zip"
        with zipfile.ZipFile(zp, "w") as zf: zf.write(work / "MANIFEST.json", "MANIFEST.json"); zf.writestr("a.txt", b"tampered")
        with self.assertRaises(maintenance.MaintenanceError): maintenance.verify_debug_bundle(zp)

    # 52
    def test_52_debug_sidecar_matches_zip(self):
        zp = self.root / "debug.zip"; zp.write_bytes(b"abc")
        side = maintenance.write_debug_sidecar(zp)
        self.assertTrue(side.read_text().startswith(sha(b"abc")))

    # 53
    def test_53_retention_plan_selects_old_debug_bundles(self):
        d = self.root / "artifacts" / "debug"; d.mkdir(parents=True)
        for i in range(4):
            pth=d/f"Cortex_DebugBundle_{i}.zip"; pth.write_bytes(str(i).encode()); os.utime(pth,(i+1,i+1))
        plan = maintenance.retention_plan(self.root, keep_debug=2, keep_log_files=2)
        self.assertEqual(len([x for x in plan["targets"] if x.endswith(".zip")]), 2)

    # 54
    def test_54_artifact_prune_dry_run_does_not_delete(self):
        d=self.root/"artifacts/debug"; d.mkdir(parents=True)
        for i in range(3):
            pth=d/f"Cortex_DebugBundle_{i}.zip"; pth.write_bytes(b"x"); os.utime(pth,(i+1,i+1))
        result=maintenance.prune_artifacts(self.root, keep_debug=1, keep_log_files=2, apply=False)
        self.assertEqual(result["deleteCount"],2); self.assertEqual(len(list(d.glob("*.zip"))),3)

    # 55
    def test_55_artifact_prune_apply_deletes_old(self):
        d=self.root/"artifacts/debug"; d.mkdir(parents=True)
        for i in range(3):
            pth=d/f"Cortex_DebugBundle_{i}.zip"; pth.write_bytes(b"x"); os.utime(pth,(i+1,i+1))
        result=maintenance.prune_artifacts(self.root, keep_debug=1, keep_log_files=2, apply=True)
        self.assertEqual(len(result["deleted"]),2); self.assertEqual(len(list(d.glob("*.zip"))),1)

    # 56
    def test_56_operation_lock_acquires_and_releases(self):
        lock=self.root/".project_control/pcc-operation.lock"
        with maintenance.OperationLock(self.root,"test"): self.assertTrue(lock.exists())
        self.assertFalse(lock.exists())

    # 57
    def test_57_operation_lock_reclaims_dead_pid(self):
        lock=self.root/".project_control/pcc-operation.lock"; lock.parent.mkdir(parents=True); lock.write_text(json.dumps({"pid":99999999}))
        with maintenance.OperationLock(self.root,"test"): self.assertTrue(lock.exists())
        self.assertFalse(lock.exists())

    # 58
    def test_58_doctor_reports_hygiene_and_disk(self):
        report=maintenance.doctor(self.root)
        self.assertIn("hygiene",report); self.assertIn("disk",report); self.assertTrue(report["hygiene"]["clean"])

    # 59
    def test_59_parser_exposes_python_maintenance_commands(self):
        for command in ("doctor","doctor-json","root-hygiene","root-hygiene-fix","artifact-prune","verify-latest-debug","format","storage-status","storage-prepare","storage-reclaim-plan","vault-mirror","vault-mirror-all","vault-verify"):
            self.assertEqual(pcc.build_parser().parse_args([command,"--root",str(self.root)]).command,command)

    # 60
    def test_60_pcc12_version_and_no_duplicate_fast_menu(self):
        self.assertEqual(pcc.PCC_VERSION,"CTX-PCC-12.4")
        source=(TOOLS/"CortexPCC.py").read_text(encoding="utf-8")
        self.assertEqual(source.count('print(" 21 Fast gate")'),1)

    # 61
    def test_61_gui_command_catalog_merges_contract_and_provider_commands(self):
        repo_root = TOOLS.parents[1]
        contract = ProjectContract.load(repo_root)
        rows = command_catalog(contract)
        keys = {(row.key, row.source) for row in rows}
        self.assertIn(("fmt.check", "project_contract"), keys)
        self.assertIn(("full", "project_provider"), keys)
        self.assertIn(("vault-gc-plan", "project_provider"), keys)
        self.assertGreaterEqual(len(rows), 70)

    # 62
    def test_62_registered_contract_command_resolves_direct_argv(self):
        repo_root = TOOLS.parents[1]
        backend = BackendClient(repo_root, ProjectContract.load(repo_root))
        argv = backend.contract_argv("fmt.check")
        self.assertEqual(argv[0], "cargo")
        self.assertIn("fmt", argv)
        self.assertIn("--check", argv)

    # 63
    def test_63_gui_has_console_composer_and_category_command_surfaces(self):
        source=(TOOLS/"CortexPCCGui.py").read_text(encoding="utf-8")
        for token in (
            "console_input_var",
            "_submit_console_input",
            "_start_cortex_cli",
            "_start_shell_command",
            "Storage & Vault",
            "Command Registry",
            "_scrollable_page_body",
            "_responsive_action_grid",
            'GUI_VERSION = "PCC-GUI-',
        ):
            self.assertIn(token, source)


    def test_63a_gui_scroll_surfaces_are_visible_and_keyboard_accessible(self):
        source=(TOOLS/"CortexPCCGui.py").read_text(encoding="utf-8")
        for token in (
            "Dark.Vertical.TScrollbar",
            "_dark_scrollbar",
            "_scroll_active_page_key",
            "_scroll_active_page_home_end",
            'self.window.bind("<Prior>"',
            'self.window.bind("<Next>"',
            'self.window.bind("<Home>"',
            'self.window.bind("<End>"',
            'takefocus=True',
        ):
            self.assertIn(token, source)
        self.assertGreaterEqual(source.count("self._dark_scrollbar("), 8)


    def test_63b_desktop_registry_consumes_portable_vault_authority(self):
        registry_source=(TOOLS.parents[1]/"crates/cortex_registry/src/lib.rs").read_text(encoding="utf-8")
        desktop_source=(TOOLS.parents[1]/"crates/cortex_desktop_core/src/lib.rs").read_text(encoding="utf-8")
        self.assertIn("CORTEX_VAULT_ROOT", registry_source)
        self.assertIn("PCC_VAULT_ROOT", registry_source)
        self.assertIn("portable_drive_root_vault.v1.json", registry_source)
        self.assertEqual(desktop_source.count("ensure_library_root_for_workspace"), 2)



    # 64
    def test_64_discovery_normalizes_legacy_risk_aliases_without_writing_project(self):
        contract={
            "schema":"forge.project.v1",
            "project":{"id":"fixture","name":"Fixture","kind":"test"},
            "commands":[
                {"key":"inspect","label":"Inspect","program":"tool","risk":"read"},
                {"key":"build","label":"Build","program":"tool","risk":"write","mutates":True},
            ],
            "quality_gates":[],
        }
        path=self.root/"project.control.json"
        path.write_text(json.dumps(contract),encoding="utf-8")
        before=path.read_bytes()
        normalized=discovery.discover_project_contract_data(self.root)
        risks={item["key"]:item["risk"] for item in normalized["commands"]}
        self.assertEqual(risks["inspect"],"read_only")
        self.assertEqual(risks["build"],"local_mutation")
        self.assertEqual(path.read_bytes(),before)

    def test_61_quick_gate_has_explicit_windows_linker_preflight(self) -> None:
        source = (TOOLS / "CortexPCC.py").read_text(encoding="utf-8")
        self.assertIn("Windows linker toolchain", source)
        self.assertIn("HYDRATE_CORTEX_BUILD_TOOLS.cmd", source)
        self.assertIn("windows-linker-toolchain", source)



    def test_65_native_project_pcc_outranks_generic_rust_adapter(self):
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers=[]\n', encoding="utf-8")
        (self.root / "PCC.cmd").write_text('@echo off\npython ProjectControlCenter.py %*\n', encoding="utf-8")
        (self.root / "ProjectControlCenter.py").write_text(
            'commands = ["full", "build", "test", "run", "audit", "updates", "debug-bundle", "dx12"]\n',
            encoding="utf-8",
        )
        manifest_dir = self.root / "project"
        manifest_dir.mkdir()
        (manifest_dir / "forgepy.project.json").write_text(
            json.dumps({"project":{"id":"havenwild-bevy","name":"Havenwild Bevy","kind":"rust-workspace"},"runtime":"dx12"}),
            encoding="utf-8",
        )
        data = discovery.discover_project_contract_data(self.root)
        info = data["_pccDiscovery"]
        self.assertEqual(info["source"], "native-project-pcc")
        self.assertEqual(info["provider"], "PCC.cmd")
        self.assertEqual(info["manifest"], "project/forgepy.project.json")
        self.assertEqual(info["fallback"], "rust-workspace")
        commands = {item["key"]: item for item in data["commands"]}
        self.assertEqual(commands["gate.full"]["program"], "PCC.cmd")
        self.assertEqual(commands["gate.full"]["args"], ["full"])
        self.assertNotEqual(commands["gate.full"]["program"], "__pcc_internal__")
        self.assertEqual(commands["run.runtime"]["args"], ["run", "dx12"])

    def test_66_explicit_project_contract_outranks_native_pcc_and_generic_adapter(self):
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers=[]\n', encoding="utf-8")
        (self.root / "PCC.cmd").write_text('@echo off\n', encoding="utf-8")
        (self.root / "project.control.json").write_text(json.dumps({
            "project":{"id":"fixture","name":"Fixture","kind":"rust-workspace"},
            "commands":[{"key":"gate.full","label":"Authoritative Full","risk":"read_only","program":"custom-tool","args":["full"],"category":"gate"}],
            "quality_gates":[{"key":"gate.full"}],
        }), encoding="utf-8")
        data = discovery.discover_project_contract_data(self.root)
        self.assertEqual(data["_pccDiscovery"]["source"], "project.control.json")
        self.assertEqual(len(data["commands"]), 1)
        self.assertEqual(data["commands"][0]["program"], "custom-tool")

    def test_67_native_project_pcc_summary_exposes_authority_manifest_entrypoint_and_fallback(self):
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers=[]\n', encoding="utf-8")
        (self.root / "PCC.cmd").write_text('@echo off\n', encoding="utf-8")
        (self.root / "project").mkdir()
        (self.root / "project" / "forgepy.project.json").write_text('{}', encoding="utf-8")
        summary = discovery.discovery_summary(self.root)
        self.assertEqual(summary["authority"], "native-project-pcc")
        self.assertEqual(summary["manifest"], "project/forgepy.project.json")
        self.assertEqual(summary["entrypoint"], "PCC.cmd")
        self.assertEqual(summary["fallback"], "rust-workspace")


    def test_68_console_uses_python_cortex_bridge_without_building_on_prompt(self):
        source=(TOOLS/"CortexPCCGui.py").read_text(encoding="utf-8")
        bridge=(TOOLS/"CortexPythonBridge.py").read_text(encoding="utf-8")
        self.assertIn('GUI_VERSION = "PCC-GUI-', source)
        self.assertIn('def _cortex_runtime_root', source)
        self.assertIn('CortexPythonBridge.py', source)
        self.assertIn('environment_root=cortex_root', source)
        self.assertNotIn('"cargo", "run"', source)
        self.assertIn('BRIDGE_VERSION = "CORTEX-PY-BRIDGE-0.7"', bridge)
        self.assertNotIn('cargo", "metadata', bridge)
        self.assertIn('Project operation is not available for', source)
        self.assertIn('/newchat', source)
        self.assertIn('--conversation-id', source)
        self.assertIn('re.fullmatch(r"[A-Za-z0-9_-]+(?:\\.[A-Za-z0-9_-]+)+", text)', source)

    def test_69_universal_process_environment_uses_bounded_toolchain_broker(self):
        storage_source=(TOOLS/"PCCVaultStorage.py").read_text(encoding="utf-8")
        surface_source=(TOOLS/"PCCSurfaceCommon.py").read_text(encoding="utf-8")
        shared_source=(TOOLS/"PCCSharedEnvironment.py").read_text(encoding="utf-8")
        host_source=(TOOLS/"PCCOperationHost.py").read_text(encoding="utf-8")
        for token in ("CORTEX_PYTHON_EXE", "PCC_VAULT_ROOT", "RUSTUP_HOME", 'toolchains / "python"'):
            self.assertIn(token, storage_source)
        self.assertIn("environment_root: Path | None = None", surface_source)
        self.assertIn("apply_shared_toolchain_environment(os.environ.copy(), root=effective_root)", surface_source)
        self.assertIn('BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.4"', shared_source)
        self.assertIn("resolve_toolchain_environment", host_source)
        self.assertIn("Provider PID", host_source)

    def test_69b_patch_apply_requires_explicit_yes(self):
        source=(TOOLS/"PCCAutoAdapter.py").read_text(encoding="utf-8")
        self.assertNotIn('assume_yes or command == "patch-apply"', source)
        self.assertIn('elif assume_yes:', source)



    def test_70_git_identity_round_trip_and_status_summary(self):
        subprocess.run(["git", "init", "-b", "main"], cwd=self.root, check=True, stdout=subprocess.DEVNULL)
        self.assertEqual(git.set_git_identity(self.root, "Cortex Test", "cortex-test@example.invalid", "local"), 0)
        status = git.git_identity_status(self.root)
        self.assertTrue(status["configured"])
        self.assertEqual(status["effectiveName"], "Cortex Test")
        self.assertEqual(status["effectiveEmail"], "cortex-test@example.invalid")
        summary = git.status_summary(self.root)
        self.assertTrue(summary["gitIdentityConfigured"])
        self.assertEqual(summary["gitIdentitySource"], "local")

    def test_71_git_identity_validation_fails_closed(self):
        with self.assertRaises(git.GitError):
            git.validate_git_identity("", "user@example.invalid")
        with self.assertRaises(git.GitError):
            git.validate_git_identity("User", "not-an-email")
        with self.assertRaises(git.GitError):
            git.validate_git_identity("User\nInjected", "user@example.invalid")

    def test_72_gui_exposes_first_class_git_identity_flow(self):
        source=(TOOLS/"CortexPCCGui.py").read_text(encoding="utf-8")
        for token in (
            "Configure Git Identity",
            "_configure_git_identity",
            "_refresh_git_identity_panel",
            "--git-name",
            "--git-email",
            "--git-scope",
            'GUI_VERSION = "PCC-GUI-',
        ):
            self.assertIn(token, source)
        pcc_source=(TOOLS/"CortexPCC.py").read_text(encoding="utf-8")
        self.assertIn('"git-identity"', pcc_source)
        self.assertIn('"git-identity-status"', pcc_source)



    def test_73_project_github_authority_auto_configures_local_identity(self):
        subprocess.run(["git", "init", "-b", "main"], cwd=self.root, check=True, stdout=subprocess.DEVNULL)
        subprocess.run(["git", "config", "--local", "--unset-all", "user.name"], cwd=self.root, check=False)
        subprocess.run(["git", "config", "--local", "--unset-all", "user.email"], cwd=self.root, check=False)
        cfg = self.root / "config" / "cortex" / "github_authority.v1.json"
        cfg.parent.mkdir(parents=True)
        cfg.write_text(json.dumps({
            "schema":"cortex.github_authority.v1",
            "remote":"https://github.com/shifty81/Cortex.git",
            "author":{"name":"shifty81","email":"50773914+shifty81@users.noreply.github.com"}
        }), encoding="utf-8")
        authority = git.apply_project_git_defaults(self.root)
        self.assertEqual(authority["remote"], "https://github.com/shifty81/Cortex.git")
        status = git.git_identity_status(self.root)
        self.assertTrue(status["configured"])
        self.assertEqual(status["source"], "local")
        self.assertEqual(status["localName"], "shifty81")
        self.assertEqual(status["localEmail"], "50773914+shifty81@users.noreply.github.com")
        self.assertEqual(status["effectiveName"], "shifty81")
        self.assertEqual(status["effectiveEmail"], "50773914+shifty81@users.noreply.github.com")




class VaultHealthGuiRegressionTests(unittest.TestCase):
    def test_storage_health_is_dispatched_as_background_metadata_job(self):
        gui = (TOOLS / "CortexPCCGui.py").read_text(encoding="utf-8")
        store = (TOOLS / "PCCVaultStorage.py").read_text(encoding="utf-8")
        self.assertIn('self._start_vault_health_job("Storage Health", vault_quick_storage_health)', gui)
        self.assertIn('self._start_vault_health_job("Deep Health", vault_storage_health, deep=True)', gui)
        self.assertIn('self._start_vault_health_job("CAS GC Plan", vault_cas_gc_plan, deep=True)', gui)
        self.assertIn('threading.Thread(target=work, name=f"vault-health-', gui)
        self.assertIn('elif kind == "vault-health-done":', gui)
        self.assertIn('elif kind == "vault-health-error":', gui)
        self.assertIn('"deepChecksPerformed": False', store)
        self.assertNotIn('ensure_layout(root)\n    deps = dependency_status(root)', store)


if __name__ == "__main__":
    unittest.main(verbosity=2)
