#!/usr/bin/env python3
from __future__ import annotations

import csv
import hashlib
import json
import os
import shutil
import sqlite3
import time
from collections import Counter, defaultdict
from dataclasses import dataclass, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable

from PCCStoragePaths import project_key as storage_project_key, resolve_vault_root

CATALOG_VERSION = "PCC-VAULT-CATALOG-0.1"
HASH_LIMIT = 64 * 1024 * 1024
LARGE_FILE_BYTES = 25 * 1024 * 1024

BUILD_DIRS = {
    "target", "build", "builds", "bin", "obj", "dist", "out", "deriveddatacache",
    "intermediate", "saved", ".cmake", "cmake-build-debug", "cmake-build-release",
}
CACHE_DIRS = {"cache", ".cache", "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache"}
DEPENDENCY_DIRS = {"node_modules", ".gradle", ".cargo", "packages", "deps", "dependencies"}
VENDOR_DIRS = {"vendor", "third_party", "third-party", "extern", "external"}
HISTORY_DIRS = {".git", ".hg", ".svn"}
OPERATIONAL_DIRS = {".cortex", ".project_control", "artifacts", "logs", "updates", "handoffs"}
ARCHIVE_DIRS = {"archive", "archives", "backup", "backups", "handoffs", "rollups"}
REFERENCE_DIRS = {"legacy", "reference", "references", "donor", "donors", "examples", "samples"}
ASSET_DIRS = {"assets", "asset", "content", "gamedata", "resources", "textures", "sprites", "audio", "models", "fonts"}
SOURCE_DIRS = {"src", "source", "sources", "crates", "apps", "engine", "client", "server", "editor", "tools", "scripts"}

SOURCE_EXTS = {
    ".rs", ".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".cs", ".java", ".kt", ".kts",
    ".py", ".js", ".jsx", ".ts", ".tsx", ".lua", ".gd", ".go", ".zig", ".swift", ".rb",
    ".ps1", ".psm1", ".psd1", ".sh", ".bash", ".bat", ".cmd",
}
ASSET_EXTS = {
    ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tga", ".dds", ".svg", ".ico",
    ".wav", ".ogg", ".mp3", ".flac", ".m4a", ".mid", ".midi",
    ".glb", ".gltf", ".fbx", ".obj", ".blend", ".vox", ".vxm", ".dae",
    ".ttf", ".otf", ".woff", ".woff2", ".ase", ".aseprite",
}
CONFIG_EXTS = {".json", ".toml", ".yaml", ".yml", ".xml", ".ini", ".cfg", ".ron", ".properties"}
DOC_EXTS = {".md", ".txt", ".rst", ".adoc", ".pdf"}
ARCHIVE_EXTS = {".zip", ".7z", ".rar", ".tar", ".gz", ".bz2", ".xz"}

CONTROL_NAMES = {
    "project_control_center.cmd", "project.control.json", "subspacetools.ps1", "subspacetools.cmd",
    "project.ps1", "dev.sh", "build.ps1", "projectcontrolcenter.ps1", "projectcontrolcenter.py",
}


@dataclass
class FileRecord:
    rel_path: str
    size: int
    mtime_ns: int
    classification: str
    extension: str
    sha256: str = ""
    json_valid: bool | None = None
    note: str = ""


def _data_root() -> Path:
    return resolve_vault_root()


def project_key(root: Path) -> str:
    return storage_project_key(root)


def catalog_dir(root: Path) -> Path:
    path = _data_root() / "catalogs" / project_key(root)
    path.mkdir(parents=True, exist_ok=True)
    return path


