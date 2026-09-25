#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import gzip
import hashlib
import json
import mimetypes
import os
import re
import sqlite3
import stat as statmod
import time
import zipfile
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable

from PCCStoragePaths import resolve_vault_root

INTAKE_AUDIT_VERSION = "PCC-VAULT-INTAKE-AUDIT-0.1"
DB_SCHEMA = "pcc.vault_intake_audit.v1"
FULL_HASH_LIMIT = 8 * 1024 * 1024
SAMPLE_CHUNK = 64 * 1024
ZIP_ENTRY_SAMPLE_LIMIT = 2000
CHAT_HANDOFF_LIMIT = 5000

SOURCE_EXTS = {
    ".rs": "Rust", ".c": "C", ".cc": "C++", ".cpp": "C++", ".cxx": "C++",
    ".h": "C/C++ Header", ".hh": "C++ Header", ".hpp": "C++ Header",
    ".cs": "C#", ".java": "Java", ".kt": "Kotlin", ".kts": "Kotlin",
    ".py": "Python", ".js": "JavaScript", ".jsx": "JavaScript/JSX",
    ".ts": "TypeScript", ".tsx": "TypeScript/TSX", ".lua": "Lua", ".gd": "GDScript",
    ".go": "Go", ".zig": "Zig", ".swift": "Swift", ".rb": "Ruby",
    ".ps1": "PowerShell", ".psm1": "PowerShell", ".sh": "Shell", ".bash": "Shell",
    ".bat": "Windows Batch", ".cmd": "Windows Batch", ".glsl": "GLSL",
    ".hlsl": "HLSL", ".wgsl": "WGSL",
}
IMAGE_EXTS = {".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tga", ".dds", ".svg", ".ico", ".exr", ".hdr"}
AUDIO_EXTS = {".wav", ".ogg", ".mp3", ".flac", ".m4a", ".aac", ".opus", ".mid", ".midi"}
VIDEO_EXTS = {".mp4", ".mkv", ".mov", ".avi", ".webm", ".wmv", ".m4v"}
MODEL_EXTS = {".glb", ".gltf", ".fbx", ".obj", ".blend", ".vox", ".vxm", ".dae", ".3ds", ".stl", ".ply"}
FONT_EXTS = {".ttf", ".otf", ".woff", ".woff2", ".fnt"}
PIXEL_EXTS = {".ase", ".aseprite"}
DOC_EXTS = {".md", ".txt", ".rst", ".adoc", ".pdf", ".doc", ".docx", ".odt", ".rtf", ".epub"}
SHEET_EXTS = {".csv", ".tsv", ".xls", ".xlsx", ".ods"}
CONFIG_EXTS = {".json", ".toml", ".yaml", ".yml", ".xml", ".ini", ".cfg", ".ron", ".properties", ".env"}
ARCHIVE_EXTS = {".zip", ".7z", ".rar", ".tar", ".gz", ".bz2", ".xz", ".tgz", ".tbz", ".txz"}
EXECUTABLE_EXTS = {".exe", ".msi", ".msix", ".appx", ".appxbundle", ".dll", ".sys", ".scr", ".com"}
DISK_EXTS = {".iso", ".img", ".vhd", ".vhdx", ".qcow", ".qcow2"}
DATABASE_EXTS = {".db", ".sqlite", ".sqlite3", ".mdb", ".accdb"}

