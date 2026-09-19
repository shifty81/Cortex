#!/usr/bin/env python3
"""Fail-closed, non-mutating-source Cortex recovery rollup and verifier.

Requires Python 3.11+. Writes only under artifacts/source-rollups. No network,
Git mutation, installer action, or source file modifications are performed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import uuid
import zipfile
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

SCHEMA = "cortex.source_rollup.v1"
MANIFEST = "SOURCE_ROLLUP_MANIFEST.json"
SKIP_DIRS = {
    ".git", ".cortex", ".project_control", ".vs", ".venv", "venv", "node_modules",
    "artifacts", "logs", "target", "__pycache__", ".pytest_cache", ".mypy_cache",
    ".ruff_cache", ".next", "dist", "build", "out", "coverage", ".idea",
    ".ssh", ".aws", ".azure", ".gnupg",
}
SKIP_EXT = {
    ".pyc", ".pyo", ".exe", ".dll", ".pdb", ".ilk", ".obj", ".lib",
    ".tmp", ".temp", ".log", ".7z", ".rar", ".tar", ".gz", ".xz",
    ".gguf", ".onnx", ".safetensors", ".bin", ".patch", ".zip",
    ".pem", ".pfx", ".p12", ".key",
}
SKIP_NAMES = {
    ".env", ".npmrc", ".pypirc", ".netrc", "id_rsa", "id_ed25519", "id_ecdsa",
    "credentials.json", "secrets.json", "token.json", "auth.json", "thumbs.db",
    ".ds_store", MANIFEST.lower(),
}
MAX_PATH = 240
DEFAULT_MAX_FILE = 64 * 1024 * 1024
DEFAULT_MAX_TOTAL = 1024 * 1024 * 1024
DEFAULT_MAX_COUNT = 50_000


class RollupError(RuntimeError):
    pass


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def link_or_reparse(path: Path) -> bool:
    info = path.lstat()
    return stat.S_ISLNK(info.st_mode) or bool(
        getattr(info, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    )


def safe_name(rel: str) -> None:
    p = PurePosixPath(rel)
    if not rel or rel.startswith("/") or "\\" in rel or re.match(r"^[a-zA-Z]:", rel):
        raise RollupError("unsafe archive path: " + rel)
    if len(rel) > MAX_PATH or p.as_posix() != rel or any(part in ("", ".", "..") for part in p.parts):
        raise RollupError("invalid archive path: " + rel)


def check_output_dir(root: Path) -> Path:
    current = root
    for part in ("artifacts", "source-rollups"):
        current = current / part
        if current.exists() or current.is_symlink():
            if link_or_reparse(current) or not current.is_dir():
                raise RollupError("output directory is linked or not a directory: " + str(current))
        else:
            current.mkdir()
    return current


def classify(rel: str, is_dir: bool) -> str | None:
    p = PurePosixPath(rel)
    name = p.name.casefold()
    if name in SKIP_NAMES or name in {".git", ".cortex", ".project_control"} or name.startswith(".env."):
        return "sensitive-or-reserved-name"
    if is_dir:
        return "generated-or-operational-directory" if name in SKIP_DIRS else None
    if p.suffix.casefold() in SKIP_EXT:
        return "generated-binary-or-transport-file"
    return None


def walk_source(root: Path) -> tuple[list[tuple[str, Path]], list[dict[str, str]]]:
    files: list[tuple[str, Path]] = []
    excluded: list[dict[str, str]] = []

    def descend(directory: Path, prefix: str) -> None:
        with os.scandir(directory) as scan:
            entries = sorted(scan, key=lambda e: (e.name.casefold(), e.name))
        for entry in entries:
            rel = f"{prefix}/{entry.name}" if prefix else entry.name
            safe_name(rel)
            path = Path(entry.path)
            info = path.lstat()
            is_dir = stat.S_ISDIR(info.st_mode)
            if link_or_reparse(path):
                excluded.append({"path": rel, "reason": "symlink-or-reparse-point"})
                continue
            reason = classify(rel, is_dir)
            if reason:
                excluded.append({"path": rel, "reason": reason})
                continue
            if is_dir:
                descend(path, rel)
            elif stat.S_ISREG(info.st_mode):
                files.append((rel, path))
            else:
                excluded.append({"path": rel, "reason": "non-regular-file"})

    descend(root, "")
    folded = [rel.casefold() for rel, _ in files]
    if len(folded) != len(set(folded)):
        raise RollupError("case-insensitive duplicate source paths")
    return files, excluded


def git_info(root: Path) -> dict[str, object]:
    def call(*args: str) -> str | None:
        try:
            result = subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True,
                                    encoding="utf-8", errors="replace", timeout=12, check=False)
        except (OSError, subprocess.TimeoutExpired):
            return None
        return result.stdout.strip() if result.returncode == 0 else None

    top = call("rev-parse", "--show-toplevel")
    if top is None or os.path.normcase(str(Path(top).resolve())) != os.path.normcase(str(root.resolve())):
        return {"repository": False, "head": None, "branch": None, "changedPathCount": None}
    status = call("status", "--porcelain=v1", "--untracked-files=all")
    return {"repository": True, "head": call("rev-parse", "HEAD"),
            "branch": call("branch", "--show-current"),
            "changedPathCount": None if status is None else len(status.splitlines())}


def patch_state(root: Path) -> dict[str, object]:
    receipts: list[dict[str, str]] = []
    receipt_root = root / "artifacts" / "patches" / "receipts"
    if receipt_root.exists():
        for ancestor in (root / "artifacts", root / "artifacts" / "patches", receipt_root):
            if link_or_reparse(ancestor) or not ancestor.is_dir():
                raise RollupError("patch receipt path is linked or not a directory: " + str(ancestor))
        for file in sorted(receipt_root.glob("*.json"), key=lambda p: p.name):
            if link_or_reparse(file) or not file.is_file():
                continue
            if file.stat().st_size > 2_000_000:
                raise RollupError("oversized patch receipt: " + file.name)
            try:
                data = json.loads(file.read_text(encoding="utf-8"))
            except (UnicodeError, ValueError) as exc:
                raise RollupError("unreadable patch receipt: " + file.name) from exc
            if not isinstance(data, dict):
                raise RollupError("invalid patch receipt: " + file.name)
            # Whitelist safe lineage fields, never embed raw receipt paths, logs or credentials.
            receipts.append({"patchId": str(data.get("patchId", ""))[:128],
                             "status": str(data.get("status", ""))[:24],
                             "receiptSha256": sha_file(file)})
    transports: list[dict[str, str | int]] = []
    for path in sorted(root.glob("*.zip"), key=lambda p: p.name):
        if link_or_reparse(path) or not path.is_file():
            continue
        try:
            with zipfile.ZipFile(path) as zf:
                names = zf.namelist()
                if names.count("PATCH_MANIFEST.json") != 1:
                    continue
                if zf.getinfo("PATCH_MANIFEST.json").file_size > 1_000_000:
                    raise RollupError("oversized patch manifest: " + path.name)
                meta = json.loads(zf.read("PATCH_MANIFEST.json"))
                if not isinstance(meta, dict) or meta.get("schema") != "cortex.root_patch.v1":
                    continue
                if path.stat().st_size > DEFAULT_MAX_FILE:
                    raise RollupError("pending patch exceeds 64 MiB; archive separately: " + path.name)
                transports.append({"filename": path.name, "patchId": str(meta.get("patchId", ""))[:128],
                                   "bytes": path.stat().st_size, "sha256": sha_file(path)})
        except (OSError, zipfile.BadZipFile, UnicodeError, ValueError, RuntimeError) as exc:
            raise RollupError("unreadable root ZIP: " + path.name) from exc
    return {"appliedReceipts": receipts, "pendingTransports": transports}


def create(root: Path, *, max_file: int = DEFAULT_MAX_FILE,
           max_total: int = DEFAULT_MAX_TOTAL, max_count: int = DEFAULT_MAX_COUNT) -> Path:
    root = root.absolute()
    if not root.is_dir() or link_or_reparse(root):
        raise RollupError("root must be a real directory, not a symlink/reparse point")
    if not (root / "Cargo.toml").is_file() or not (root / "project.control.json").is_file():
        raise RollupError("not an identifiable Cortex source root (Cargo.toml and project.control.json required)")
    files, excluded = walk_source(root)
    if not files:
        raise RollupError("no source files found")
    if len(files) > max_count:
        raise RollupError(f"source file count exceeds cap ({max_count}); no archive written")
    total = sum(path.lstat().st_size for _, path in files)
    oversize = [rel for rel, path in files if path.lstat().st_size > max_file]
    if oversize or total > max_total:
        raise RollupError("source exceeds configured safety limits; no archive written: " +
                          (", ".join(oversize[:5]) if oversize else f"total={total}"))
    git_before = git_info(root)
    lineage = patch_state(root)
    outdir = check_output_dir(root)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    filename = f"Cortex_SourceRollup_{stamp}_{uuid.uuid4().hex[:8]}.zip"
    destination = outdir / filename
    temp = outdir / (filename + ".tmp")
    records: list[dict[str, object]] = []
    try:
        with zipfile.ZipFile(temp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6,
                             allowZip64=True) as zf:
            for rel, path in files:
                before = path.lstat()
                if link_or_reparse(path) or not stat.S_ISREG(before.st_mode):
                    raise RollupError("source changed to link/nonfile during capture: " + rel)
                if before.st_size > max_file:
                    raise RollupError("source grew over cap during capture: " + rel)
                contents = path.read_bytes()
                after = path.lstat()
                if link_or_reparse(path) or not stat.S_ISREG(after.st_mode):
                    raise RollupError("source changed to link/nonfile during read: " + rel)
                if (before.st_size, before.st_mtime_ns, before.st_ino) != (after.st_size, after.st_mtime_ns, after.st_ino):
                    raise RollupError("source changed while reading: " + rel)
                if len(contents) != before.st_size:
                    raise RollupError("short read: " + rel)
                zf.writestr(rel, contents)
                records.append({"path": rel, "bytes": len(contents), "sha256": digest(contents)})
            # Check the source is not changing while packaging. Read again; no mixed-tree success.
            for record in records:
                current = root / str(record["path"])
                if link_or_reparse(current) or sha_file(current) != record["sha256"]:
                    raise RollupError("source changed during archive construction: " + str(record["path"]))
            if walk_source(root)[0] != files:
                raise RollupError("source file set changed during archive construction")
            git_after = git_info(root)
            if git_before != git_after:
                raise RollupError("Git source state changed during archive construction")
            manifest = {"schema": SCHEMA, "createdUtc": datetime.now(timezone.utc).isoformat(),
                        "rootBasename": root.name, "git": git_after, "patchLineage": lineage,
                        "fileCount": len(records), "sourceBytes": sum(int(r["bytes"]) for r in records),
                        "files": records, "exclusions": excluded,
                        "disclaimer": "Source snapshot excludes generated/operational/binary and sensitive paths listed above. "
                                      "Review ZIP before sharing; embedded source content may contain secrets."}
            zf.writestr(MANIFEST, json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        verify(temp)
        sha_path = outdir / (filename + ".sha256")
        checksum = sha_file(temp)
        sha_tmp = outdir / (sha_path.name + ".tmp")
        try:
            sha_tmp.write_text(f"{checksum}  {filename}\n", encoding="ascii")
            os.replace(temp, destination)
            try:
                os.replace(sha_tmp, sha_path)
            except OSError:
                destination.unlink(missing_ok=True)
                raise
        finally:
            sha_tmp.unlink(missing_ok=True)
        return destination
    finally:
        temp.unlink(missing_ok=True)


def verify(archive: Path) -> dict[str, object]:
    with zipfile.ZipFile(archive, "r") as zf:
        names = zf.namelist()
        if len(names) != len(set(n.casefold() for n in names)) or names.count(MANIFEST) != 1:
            raise RollupError("duplicate or missing manifest")
        if len(names) > DEFAULT_MAX_COUNT + 1:
            raise RollupError("archive exceeds file-count budget")
        if zf.getinfo(MANIFEST).file_size > 25 * 1024 * 1024:
            raise RollupError("archive manifest exceeds verification budget")
        for name in names:
            safe_name(name)
            if name != MANIFEST and zf.getinfo(name).file_size > DEFAULT_MAX_FILE:
                raise RollupError("archive entry exceeds verification budget: " + name)
            mode = (zf.getinfo(name).external_attr >> 16) & 0xFFFF
            if mode and (stat.S_ISLNK(mode) or stat.S_ISCHR(mode) or stat.S_ISFIFO(mode)):
                raise RollupError("nonregular ZIP entry: " + name)
        manifest = json.loads(zf.read(MANIFEST))
        if manifest.get("schema") != SCHEMA:
            raise RollupError("invalid source rollup schema")
        files = manifest.get("files")
        if not isinstance(files, list) or len(files) != manifest.get("fileCount"):
            raise RollupError("file inventory mismatch")
        listed = set()
        total = 0
        for record in files:
            rel = record["path"]
            safe_name(rel)
            if rel in listed or rel == MANIFEST or rel not in names:
                raise RollupError("missing/duplicate inventory entry: " + rel)
            listed.add(rel)
            data = zf.read(rel)
            if digest(data) != record["sha256"] or len(data) != record["bytes"]:
                raise RollupError("file digest/size mismatch: " + rel)
            total += len(data)
            if total > DEFAULT_MAX_TOTAL:
                raise RollupError("archive exceeds total-size verification budget")
        if listed != set(names) - {MANIFEST} or total != manifest.get("sourceBytes"):
            raise RollupError("unexpected ZIP files or aggregate source size mismatch")
        return {"schema": SCHEMA, "fileCount": len(files), "sourceBytes": total,
                "excludedCount": len(manifest.get("exclusions", [])), "sha256": sha_file(archive)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Cortex read-only source rollup and integrity verification")
    sub = parser.add_subparsers(dest="command", required=True)
    make = sub.add_parser("create", help="capture source + selected patch lineage without altering source")
    make.add_argument("--root", type=Path, required=True)
    check = sub.add_parser("verify", help="validate every source payload against its SHA-256 manifest")
    check.add_argument("--archive", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "create":
            archive = create(args.root)
            print("SOURCE_ROLLUP_CREATED=" + str(archive))
            print("SOURCE_ROLLUP_SHA256=" + sha_file(archive))
        else:
            print("SOURCE_ROLLUP_VERIFIED=" + json.dumps(verify(args.archive), sort_keys=True))
        return 0
    except (RollupError, OSError, ValueError, zipfile.BadZipFile) as exc:
        print("SOURCE_ROLLUP_FAILED=" + str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