def database_path() -> Path:
    path = _data_root() / "vault_catalog.db"
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _connect() -> sqlite3.Connection:
    db = sqlite3.connect(database_path())
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA foreign_keys=ON")
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS projects(
            project_key TEXT PRIMARY KEY,
            root_path TEXT NOT NULL,
            name TEXT NOT NULL,
            last_scan_utc TEXT NOT NULL,
            scan_version TEXT NOT NULL,
            summary_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS files(
            project_key TEXT NOT NULL,
            rel_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime_ns INTEGER NOT NULL,
            classification TEXT NOT NULL,
            extension TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            json_valid INTEGER,
            note TEXT NOT NULL,
            PRIMARY KEY(project_key, rel_path),
            FOREIGN KEY(project_key) REFERENCES projects(project_key) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_files_hash ON files(project_key, sha256);
        CREATE INDEX IF NOT EXISTS idx_files_class ON files(project_key, classification);
        CREATE INDEX IF NOT EXISTS idx_files_rel ON files(project_key, rel_path);
        """
    )
    return db


def classify_path(root: Path, path: Path, *, is_dir: bool = False) -> str:
    try:
        rel = path.relative_to(root)
    except ValueError:
        return "UNKNOWN"
    parts = [part.casefold() for part in rel.parts]
    names = set(parts[:-1] if not is_dir else parts)
    leaf = path.name.casefold()
    ext = path.suffix.casefold()

    if any(part in HISTORY_DIRS for part in names) or leaf in HISTORY_DIRS:
        return "HISTORY"
    if parts and parts[0] in OPERATIONAL_DIRS:
        return "OPERATIONAL"
    if any(part in BUILD_DIRS for part in names) or leaf in BUILD_DIRS:
        return "BUILD_OUTPUT"
    if any(part in CACHE_DIRS for part in names) or leaf in CACHE_DIRS:
        return "CACHE"
    if any(part in DEPENDENCY_DIRS for part in names) or leaf in DEPENDENCY_DIRS:
        return "DEPENDENCY"
    if any(part in VENDOR_DIRS for part in names) or leaf in VENDOR_DIRS:
        return "VENDOR"
    if any(part in ARCHIVE_DIRS for part in names) or leaf in ARCHIVE_DIRS or ext in ARCHIVE_EXTS:
        return "ARCHIVE"
    if any(part in REFERENCE_DIRS for part in names) or leaf in REFERENCE_DIRS:
        return "REFERENCE"
    if leaf in CONTROL_NAMES or "controlcenter" in leaf or "projectcontrol" in leaf or leaf.endswith("tools.ps1"):
        return "CONTROL"
    if any(part in ASSET_DIRS for part in names) or ext in ASSET_EXTS:
        return "ASSET"
    if any(part in SOURCE_DIRS for part in names) or ext in SOURCE_EXTS:
        return "SOURCE"
    if ext in CONFIG_EXTS or leaf in {"cargo.toml", "cmakelists.txt", "package.json", "pyproject.toml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"}:
        return "CONFIG"
    if ext in DOC_EXTS:
        return "DOCUMENTATION"
    if is_dir:
        return "COMPONENT"
    return "UNKNOWN"


def _should_prune_dir(root: Path, path: Path) -> bool:
    classification = classify_path(root, path, is_dir=True)
    return classification in {"HISTORY", "OPERATIONAL", "BUILD_OUTPUT", "CACHE", "DEPENDENCY"}


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def _cached_hashes(root: Path) -> dict[str, tuple[int, int, str]]:
    key = project_key(root)
    try:
        db = _connect()
        rows = db.execute("SELECT rel_path,size,mtime_ns,sha256 FROM files WHERE project_key=? AND sha256<>''", (key,)).fetchall()
        db.close()
        return {str(rel): (int(size), int(mtime), str(digest)) for rel, size, mtime, digest in rows}
    except Exception:
        return {}


def _environment_inventory(root: Path) -> dict[str, Any]:
    markers = {
        "cargo": root.joinpath("Cargo.toml").is_file(),
        "cmake": root.joinpath("CMakeLists.txt").is_file() or root.joinpath("engine", "CMakeLists.txt").is_file(),
        "gradle": root.joinpath("gradlew").is_file() or root.joinpath("gradlew.bat").is_file() or root.joinpath("build.gradle").is_file(),
        "node": root.joinpath("package.json").is_file(),
        "python": root.joinpath("pyproject.toml").is_file() or root.joinpath("requirements.txt").is_file(),
        "git": root.joinpath(".git").exists(),
    }
    executables = {name: shutil.which(name) or "" for name in ("git", "cargo", "rustc", "cmake", "python", "py", "java", "node", "npm", "pwsh", "powershell")}
    return {"markers": markers, "executables": executables}


def scan_project(
    root: Path,
    *,
    deep_hash: bool = False,
    progress: Callable[[dict[str, Any]], None] | None = None,
    cancelled: Callable[[], bool] | None = None,
) -> dict[str, Any]:
    root = root.expanduser().resolve()
    if not root.is_dir():
        raise FileNotFoundError(root)
    started = time.time()
    cached = _cached_hashes(root)
    records: list[FileRecord] = []
    excluded_dirs: list[dict[str, Any]] = []
    class_counts: Counter[str] = Counter()
    extension_counts: Counter[str] = Counter()
    invalid_json: list[dict[str, str]] = []
    large_files: list[dict[str, Any]] = []
    control_inventory: list[str] = []
    hashed = 0
    reused_hashes = 0

    for current, dirs, files in os.walk(root):
        current_path = Path(current)
        keep_dirs: list[str] = []
        for name in dirs:
            path = current_path / name
            classification = classify_path(root, path, is_dir=True)
            if _should_prune_dir(root, path):
                excluded_dirs.append({"path": path.relative_to(root).as_posix(), "classification": classification})
            else:
                keep_dirs.append(name)
        dirs[:] = keep_dirs

        for name in files:
            if cancelled and cancelled():
                raise InterruptedError("Vault scan cancelled")
            path = current_path / name
            if path.is_symlink():
                continue
            try:
                stat = path.stat()
                if not path.is_file():
                    continue
            except OSError:
                continue
            rel = path.relative_to(root).as_posix()
            classification = classify_path(root, path)
            ext = path.suffix.casefold()
            record = FileRecord(rel, int(stat.st_size), int(stat.st_mtime_ns), classification, ext)
            class_counts[classification] += 1
            if ext:
                extension_counts[ext] += 1
            if stat.st_size >= LARGE_FILE_BYTES:
                large_files.append({"path": rel, "bytes": int(stat.st_size), "classification": classification})
            low = name.casefold()
            if low in CONTROL_NAMES or "controlcenter" in low or "projectcontrol" in low or low.endswith("tools.ps1"):
                control_inventory.append(rel)

            if ext == ".json" and stat.st_size <= 16 * 1024 * 1024:
                try:
                    json.loads(path.read_text(encoding="utf-8-sig"))
                    record.json_valid = True
                except Exception as exc:
                    record.json_valid = False
                    invalid_json.append({"path": rel, "error": str(exc)[:500]})

            hashable = classification not in {"BUILD_OUTPUT", "CACHE", "DEPENDENCY", "HISTORY"}
            size_limit = (512 * 1024 * 1024) if deep_hash else HASH_LIMIT
            if hashable and stat.st_size <= size_limit:
                old = cached.get(rel)
                if old and old[0] == stat.st_size and old[1] == stat.st_mtime_ns and old[2]:
                    record.sha256 = old[2]
                    reused_hashes += 1
                else:
                    try:
                        record.sha256 = _sha256(path)
                        hashed += 1
                    except OSError as exc:
                        record.note = f"hash failed: {exc}"
            elif hashable and stat.st_size > size_limit:
                record.note = "large file: hash deferred" if not deep_hash else "file exceeds deep-hash safety limit"

            records.append(record)
            if progress and len(records) % 250 == 0:
                progress({"files": len(records), "path": rel, "hashed": hashed, "reusedHashes": reused_hashes})

    duplicates: dict[str, list[str]] = defaultdict(list)
    for rec in records:
        if rec.sha256:
            duplicates[rec.sha256].append(rec.rel_path)
    duplicate_groups = [
        {"sha256": digest, "count": len(paths), "paths": sorted(paths)}
        for digest, paths in duplicates.items() if len(paths) > 1
    ]
    duplicate_groups.sort(key=lambda item: (-int(item["count"]), str(item["sha256"])))

    now = datetime.now(timezone.utc).isoformat()
    managed_count = len(records)
    summary = {
        "schema": "pcc.vault_catalog.v1",
        "version": CATALOG_VERSION,
        "projectKey": project_key(root),
        "project": root.name,
        "root": str(root),
        "startedUtc": datetime.fromtimestamp(started, timezone.utc).isoformat(),
        "completedUtc": now,
        "elapsedSeconds": round(time.time() - started, 3),
        "files": managed_count,
        "excludedDirectories": len(excluded_dirs),
        "classCounts": dict(sorted(class_counts.items())),
        "extensionCounts": dict(extension_counts.most_common()),
        "largeFiles": len(large_files),
        "duplicateGroups": len(duplicate_groups),
        "invalidJson": len(invalid_json),
        "hashedFiles": hashed,
        "reusedHashes": reused_hashes,
        "deepHash": bool(deep_hash),
        "controlCenterInventory": sorted(control_inventory),
        "environment": _environment_inventory(root),
    }

    out_dir = catalog_dir(root)
    _write_reports(out_dir, root, records, excluded_dirs, large_files, duplicate_groups, invalid_json, summary)
    _persist_database(root, records, summary)
    if progress:
        progress({"files": len(records), "path": "", "hashed": hashed, "reusedHashes": reused_hashes, "done": True})
    return summary


def _write_reports(
    out_dir: Path,
    root: Path,
    records: list[FileRecord],
    excluded_dirs: list[dict[str, Any]],
    large_files: list[dict[str, Any]],
    duplicate_groups: list[dict[str, Any]],
    invalid_json: list[dict[str, str]],
    summary: dict[str, Any],
) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "scan-summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    (out_dir / "environment.json").write_text(json.dumps(summary.get("environment") or {}, indent=2) + "\n", encoding="utf-8")
    (out_dir / "control-center-inventory.json").write_text(json.dumps(summary.get("controlCenterInventory") or [], indent=2) + "\n", encoding="utf-8")
    (out_dir / "tree.txt").write_text("\n".join(sorted(rec.rel_path for rec in records)) + ("\n" if records else ""), encoding="utf-8")

    with (out_dir / "file-manifest.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=["path", "bytes", "mtime_ns", "classification", "extension", "sha256", "json_valid", "note"])
        writer.writeheader()
        for rec in records:
            writer.writerow({
                "path": rec.rel_path, "bytes": rec.size, "mtime_ns": rec.mtime_ns,
                "classification": rec.classification, "extension": rec.extension,
                "sha256": rec.sha256, "json_valid": "" if rec.json_valid is None else str(rec.json_valid).lower(),
                "note": rec.note,
            })
    with (out_dir / "excluded-files.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=["path", "classification"]); writer.writeheader(); writer.writerows(excluded_dirs)
    with (out_dir / "large-files.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=["path", "bytes", "classification"]); writer.writeheader(); writer.writerows(large_files)
    with (out_dir / "json-validation.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=["path", "error"]); writer.writeheader(); writer.writerows(invalid_json)
    with (out_dir / "duplicate-hashes.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=["sha256", "count", "paths"]); writer.writeheader()
        for item in duplicate_groups:
            writer.writerow({"sha256": item["sha256"], "count": item["count"], "paths": " | ".join(item["paths"])})


def _persist_database(root: Path, records: list[FileRecord], summary: dict[str, Any]) -> None:
    key = project_key(root)
    db = _connect()
    try:
        with db:
            db.execute(
                "INSERT INTO projects(project_key,root_path,name,last_scan_utc,scan_version,summary_json) VALUES(?,?,?,?,?,?) "
                "ON CONFLICT(project_key) DO UPDATE SET root_path=excluded.root_path,name=excluded.name,last_scan_utc=excluded.last_scan_utc,scan_version=excluded.scan_version,summary_json=excluded.summary_json",
                (key, str(root), root.name, summary["completedUtc"], CATALOG_VERSION, json.dumps(summary, separators=(",", ":"))),
            )
            db.execute("DELETE FROM files WHERE project_key=?", (key,))
            db.executemany(
                "INSERT INTO files(project_key,rel_path,size,mtime_ns,classification,extension,sha256,json_valid,note) VALUES(?,?,?,?,?,?,?,?,?)",
                [
                    (key, rec.rel_path, rec.size, rec.mtime_ns, rec.classification, rec.extension, rec.sha256,
                     None if rec.json_valid is None else (1 if rec.json_valid else 0), rec.note)
                    for rec in records
                ],
            )
    finally:
        db.close()


def latest_summary(root: Path) -> dict[str, Any] | None:
    path = catalog_dir(root) / "scan-summary.json"
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8-sig"))
        return data if isinstance(data, dict) else None
    except Exception:
        return None


def catalog_record(root: Path, rel_path: str) -> dict[str, Any] | None:
    key = project_key(root)
    try:
        db = _connect()
        row = db.execute(
            "SELECT rel_path,size,mtime_ns,classification,extension,sha256,json_valid,note FROM files WHERE project_key=? AND rel_path=?",
            (key, rel_path),
        ).fetchone()
        db.close()
    except Exception:
        return None
    if not row:
        return None
    return {
        "relPath": row[0], "bytes": int(row[1]), "mtimeNs": int(row[2]), "classification": row[3],
        "extension": row[4], "sha256": row[5], "jsonValid": None if row[6] is None else bool(row[6]), "note": row[7],
    }


def search_catalog(root: Path, query: str, limit: int = 500) -> list[dict[str, Any]]:
    key = project_key(root)
    term = f"%{query.strip()}%"
    try:
        db = _connect()
        rows = db.execute(
            "SELECT rel_path,size,classification,extension,sha256 FROM files WHERE project_key=? AND rel_path LIKE ? ORDER BY rel_path LIMIT ?",
            (key, term, int(limit)),
        ).fetchall()
        db.close()
    except Exception:
        return []
    return [
        {"relPath": row[0], "bytes": int(row[1]), "classification": row[2], "extension": row[3], "sha256": row[4]}
        for row in rows
    ]


def vault_root() -> Path:
    path = _data_root()
    path.mkdir(parents=True, exist_ok=True)
    return path


def baseline_dir(root: Path) -> Path:
    path = catalog_dir(root) / "baselines"
    path.mkdir(parents=True, exist_ok=True)
    return path


def capture_baseline(root: Path) -> Path:
    root = root.expanduser().resolve()
    key = project_key(root)
    db = _connect()
    try:
        rows = db.execute(
            "SELECT rel_path,size,mtime_ns,classification,sha256 FROM files WHERE project_key=? ORDER BY rel_path",
            (key,),
        ).fetchall()
    finally:
        db.close()
    if not rows:
        raise RuntimeError("No Vault catalog exists yet. Run Scan Active Project first.")
    payload = {
        "schema": "pcc.vault_baseline.v1",
        "version": CATALOG_VERSION,
        "projectKey": key,
        "root": str(root),
        "createdUtc": datetime.now(timezone.utc).isoformat(),
        "files": [
            {"path": r[0], "bytes": int(r[1]), "mtimeNs": int(r[2]), "classification": r[3], "sha256": r[4]}
            for r in rows
        ],
    }
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    target = baseline_dir(root) / f"baseline-{stamp}.json"
    target.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    latest = baseline_dir(root) / "baseline-latest.json"
    temp = latest.with_suffix(".json.tmp")
    temp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    os.replace(temp, latest)
    return target


def compare_baseline(root: Path) -> dict[str, Any]:
    root = root.expanduser().resolve()
    latest = baseline_dir(root) / "baseline-latest.json"
    if not latest.is_file():
        raise RuntimeError("No Vault baseline exists yet. Capture Baseline first.")
    baseline = json.loads(latest.read_text(encoding="utf-8-sig"))
    before = {str(item.get("path")): item for item in baseline.get("files", []) if isinstance(item, dict) and item.get("path")}
    key = project_key(root)
    db = _connect()
    try:
        rows = db.execute(
            "SELECT rel_path,size,mtime_ns,classification,sha256 FROM files WHERE project_key=? ORDER BY rel_path",
            (key,),
        ).fetchall()
    finally:
        db.close()
    after = {
        str(r[0]): {"path": r[0], "bytes": int(r[1]), "mtimeNs": int(r[2]), "classification": r[3], "sha256": r[4]}
        for r in rows
    }
    added = sorted(set(after) - set(before))
    removed = sorted(set(before) - set(after))
    changed: list[str] = []
    for path in sorted(set(before) & set(after)):
        a, b = before[path], after[path]
        ah, bh = str(a.get("sha256") or ""), str(b.get("sha256") or "")
        if ah and bh:
            different = ah != bh
        else:
            different = int(a.get("bytes") or 0) != int(b.get("bytes") or 0) or int(a.get("mtimeNs") or 0) != int(b.get("mtimeNs") or 0)
        if different:
            changed.append(path)
    result = {
        "schema": "pcc.vault_baseline_delta.v1",
        "root": str(root),
        "baselineUtc": baseline.get("createdUtc"),
        "comparedUtc": datetime.now(timezone.utc).isoformat(),
        "added": added,
        "removed": removed,
        "changed": changed,
        "counts": {"added": len(added), "removed": len(removed), "changed": len(changed)},
    }
    target = catalog_dir(root) / "baseline-comparison.json"
    target.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result