BUILD_NAMES = {"target", "build", "builds", "bin", "obj", "dist", "out", "deriveddatacache", "intermediate", "saved"}
CACHE_NAMES = {"cache", ".cache", "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".gradle"}
DEPENDENCY_NAMES = {"node_modules", ".cargo", "packages", "deps", "dependencies", "vendor", "third_party", "third-party", "extern", "external"}
HISTORY_NAMES = {".git", ".hg", ".svn"}
BACKUP_WORDS = {"backup", "backups", "bak", "old", "older", "archive", "archives", "rollup", "rollups", "snapshot", "snapshots"}
REFERENCE_WORDS = {"reference", "references", "donor", "donors", "example", "examples", "sample", "samples", "legacy"}
LICENSE_NAMES = {"license", "license.txt", "license.md", "copying", "copying.txt", "notice", "notice.txt"}
README_NAMES = {"readme", "readme.md", "readme.txt", "readme.rst"}
CREDIT_NAMES = {"credits", "credits.txt", "credits.md", "authors", "authors.txt", "attribution", "attribution.txt"}
CONTROL_NAMES = {"project.control.json", "project_control_center.cmd", "pcc.cmd", "pcc.ps1", "projectcontrolcenter.py"}

PROJECT_MARKERS: dict[str, str] = {
    "cargo.toml": "Rust/Cargo",
    "cmakelists.txt": "CMake/C++",
    "package.json": "Node/Web",
    "pyproject.toml": "Python",
    "requirements.txt": "Python",
    "go.mod": "Go",
    "pom.xml": "Java/Maven",
    "build.gradle": "Java/Gradle",
    "build.gradle.kts": "Java/Gradle",
    "settings.gradle": "Java/Gradle",
    "settings.gradle.kts": "Java/Gradle",
    "project.godot": "Godot",
    "project.control.json": "PCC Project",
}
PROJECT_SUFFIX_MARKERS: dict[str, str] = {
    ".sln": "Visual Studio",
    ".csproj": ".NET/C#",
    ".vcxproj": "Visual C++",
    ".uproject": "Unreal Engine",
    ".godot": "Godot",
}

VERSION_RE = re.compile(
    r"(?:^|[._\-\s(])(?:v(?:er(?:sion)?)?\s*)?(\d{1,4}(?:[._-]\d{1,4}){0,3})(?:$|[._\-\s)])",
    re.IGNORECASE,
)
DATE_RE = re.compile(r"(?:19|20)\d{2}[._-]?(?:0[1-9]|1[0-2])[._-]?(?:0[1-9]|[12]\d|3[01])")
COPY_RE = re.compile(r"(?:\bcopy\b|\(\d+\)$|\bbackup\b|\bbak\b|\bold\b)", re.IGNORECASE)


def _utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def _stamp() -> str:
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def _human_bytes(value: int) -> str:
    size = float(max(0, value))
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    index = 0
    while size >= 1024.0 and index < len(units) - 1:
        size /= 1024.0
        index += 1
    return f"{size:.2f} {units[index]}"


def audit_root(project_root: Path | None = None) -> Path:
    vault = resolve_vault_root(project_root)
    candidates = [vault / "Intake", vault / "intake"]
    for item in candidates:
        if item.is_dir():
            return item.resolve()
    if vault.is_dir():
        try:
            for child in vault.iterdir():
                if child.is_dir() and child.name.casefold() == "intake":
                    return child.resolve()
        except OSError:
            pass
    return (vault / "Intake").resolve()


def audit_dir(project_root: Path | None = None) -> Path:
    path = resolve_vault_root(project_root) / "catalogs" / "intake"
    path.mkdir(parents=True, exist_ok=True)
    return path


def database_path(project_root: Path | None = None) -> Path:
    return audit_dir(project_root) / "intake_audit.db"


def latest_summary_path(project_root: Path | None = None) -> Path:
    return audit_dir(project_root) / "latest-summary.json"


def latest_handoff_path(project_root: Path | None = None) -> Path:
    return audit_dir(project_root) / "LATEST_HANDOFF.txt"


def _connect(project_root: Path | None = None) -> sqlite3.Connection:
    db = sqlite3.connect(database_path(project_root))
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("PRAGMA synchronous=NORMAL")
    db.executescript(
        """
        CREATE TABLE IF NOT EXISTS files(
            rel_path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime_ns INTEGER NOT NULL,
            extension TEXT NOT NULL,
            mime TEXT NOT NULL,
            role TEXT NOT NULL,
            subtype TEXT NOT NULL,
            language TEXT NOT NULL,
            signature TEXT NOT NULL,
            source_zone TEXT NOT NULL,
            top_group TEXT NOT NULL,
            size_bucket TEXT NOT NULL,
            age_bucket TEXT NOT NULL,
            generated INTEGER NOT NULL,
            risk TEXT NOT NULL,
            version_hint TEXT NOT NULL,
            quick_hash TEXT NOT NULL,
            full_sha256 TEXT NOT NULL,
            note TEXT NOT NULL,
            last_scan_id TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_intake_role ON files(role);
        CREATE INDEX IF NOT EXISTS idx_intake_ext ON files(extension);
        CREATE INDEX IF NOT EXISTS idx_intake_quick_hash ON files(size,quick_hash);
        CREATE INDEX IF NOT EXISTS idx_intake_full_hash ON files(full_sha256);
        CREATE INDEX IF NOT EXISTS idx_intake_group ON files(top_group);

        CREATE TABLE IF NOT EXISTS project_candidates(
            root_rel TEXT PRIMARY KEY,
            markers_json TEXT NOT NULL,
            engine_hint TEXT NOT NULL,
            has_git INTEGER NOT NULL,
            has_pcc INTEGER NOT NULL,
            has_license INTEGER NOT NULL,
            has_readme INTEGER NOT NULL,
            has_credits INTEGER NOT NULL,
            last_scan_id TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS archives(
            rel_path TEXT PRIMARY KEY,
            format TEXT NOT NULL,
            entry_count INTEGER NOT NULL,
            sampled_entries INTEGER NOT NULL,
            uncompressed_bytes INTEGER NOT NULL,
            compressed_bytes INTEGER NOT NULL,
            project_markers_json TEXT NOT NULL,
            suspicious INTEGER NOT NULL,
            note TEXT NOT NULL,
            last_scan_id TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS scans(
            scan_id TEXT PRIMARY KEY,
            root_path TEXT NOT NULL,
            started_utc TEXT NOT NULL,
            completed_utc TEXT NOT NULL,
            status TEXT NOT NULL,
            files INTEGER NOT NULL,
            directories INTEGER NOT NULL,
            bytes INTEGER NOT NULL,
            errors INTEGER NOT NULL,
            reused INTEGER NOT NULL,
            hashed INTEGER NOT NULL,
            summary_json TEXT NOT NULL
        );
        """
    )
    return db


def _size_bucket(size: int) -> str:
    if size < 1024:
        return "tiny_lt_1KiB"
    if size < 1024 * 1024:
        return "small_lt_1MiB"
    if size < 64 * 1024 * 1024:
        return "medium_lt_64MiB"
    if size < 1024 * 1024 * 1024:
        return "large_lt_1GiB"
    return "huge_ge_1GiB"


def _age_bucket(mtime_ns: int) -> str:
    age_days = max(0.0, (time.time_ns() - int(mtime_ns)) / 1_000_000_000 / 86400.0)
    if age_days < 7:
        return "lt_7d"
    if age_days < 30:
        return "7_30d"
    if age_days < 180:
        return "30_180d"
    if age_days < 365:
        return "180_365d"
    if age_days < 730:
        return "1_2y"
    return "ge_2y"


def _source_zone(rel: Path) -> str:
    parts = [part.casefold().replace("_", " ").replace("-", " ").strip() for part in rel.parts]
    if parts and parts[0] in {"needs sorted", "needssorted", "unsorted", "needs review", "review"}:
        return "needs_sorted"
    return "intake"


def _top_group(rel: Path) -> str:
    return rel.parts[0] if rel.parts else "."


def _path_generated(rel: Path) -> bool:
    names = {part.casefold() for part in rel.parts[:-1]}
    return bool(names & (BUILD_NAMES | CACHE_NAMES | DEPENDENCY_NAMES))


def _version_hint(path: Path) -> str:
    stem = path.stem
    parts: list[str] = []
    if COPY_RE.search(stem):
        parts.append("copy_or_backup_name")
    m = VERSION_RE.search(stem)
    if m:
        parts.append(f"version={m.group(1)}")
    d = DATE_RE.search(stem)
    if d:
        parts.append(f"date={d.group(0)}")
    return ";".join(parts)


def _signature(path: Path) -> str:
    try:
        with path.open("rb") as fh:
            head = fh.read(32)
    except OSError:
        return ""
    signatures = [
        (b"PK\x03\x04", "ZIP"), (b"PK\x05\x06", "ZIP"), (b"7z\xbc\xaf\x27\x1c", "7Z"),
        (b"Rar!\x1a\x07", "RAR"), (b"\x89PNG\r\n\x1a\n", "PNG"), (b"\xff\xd8\xff", "JPEG"),
        (b"GIF87a", "GIF"), (b"GIF89a", "GIF"), (b"BM", "BMP"), (b"RIFF", "RIFF"),
        (b"OggS", "OGG"), (b"fLaC", "FLAC"), (b"%PDF-", "PDF"), (b"MZ", "PE"),
        (b"SQLite format 3\x00", "SQLITE"),
    ]
    for magic, label in signatures:
        if head.startswith(magic):
            return label
    if len(head) >= 12 and head[4:8] == b"ftyp":
        return "MP4_FAMILY"
    return ""


def _quick_hash(path: Path, size: int) -> str:
    h = hashlib.sha256()
    h.update(str(size).encode("ascii"))
    h.update(b"\0")
    try:
        with path.open("rb") as fh:
            if size <= SAMPLE_CHUNK * 3:
                for block in iter(lambda: fh.read(1024 * 1024), b""):
                    h.update(block)
            else:
                h.update(fh.read(SAMPLE_CHUNK))
                midpoint = max(0, (size // 2) - (SAMPLE_CHUNK // 2))
                fh.seek(midpoint)
                h.update(fh.read(SAMPLE_CHUNK))
                fh.seek(max(0, size - SAMPLE_CHUNK))
                h.update(fh.read(SAMPLE_CHUNK))
    except OSError:
        return ""
    return h.hexdigest()


def _full_hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def _classify(rel: Path, path: Path, signature: str) -> tuple[str, str, str, str, bool, str]:
    ext = path.suffix.casefold()
    leaf = path.name.casefold()
    names = {part.casefold() for part in rel.parts[:-1]}
    mime = mimetypes.guess_type(path.name)[0] or ""
    language = SOURCE_EXTS.get(ext, "")
    generated = _path_generated(rel)

    if names & HISTORY_NAMES:
        return "HISTORY", "vcs_history", language, mime, generated, "review"
    if names & BUILD_NAMES:
        return "BUILD_OUTPUT", "generated_build", language, mime, True, "rebuildable"
    if names & CACHE_NAMES:
        return "CACHE", "generated_cache", language, mime, True, "rebuildable"
    if names & DEPENDENCY_NAMES:
        return "DEPENDENCY", "vendored_or_package_tree", language, mime, True, "third_party_review"
    if leaf in CONTROL_NAMES or "controlcenter" in leaf or "projectcontrol" in leaf:
        return "CONTROL", "project_control", language, mime, generated, "review"
    if ext in ARCHIVE_EXTS or signature in {"ZIP", "7Z", "RAR"}:
        return "ARCHIVE", ext.lstrip(".") or signature.casefold(), language, mime, generated, "archive_review"
    if ext in EXECUTABLE_EXTS or signature == "PE":
        subtype = "installer" if ext in {".msi", ".msix", ".appx", ".appxbundle"} else "windows_binary"
        return "BINARY", subtype, language, mime, generated, "executable_review"
    if ext in DISK_EXTS:
        return "DISK_IMAGE", ext.lstrip("."), language, mime, generated, "archive_review"
    if ext in IMAGE_EXTS or signature in {"PNG", "JPEG", "GIF", "BMP"}:
        return "ASSET", "image", language, mime, generated, "asset_library"
    if ext in AUDIO_EXTS or signature in {"OGG", "FLAC", "RIFF"}:
        return "ASSET", "audio", language, mime, generated, "asset_library"
    if ext in VIDEO_EXTS or signature == "MP4_FAMILY":
        return "ASSET", "video", language, mime, generated, "asset_library"
    if ext in MODEL_EXTS:
        return "ASSET", "3d_model", language, mime, generated, "asset_library"
    if ext in FONT_EXTS:
        return "ASSET", "font", language, mime, generated, "asset_library"
    if ext in PIXEL_EXTS:
        return "ASSET", "pixel_source", language, mime, generated, "asset_library"
    if language:
        subtype = "shader" if ext in {".glsl", ".hlsl", ".wgsl"} else "source_code"
        return "SOURCE", subtype, language, mime, generated, "project_or_snippet"
    if ext in CONFIG_EXTS or leaf in PROJECT_MARKERS:
        return "CONFIG", "configuration", language, mime, generated, "project_or_config"
    if ext in DOC_EXTS or signature == "PDF":
        return "DOCUMENTATION", "document", language, mime, generated, "reference_or_docs"
    if ext in SHEET_EXTS:
        return "DATA", "spreadsheet", language, mime, generated, "data_review"
    if ext in DATABASE_EXTS or signature == "SQLITE":
        return "DATA", "database", language, mime, generated, "data_review"
    if names & BACKUP_WORDS or any(word in leaf for word in ("backup", "rollup", "snapshot")):
        return "REFERENCE", "backup_or_snapshot", language, mime, generated, "archive_review"
    if names & REFERENCE_WORDS:
        return "REFERENCE", "reference_material", language, mime, generated, "reference_library"
    return "UNKNOWN", "unknown", language, mime, generated, "manual_review"


def _project_marker_for(path: Path) -> str:
    leaf = path.name.casefold()
    if leaf in PROJECT_MARKERS:
        return PROJECT_MARKERS[leaf]
    ext = path.suffix.casefold()
    return PROJECT_SUFFIX_MARKERS.get(ext, "")


def _project_root_for_marker(rel: Path, path: Path) -> Path:
    if path.name.casefold() == "projectversion.txt" and len(rel.parts) >= 2 and rel.parts[-2].casefold() == "projectsettings":
        return Path(*rel.parts[:-2]) if len(rel.parts) > 2 else Path(".")
    return rel.parent


def _marker_kind(rel: Path, path: Path) -> str:
    kind = _project_marker_for(path)
    if kind:
        return kind
    if path.name.casefold() == "projectversion.txt" and rel.parent.name.casefold() == "projectsettings":
        return "Unity"
    return ""


def _inspect_zip(path: Path, rel: str) -> dict[str, Any]:
    result = {
        "relPath": rel,
        "format": "zip",
        "entryCount": 0,
        "sampledEntries": 0,
        "uncompressedBytes": 0,
        "compressedBytes": 0,
        "projectMarkers": [],
        "suspicious": False,
        "note": "",
    }
    try:
        with zipfile.ZipFile(path) as zf:
            infos = zf.infolist()
            result["entryCount"] = len(infos)
            markers: set[str] = set()
            total_u = 0
            total_c = 0
            sampled = 0
            for info in infos:
                total_u += int(info.file_size)
                total_c += int(info.compress_size)
                if sampled < ZIP_ENTRY_SAMPLE_LIMIT:
                    sampled += 1
                    leaf = Path(info.filename).name.casefold()
                    if leaf in PROJECT_MARKERS:
                        markers.add(PROJECT_MARKERS[leaf])
                    ext = Path(leaf).suffix.casefold()
                    if ext in PROJECT_SUFFIX_MARKERS:
                        markers.add(PROJECT_SUFFIX_MARKERS[ext])
            result["sampledEntries"] = sampled
            result["uncompressedBytes"] = total_u
            result["compressedBytes"] = total_c
            result["projectMarkers"] = sorted(markers)
            ratio = (total_u / max(1, total_c)) if total_u else 0.0
            result["suspicious"] = bool(ratio >= 100.0 or total_u >= 100 * 1024 * 1024 * 1024)
            if len(infos) > ZIP_ENTRY_SAMPLE_LIMIT:
                result["note"] = f"project-marker inspection sampled first {ZIP_ENTRY_SAMPLE_LIMIT} entries"
    except Exception as exc:
        result["note"] = f"zip inventory failed: {str(exc)[:300]}"
    return result


def _write_csv(path: Path, fieldnames: list[str], rows: Iterable[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow(row)


def _recommend_group(class_counts: Counter[str], project_count: int, generated_bytes: int, total_bytes: int) -> tuple[str, str]:
    if project_count:
        return "PROJECTS_REVIEW", "high"
    if not class_counts:
        return "MANUAL_REVIEW", "low"
    role, count = class_counts.most_common(1)[0]
    total = sum(class_counts.values())
    dominance = count / max(1, total)
    mapping = {
        "ASSET": "ASSETS_REVIEW", "ARCHIVE": "ARCHIVES_REVIEW", "REFERENCE": "REFERENCE_REVIEW",
        "SOURCE": "SOURCE_SNIPPETS_REVIEW", "DOCUMENTATION": "DOCUMENTATION_REVIEW",
        "BINARY": "TOOLS_BINARIES_REVIEW", "DISK_IMAGE": "ARCHIVES_REVIEW", "DATA": "DATA_REVIEW",
        "BUILD_OUTPUT": "REBUILDABLE_CLEANUP_REVIEW", "CACHE": "REBUILDABLE_CLEANUP_REVIEW",
        "DEPENDENCY": "DEPENDENCY_REVIEW", "UNKNOWN": "MANUAL_REVIEW", "CONFIG": "PROJECT_OR_CONFIG_REVIEW",
        "CONTROL": "PROJECT_CONTROL_REVIEW", "HISTORY": "SOURCE_HISTORY_REVIEW",
    }
    bucket = mapping.get(role, "MANUAL_REVIEW")
    confidence = "high" if dominance >= 0.80 else ("medium" if dominance >= 0.55 else "low")
    if total_bytes and generated_bytes / total_bytes >= 0.70:
        bucket, confidence = "REBUILDABLE_CLEANUP_REVIEW", "high"
    return bucket, confidence


def scan_intake(
    project_root: Path | None = None,
    *,
    intake: Path | None = None,
    inspect_archives: bool = True,
    progress: Callable[[dict[str, Any]], None] | None = None,
    cancelled: Callable[[], bool] | None = None,
) -> dict[str, Any]:
    root = (intake or audit_root(project_root)).expanduser().resolve()
    if not root.is_dir():
        raise FileNotFoundError(f"Vault Intake folder does not exist: {root}")

    scan_id = _stamp() + "-" + hashlib.sha256(str(root).encode("utf-8", errors="replace")).hexdigest()[:8]
    started_utc = _utc()
    db = _connect(project_root)
    file_count = 0
    dir_count = 0
    byte_count = 0
    error_count = 0
    reused = 0
    hashed = 0
    role_counts: Counter[str] = Counter()
    subtype_counts: Counter[str] = Counter()
    ext_counts: Counter[str] = Counter()
    zone_counts: Counter[str] = Counter()
    risk_counts: Counter[str] = Counter()
    language_counts: Counter[str] = Counter()
    size_buckets: Counter[str] = Counter()
    age_buckets: Counter[str] = Counter()
    group_stats: dict[str, dict[str, Any]] = defaultdict(lambda: {
        "files": 0, "bytes": 0, "generatedBytes": 0, "roles": Counter(), "zones": Counter(), "projects": set(),
    })
    project_markers: dict[str, set[str]] = defaultdict(set)
    project_flags: dict[str, dict[str, bool]] = defaultdict(lambda: {
        "git": False, "pcc": False, "license": False, "readme": False, "credits": False,
    })
    archive_rows: list[dict[str, Any]] = []
    errors: list[dict[str, str]] = []

    def emit() -> None:
        if progress:
            progress({"files": file_count, "directories": dir_count, "bytes": byte_count, "errors": error_count, "reused": reused, "hashed": hashed})

    try:
        with db:
            db.execute(
                "INSERT OR REPLACE INTO scans(scan_id,root_path,started_utc,completed_utc,status,files,directories,bytes,errors,reused,hashed,summary_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                (scan_id, str(root), started_utc, "", "RUNNING", 0, 0, 0, 0, 0, 0, "{}"),
            )

        stack: list[Path] = [root]
        pending_rows: list[tuple[Any, ...]] = []
        while stack:
            if cancelled and cancelled():
                raise InterruptedError("Vault Intake audit cancelled by operator")
            current = stack.pop()
            dir_count += 1
            try:
                entries = list(os.scandir(current))
            except OSError as exc:
                error_count += 1
                if len(errors) < 1000:
                    errors.append({"path": str(current), "error": str(exc)[:500]})
                continue

            for entry in entries:
                path = Path(entry.path)
                try:
                    if entry.is_symlink():
                        continue
                    info = entry.stat(follow_symlinks=False)
                except OSError as exc:
                    error_count += 1
                    if len(errors) < 1000:
                        errors.append({"path": str(path), "error": str(exc)[:500]})
                    continue

                if statmod.S_ISDIR(info.st_mode):
                    stack.append(path)
                    continue
                if not statmod.S_ISREG(info.st_mode):
                    continue

                try:
                    rel = path.relative_to(root)
                except ValueError:
                    continue
                rel_text = rel.as_posix()
                size = int(info.st_size)
                mtime_ns = int(info.st_mtime_ns)
                ext = path.suffix.casefold()
                source_zone = _source_zone(rel)
                top_group = _top_group(rel)
                group = group_stats[top_group]

                cached = db.execute(
                    "SELECT size,mtime_ns,extension,mime,role,subtype,language,signature,source_zone,top_group,size_bucket,age_bucket,generated,risk,version_hint,quick_hash,full_sha256,note FROM files WHERE rel_path=?",
                    (rel_text,),
                ).fetchone()

                if cached and int(cached[0]) == size and int(cached[1]) == mtime_ns:
                    (
                        _size, _mtime, old_ext, mime, role, subtype, language, signature, old_zone, old_group,
                        size_bucket, age_bucket, generated_i, risk, version_hint, quick_hash, full_sha256, note,
                    ) = cached
                    generated = bool(generated_i)
                    source_zone = str(old_zone or source_zone)
                    top_group = str(old_group or top_group)
                    reused += 1
                else:
                    signature = _signature(path)
                    role, subtype, language, mime, generated, risk = _classify(rel, path, signature)
                    size_bucket = _size_bucket(size)
                    age_bucket = _age_bucket(mtime_ns)
                    version_hint = _version_hint(path)
                    quick_hash = _quick_hash(path, size)
                    full_sha256 = ""
                    note = ""
                    if size <= FULL_HASH_LIMIT:
                        try:
                            full_sha256 = _full_hash(path)
                            hashed += 1
                        except OSError as exc:
                            note = f"full hash failed: {str(exc)[:250]}"

                # Archive central-directory inventory is bounded and never extracts data.
                # Re-run it even when file metadata was cached so the archive report remains complete.
                if inspect_archives and role == "ARCHIVE" and (ext == ".zip" or signature == "ZIP"):
                    archive_rows.append(_inspect_zip(path, rel_text))

                age_bucket = _age_bucket(mtime_ns)
                marker_kind = _marker_kind(rel, path)
                if marker_kind:
                    project_root_rel = _project_root_for_marker(rel, path).as_posix()
                    project_markers[project_root_rel].add(marker_kind)
                    group["projects"].add(project_root_rel)

                leaf = path.name.casefold()
                parent_rel = rel.parent.as_posix()
                if leaf == ".git":
                    project_flags[parent_rel]["git"] = True
                if leaf in CONTROL_NAMES:
                    project_flags[parent_rel]["pcc"] = True
                if leaf in LICENSE_NAMES:
                    project_flags[parent_rel]["license"] = True
                if leaf in README_NAMES:
                    project_flags[parent_rel]["readme"] = True
                if leaf in CREDIT_NAMES:
                    project_flags[parent_rel]["credits"] = True

                file_count += 1
                byte_count += size
                role_counts[str(role)] += 1
                subtype_counts[str(subtype)] += 1
                if ext:
                    ext_counts[ext] += 1
                zone_counts[source_zone] += 1
                risk_counts[str(risk)] += 1
                if language:
                    language_counts[str(language)] += 1
                size_buckets[str(size_bucket)] += 1
                age_buckets[str(age_bucket)] += 1
                group["files"] += 1
                group["bytes"] += size
                group["roles"][str(role)] += 1
                group["zones"][source_zone] += 1
                if generated:
                    group["generatedBytes"] += size

                pending_rows.append((
                    rel_text, size, mtime_ns, ext, str(mime or ""), str(role), str(subtype), str(language or ""),
                    str(signature or ""), source_zone, top_group, str(size_bucket), str(age_bucket), 1 if generated else 0,
                    str(risk), str(version_hint), str(quick_hash or ""), str(full_sha256 or ""), str(note), scan_id,
                ))
                if len(pending_rows) >= 500:
                    with db:
                        db.executemany(
                            "INSERT INTO files(rel_path,size,mtime_ns,extension,mime,role,subtype,language,signature,source_zone,top_group,size_bucket,age_bucket,generated,risk,version_hint,quick_hash,full_sha256,note,last_scan_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) "
                            "ON CONFLICT(rel_path) DO UPDATE SET size=excluded.size,mtime_ns=excluded.mtime_ns,extension=excluded.extension,mime=excluded.mime,role=excluded.role,subtype=excluded.subtype,language=excluded.language,signature=excluded.signature,source_zone=excluded.source_zone,top_group=excluded.top_group,size_bucket=excluded.size_bucket,age_bucket=excluded.age_bucket,generated=excluded.generated,risk=excluded.risk,version_hint=excluded.version_hint,quick_hash=excluded.quick_hash,full_sha256=excluded.full_sha256,note=excluded.note,last_scan_id=excluded.last_scan_id",
                            pending_rows,
                        )
                    pending_rows.clear()
                if file_count % 250 == 0:
                    emit()

        if pending_rows:
            with db:
                db.executemany(
                    "INSERT INTO files(rel_path,size,mtime_ns,extension,mime,role,subtype,language,signature,source_zone,top_group,size_bucket,age_bucket,generated,risk,version_hint,quick_hash,full_sha256,note,last_scan_id) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) "
                    "ON CONFLICT(rel_path) DO UPDATE SET size=excluded.size,mtime_ns=excluded.mtime_ns,extension=excluded.extension,mime=excluded.mime,role=excluded.role,subtype=excluded.subtype,language=excluded.language,signature=excluded.signature,source_zone=excluded.source_zone,top_group=excluded.top_group,size_bucket=excluded.size_bucket,age_bucket=excluded.age_bucket,generated=excluded.generated,risk=excluded.risk,version_hint=excluded.version_hint,quick_hash=excluded.quick_hash,full_sha256=excluded.full_sha256,note=excluded.note,last_scan_id=excluded.last_scan_id",
                    pending_rows,
                )

        # Drop stale catalog rows only after a complete scan.
        with db:
            db.execute("DELETE FROM files WHERE last_scan_id<>?", (scan_id,))
            db.execute("DELETE FROM project_candidates")
            db.execute("DELETE FROM archives")

            for root_rel, markers in sorted(project_markers.items()):
                root_path = root if root_rel in {"", "."} else root / Path(root_rel)
                flags = project_flags[root_rel]
                try:
                    child_names = {child.name.casefold() for child in root_path.iterdir()} if root_path.is_dir() else set()
                except OSError:
                    child_names = set()
                flags["git"] = flags["git"] or ".git" in child_names
                flags["pcc"] = flags["pcc"] or bool(child_names & CONTROL_NAMES)
                flags["license"] = flags["license"] or bool(child_names & LICENSE_NAMES)
                flags["readme"] = flags["readme"] or bool(child_names & README_NAMES)
                flags["credits"] = flags["credits"] or bool(child_names & CREDIT_NAMES)
                engine_hint = " + ".join(sorted(markers))
                db.execute(
                    "INSERT INTO project_candidates(root_rel,markers_json,engine_hint,has_git,has_pcc,has_license,has_readme,has_credits,last_scan_id) VALUES(?,?,?,?,?,?,?,?,?)",
                    (root_rel, json.dumps(sorted(markers)), engine_hint, int(flags["git"]), int(flags["pcc"]), int(flags["license"]), int(flags["readme"]), int(flags["credits"]), scan_id),
                )
            for row in archive_rows:
                db.execute(
                    "INSERT OR REPLACE INTO archives(rel_path,format,entry_count,sampled_entries,uncompressed_bytes,compressed_bytes,project_markers_json,suspicious,note,last_scan_id) VALUES(?,?,?,?,?,?,?,?,?,?)",
                    (row["relPath"], row["format"], row["entryCount"], row["sampledEntries"], row["uncompressedBytes"], row["compressedBytes"], json.dumps(row["projectMarkers"]), int(bool(row["suspicious"])), row["note"], scan_id),
                )

        exact_dupe_groups = int(db.execute(
            "SELECT COUNT(*) FROM (SELECT full_sha256 FROM files WHERE full_sha256<>'' GROUP BY full_sha256 HAVING COUNT(*)>1)"
        ).fetchone()[0])
        sampled_dupe_groups = int(db.execute(
            "SELECT COUNT(*) FROM (SELECT size,quick_hash FROM files WHERE quick_hash<>'' AND full_sha256='' GROUP BY size,quick_hash HAVING COUNT(*)>1)"
        ).fetchone()[0])
        unknown_count = int(db.execute("SELECT COUNT(*) FROM files WHERE role='UNKNOWN'").fetchone()[0])
        project_count = int(db.execute("SELECT COUNT(*) FROM project_candidates").fetchone()[0])
        archive_count = int(db.execute("SELECT COUNT(*) FROM archives").fetchone()[0])
        suspicious_archives = int(db.execute("SELECT COUNT(*) FROM archives WHERE suspicious=1").fetchone()[0])

        organization_groups: list[dict[str, Any]] = []
        for group_name, stats in sorted(group_stats.items(), key=lambda item: (-int(item[1]["bytes"]), item[0].casefold())):
            bucket, confidence = _recommend_group(stats["roles"], len(stats["projects"]), int(stats["generatedBytes"]), int(stats["bytes"]))
            organization_groups.append({
                "group": group_name,
                "files": int(stats["files"]),
                "bytes": int(stats["bytes"]),
                "humanBytes": _human_bytes(int(stats["bytes"])),
                "dominantRoles": dict(stats["roles"].most_common(6)),
                "zones": dict(stats["zones"]),
                "projectCandidates": sorted(stats["projects"]),
                "recommendedBucket": bucket,
                "confidence": confidence,
                "action": "REVIEW_ONLY_NO_MOVE",
            })

        summary = {
            "schema": DB_SCHEMA,
            "version": INTAKE_AUDIT_VERSION,
            "scanId": scan_id,
            "root": str(root),
            "vaultRoot": str(resolve_vault_root(project_root)),
            "startedUtc": started_utc,
            "completedUtc": _utc(),
            "status": "PASS",
            "files": file_count,
            "directories": dir_count,
            "bytes": byte_count,
            "humanBytes": _human_bytes(byte_count),
            "errors": error_count,
            "reusedMetadata": reused,
            "fullHashedFiles": hashed,
            "roleCounts": dict(role_counts.most_common()),
            "subtypeCounts": dict(subtype_counts.most_common()),
            "extensionCounts": dict(ext_counts.most_common()),
            "zoneCounts": dict(zone_counts.most_common()),
            "riskCounts": dict(risk_counts.most_common()),
            "languageCounts": dict(language_counts.most_common()),
            "sizeBuckets": dict(size_buckets.most_common()),
            "ageBuckets": dict(age_buckets.most_common()),
            "projectCandidates": project_count,
            "archiveInventories": archive_count,
            "suspiciousArchives": suspicious_archives,
            "unknownFiles": unknown_count,
            "exactDuplicateGroups": exact_dupe_groups,
            "sampledDuplicateCandidateGroups": sampled_dupe_groups,
            "organizationGroups": organization_groups,
            "safety": {
                "sourceMutation": False,
                "moves": False,
                "deletes": False,
                "archiveExtraction": False,
                "symlinkTraversal": False,
                "largeFileHashing": "sampled only above 8 MiB",
            },
        }
        with db:
            db.execute(
                "UPDATE scans SET completed_utc=?,status='PASS',files=?,directories=?,bytes=?,errors=?,reused=?,hashed=?,summary_json=? WHERE scan_id=?",
                (summary["completedUtc"], file_count, dir_count, byte_count, error_count, reused, hashed, json.dumps(summary, separators=(",", ":")), scan_id),
            )
        latest_summary_path(project_root).write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
        _write_local_reports(project_root, summary, errors)
        emit()
        return summary
    except Exception as exc:
        with db:
            db.execute(
                "UPDATE scans SET completed_utc=?,status='CANCELLED' WHERE scan_id=?",
                (_utc(), scan_id),
            )
        raise
    finally:
        db.close()


def _write_local_reports(project_root: Path | None, summary: dict[str, Any], errors: list[dict[str, str]]) -> None:
    out = audit_dir(project_root)
    db = _connect(project_root)
    try:
        projects = [
            {
                "root": r[0], "markers": "; ".join(json.loads(r[1])), "engineHint": r[2],
                "git": bool(r[3]), "pcc": bool(r[4]), "license": bool(r[5]), "readme": bool(r[6]), "credits": bool(r[7]),
            }
            for r in db.execute("SELECT root_rel,markers_json,engine_hint,has_git,has_pcc,has_license,has_readme,has_credits FROM project_candidates ORDER BY root_rel")
        ]
        exact_duplicates = [
            {"sha256": r[0], "bytes": int(r[1]), "count": int(r[2]), "paths": r[3]}
            for r in db.execute(
                "SELECT full_sha256,MAX(size),COUNT(*),GROUP_CONCAT(rel_path,' | ') FROM files WHERE full_sha256<>'' GROUP BY full_sha256 HAVING COUNT(*)>1 ORDER BY COUNT(*) DESC,MAX(size) DESC"
            )
        ]
        sampled_duplicates = [
            {"sampleHash": r[1], "bytes": int(r[0]), "count": int(r[2]), "paths": r[3], "confidence": "candidate_requires_full_hash"}
            for r in db.execute(
                "SELECT size,quick_hash,COUNT(*),GROUP_CONCAT(rel_path,' | ') FROM files WHERE quick_hash<>'' AND full_sha256='' GROUP BY size,quick_hash HAVING COUNT(*)>1 ORDER BY COUNT(*) DESC,size DESC"
            )
        ]
        archives = [
            {"path": r[0], "format": r[1], "entries": int(r[2]), "sampledEntries": int(r[3]), "uncompressedBytes": int(r[4]), "compressedBytes": int(r[5]), "projectMarkers": "; ".join(json.loads(r[6])), "suspicious": bool(r[7]), "note": r[8]}
            for r in db.execute("SELECT rel_path,format,entry_count,sampled_entries,uncompressed_bytes,compressed_bytes,project_markers_json,suspicious,note FROM archives ORDER BY suspicious DESC,uncompressed_bytes DESC")
        ]
        unknowns = [
            {"path": r[0], "bytes": int(r[1]), "extension": r[2], "mime": r[3], "signature": r[4], "versionHint": r[5]}
            for r in db.execute("SELECT rel_path,size,extension,mime,signature,version_hint FROM files WHERE role='UNKNOWN' ORDER BY size DESC LIMIT 25000")
        ]
        large = [
            {"path": r[0], "bytes": int(r[1]), "role": r[2], "subtype": r[3], "risk": r[4]}
            for r in db.execute("SELECT rel_path,size,role,subtype,risk FROM files WHERE size>=? ORDER BY size DESC LIMIT 25000", (64 * 1024 * 1024,))
        ]

        _write_csv(out / "project-candidates.csv", ["root", "markers", "engineHint", "git", "pcc", "license", "readme", "credits"], projects)
        _write_csv(out / "exact-duplicates.csv", ["sha256", "bytes", "count", "paths"], exact_duplicates)
        _write_csv(out / "sampled-duplicate-candidates.csv", ["sampleHash", "bytes", "count", "paths", "confidence"], sampled_duplicates)
        _write_csv(out / "archive-inventory.csv", ["path", "format", "entries", "sampledEntries", "uncompressedBytes", "compressedBytes", "projectMarkers", "suspicious", "note"], archives)
        _write_csv(out / "unknown-review.csv", ["path", "bytes", "extension", "mime", "signature", "versionHint"], unknowns)
        _write_csv(out / "large-files.csv", ["path", "bytes", "role", "subtype", "risk"], large)
        _write_csv(out / "scan-errors.csv", ["path", "error"], errors)
        _write_csv(out / "organization-groups.csv", ["group", "files", "bytes", "humanBytes", "dominantRoles", "zones", "projectCandidates", "recommendedBucket", "confidence", "action"], summary.get("organizationGroups") or [])

        manifest_path = out / "file-manifest.csv.gz"
        with gzip.open(manifest_path, "wt", newline="", encoding="utf-8") as fh:
            fields = ["path", "bytes", "mtimeNs", "extension", "mime", "role", "subtype", "language", "signature", "sourceZone", "topGroup", "sizeBucket", "ageBucket", "generated", "risk", "versionHint", "quickHash", "fullSha256", "note"]
            writer = csv.DictWriter(fh, fieldnames=fields)
            writer.writeheader()
            for r in db.execute("SELECT rel_path,size,mtime_ns,extension,mime,role,subtype,language,signature,source_zone,top_group,size_bucket,age_bucket,generated,risk,version_hint,quick_hash,full_sha256,note FROM files ORDER BY rel_path"):
                writer.writerow(dict(zip(fields, r)))
    finally:
        db.close()


def latest_summary(project_root: Path | None = None) -> dict[str, Any] | None:
    path = latest_summary_path(project_root)
    if not path.is_file():
        return None
    try:
        value = json.loads(path.read_text(encoding="utf-8-sig"))
        return value if isinstance(value, dict) else None
    except Exception:
        return None


def _sample_rows(db: sqlite3.Connection, query: str, args: tuple[Any, ...] = (), limit: int = CHAT_HANDOFF_LIMIT) -> list[dict[str, Any]]:
    cursor = db.execute(query + f" LIMIT {int(limit)}", args)
    columns = [item[0] for item in cursor.description]
    return [dict(zip(columns, row)) for row in cursor.fetchall()]


def create_handoff(project_root: Path | None = None) -> Path:
    summary = latest_summary(project_root)
    if not summary:
        raise RuntimeError("No Vault Intake audit exists yet. Run Audit Intake first.")
    out = audit_dir(project_root)
    handoffs = out / "handoffs"
    handoffs.mkdir(parents=True, exist_ok=True)
    stamp = _stamp()
    staging = handoffs / f"Intake_Audit_Handoff_{stamp}"
    staging.mkdir(parents=True, exist_ok=True)
    db = _connect(project_root)
    try:
        projects = _sample_rows(db, "SELECT root_rel AS root,engine_hint AS engineHint,has_git AS git,has_pcc AS pcc,has_license AS license,has_readme AS readme,has_credits AS credits FROM project_candidates ORDER BY root_rel", limit=10000)
        exact_dupes = _sample_rows(db, "SELECT full_sha256 AS sha256,MAX(size) AS bytes,COUNT(*) AS count,GROUP_CONCAT(rel_path,' | ') AS paths FROM files WHERE full_sha256<>'' GROUP BY full_sha256 HAVING COUNT(*)>1 ORDER BY COUNT(*) DESC,MAX(size) DESC", limit=CHAT_HANDOFF_LIMIT)
        sampled_dupes = _sample_rows(db, "SELECT size AS bytes,quick_hash AS sampleHash,COUNT(*) AS count,GROUP_CONCAT(rel_path,' | ') AS paths FROM files WHERE quick_hash<>'' AND full_sha256='' GROUP BY size,quick_hash HAVING COUNT(*)>1 ORDER BY COUNT(*) DESC,size DESC", limit=CHAT_HANDOFF_LIMIT)
        unknowns = _sample_rows(db, "SELECT rel_path AS path,size AS bytes,extension,mime,signature,version_hint AS versionHint FROM files WHERE role='UNKNOWN' ORDER BY size DESC", limit=CHAT_HANDOFF_LIMIT)
        archives = _sample_rows(db, "SELECT rel_path AS path,format,entry_count AS entries,uncompressed_bytes AS uncompressedBytes,compressed_bytes AS compressedBytes,project_markers_json AS projectMarkers,suspicious,note FROM archives ORDER BY suspicious DESC,uncompressed_bytes DESC", limit=CHAT_HANDOFF_LIMIT)
        risky = _sample_rows(db, "SELECT rel_path AS path,size AS bytes,role,subtype,risk,signature FROM files WHERE risk IN ('executable_review','archive_review','third_party_review') ORDER BY size DESC", limit=CHAT_HANDOFF_LIMIT)
        versions = _sample_rows(db, "SELECT rel_path AS path,size AS bytes,role,version_hint AS versionHint FROM files WHERE version_hint<>'' ORDER BY size DESC", limit=CHAT_HANDOFF_LIMIT)
    finally:
        db.close()

    (staging / "intake-summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    (staging / "project-candidates.json").write_text(json.dumps(projects, indent=2) + "\n", encoding="utf-8")
    (staging / "exact-duplicates.json").write_text(json.dumps(exact_dupes, indent=2) + "\n", encoding="utf-8")
    (staging / "sampled-duplicate-candidates.json").write_text(json.dumps(sampled_dupes, indent=2) + "\n", encoding="utf-8")
    (staging / "unknown-review.json").write_text(json.dumps(unknowns, indent=2) + "\n", encoding="utf-8")
    (staging / "archive-review.json").write_text(json.dumps(archives, indent=2) + "\n", encoding="utf-8")
    (staging / "risk-review.json").write_text(json.dumps(risky, indent=2) + "\n", encoding="utf-8")
    (staging / "version-lineage-candidates.json").write_text(json.dumps(versions, indent=2) + "\n", encoding="utf-8")

    lines = [
        "# Cortex Vault Intake Audit Handoff",
        "",
        f"- Scan: `{summary.get('scanId')}`",
        f"- Intake root: `{summary.get('root')}`",
        f"- Files: **{summary.get('files', 0):,}**",
        f"- Size: **{summary.get('humanBytes', '0 B')}**",
        f"- Project candidates: **{summary.get('projectCandidates', 0):,}**",
        f"- Exact duplicate groups: **{summary.get('exactDuplicateGroups', 0):,}**",
        f"- Sampled large-file duplicate candidates: **{summary.get('sampledDuplicateCandidateGroups', 0):,}**",
        f"- Unknown files: **{summary.get('unknownFiles', 0):,}**",
        f"- Archive inventories: **{summary.get('archiveInventories', 0):,}**",
        f"- Suspicious archives: **{summary.get('suspiciousArchives', 0):,}**",
        f"- Scan errors: **{summary.get('errors', 0):,}**",
        "",
        "## Safety",
        "This audit is inventory-only. It does not move, rename, delete, extract, execute, or reorganize intake content.",
        "Large files use sampled hashes unless they are <= 8 MiB. Sampled duplicate matches are candidates only and must be full-hash verified before any deduplication decision.",
        "",
        "## Classification totals",
    ]
    for key, value in (summary.get("roleCounts") or {}).items():
        lines.append(f"- {key}: {int(value):,}")
    lines += ["", "## Top-level organization proposals"]
    for item in (summary.get("organizationGroups") or [])[:200]:
        lines.append(
            f"- `{item.get('group')}` — {item.get('humanBytes')} / {item.get('files', 0):,} files — "
            f"**{item.get('recommendedBucket')}** ({item.get('confidence')})"
        )
    lines += [
        "",
        "## Planning rule",
        "No source-moving or destructive organization should occur from this handoff alone. Use it to review classifications, project candidates, duplicate candidates, lineage/version candidates, provenance gaps, and proposed destination buckets first.",
    ]
    (staging / "HANDOFF.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

    zip_path = handoffs / f"Cortex_Vault_Intake_Audit_Handoff_{stamp}.zip"
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for path in sorted(staging.rglob("*")):
            if path.is_file():
                zf.write(path, path.relative_to(staging).as_posix())
    latest_handoff_path(project_root).write_text(str(zip_path) + "\n", encoding="utf-8")
    return zip_path


def status(project_root: Path | None = None) -> dict[str, Any]:
    summary = latest_summary(project_root)
    return {
        "schema": "pcc.vault_intake_status.v1",
        "version": INTAKE_AUDIT_VERSION,
        "intakeRoot": str(audit_root(project_root)),
        "catalogDir": str(audit_dir(project_root)),
        "database": str(database_path(project_root)),
        "summary": summary,
        "latestHandoff": latest_handoff_path(project_root).read_text(encoding="utf-8-sig").strip() if latest_handoff_path(project_root).is_file() else "",
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Read-only Cortex Vault Intake classification and planning audit")
    parser.add_argument("action", choices=["status", "scan", "handoff"])
    parser.add_argument("--root", help="Cortex/project root used only to resolve Vault authority")
    parser.add_argument("--intake-root", help="Optional exact Intake root override")
    parser.add_argument("--no-archive-inventory", action="store_true")
    parser.add_argument("--json", action="store_true")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    project_root = Path(args.root).expanduser().resolve() if args.root else None
    intake = Path(args.intake_root).expanduser().resolve() if args.intake_root else None
    try:
        if args.action == "scan":
            payload = scan_intake(project_root, intake=intake, inspect_archives=not args.no_archive_inventory)
            print(json.dumps(payload, indent=2))
            return 0
        if args.action == "handoff":
            path = create_handoff(project_root)
            print(f"INTAKE_HANDOFF={path}")
            return 0
        payload = status(project_root)
        print(json.dumps(payload, indent=2) if args.json else (
            f"Vault Intake: {payload['intakeRoot']}\n"
            f"Catalog: {payload['catalogDir']}\n"
            f"Files: {((payload.get('summary') or {}).get('files') or 0)}\n"
            f"Latest handoff: {payload.get('latestHandoff') or 'none'}"
        ))
        return 0
    except InterruptedError as exc:
        print(f"[CANCELLED] {exc}")
        return 130
    except Exception as exc:
        print(f"[FAIL] Vault Intake audit: {exc}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
