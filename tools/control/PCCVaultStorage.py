#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from PCCStoragePaths import (
    STORAGE_PATHS_VERSION,
    _runtime_root,
    dependency_compatibility,
    dependency_paths,
    legacy_project_key,
    object_store_dir,
    project_key,
    project_vault_dir,
    resolve_vault_root,
    storage_policy,
)
from PCCVaultCatalog import classify_path

STORAGE_VERSION = "PCC-VAULT-STORAGE-0.5"
MIRROR_SCHEMA = "pcc.project_mirror.v1"

# Rebuildable/reacquirable trees are represented by dependency/cache metadata,
# never copied into every project mirror.
EXCLUDED_CLASSES = {"BUILD_OUTPUT", "CACHE", "DEPENDENCY", "HISTORY"}
EXCLUDED_REL_PREFIXES = {
    ".git",
    ".hg",
    ".svn",
    ".cortex",
    ".project_control",
    "artifacts",
    "logs",
    "updates",
    "handoffs",
    "target",
    "node_modules",
    ".gradle",
    ".cargo",
    "build",
    "builds",
    "dist",
    "out",
    "bin",
    "obj",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
}


def _utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def _stamp() -> str:
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def _atomic_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def _migrate_legacy_project_namespaces(root: Path) -> None:
    """Move pre-portability absolute-path namespaces to stable drive-relative keys.

    This is a same-volume rename only. It preserves existing certified mirrors and
    Cargo build output instead of leaving duplicate data behind when portable
    identity becomes active. Existing destination data always wins and is never
    overwritten automatically.
    """
    vault = resolve_vault_root(root)
    old_key = legacy_project_key(root)
    new_key = project_key(root)
    if old_key == new_key:
        return
    pairs = [
        (vault / "projects" / old_key, vault / "projects" / new_key),
        (vault / "shared" / "build" / "rust" / "targets" / old_key,
         vault / "shared" / "build" / "rust" / "targets" / new_key),
    ]
    for old, new in pairs:
        if not old.exists() or new.exists():
            continue
        new.parent.mkdir(parents=True, exist_ok=True)
        try:
            old.replace(new)
        except OSError:
            # Fail closed on migration; normal directory provisioning can continue
            # without deleting either side. A later storage review can reconcile it.
            pass


def ensure_layout(root: Path) -> dict[str, str]:
    root = root.expanduser().resolve()
    _migrate_legacy_project_namespaces(root)
    paths = dependency_paths(root)
    required = [
        resolve_vault_root(root),
        project_vault_dir(root),
        object_store_dir(root),
        project_vault_dir(root) / "snapshots",
        project_vault_dir(root) / "git",
        project_vault_dir(root) / "metadata",
    ]
    required.extend(p for k, p in paths.items() if k not in {"vault_root", "project_vault"})
    for path in required:
        path.mkdir(parents=True, exist_ok=True)
    return {key: str(value) for key, value in paths.items()}


def dependency_environment(root: Path) -> dict[str, str]:
    """Environment overlay for project commands executed through PCC.

    The paths are shared machine-wide where safe.  Rust build outputs remain
    project-namespaced; if sccache is installed it becomes the safe cross-project
    compiled dependency cache.
    """
    paths = dependency_paths(root)
    vault = paths["vault_root"]
    shared = vault / "shared"
    toolchains = shared / "toolchains"
    rustup_home = toolchains / "rust" / "rustup-home"
    env = {
        "CORTEX_VAULT_ROOT": str(vault),
        "PCC_VAULT_ROOT": str(vault),
        "CORTEX_LIBRARY_ROOT": str(vault),
        "CORTEX_PORTABLE_VOLUME_ROOT": str(vault),
        "CORTEX_HOME": str(vault / ".cortex" / "home"),
        "CORTEX_RUNTIME_ROOT": str(_runtime_root()),
        "CORTEX_PROJECTS_ROOT": str(vault / "Source"),
        "CORTEX_MODELS_ROOT": str(vault / "Models"),
        "CORTEX_LOCAL_GIT_ROOT": str(vault / "Cortex" / "Git"),
        "CORTEX_STATE_MODE": "portable",
        "CARGO_HOME": str(paths["cargo_home"]),
        "RUSTUP_HOME": str(rustup_home),
        "CARGO_TARGET_DIR": str(paths["cargo_target_dir"]),
        "SCCACHE_DIR": str(paths["sccache_dir"]),
        "npm_config_cache": str(paths["npm_cache"]),
        "PIP_CACHE_DIR": str(paths["pip_cache"]),
        "GRADLE_USER_HOME": str(paths["gradle_home"]),
        "NUGET_PACKAGES": str(paths["nuget_packages"]),
        "VCPKG_DOWNLOADS": str(paths["vcpkg_downloads"]),
        "VCPKG_DEFAULT_BINARY_CACHE": str(paths["vcpkg_binary_cache"]),
    }

    # Every project operation must see the same Vault-owned toolchains, not only
    # the Cortex PCC process that happened to launch it.  This is especially
    # important for native project PCC wrappers such as PCC.cmd that invoke
    # `python`/`git`/`cargo` by name.  Preserve the inherited PATH (including
    # activated MSVC/Windows SDK directories) and prepend only verified shared
    # toolchain directories.
    path_entries: list[str] = []

    def add_path(path: Path) -> None:
        if path.is_dir():
            value = str(path)
            if os.path.normcase(value) not in {os.path.normcase(item) for item in path_entries}:
                path_entries.append(value)

    python_candidates: list[Path] = []
    # The resolved Vault is authoritative for portable/shared toolchains.  Do
    # not let a stale parent-process CORTEX_PYTHON_EXE from another Vault
    # outrank the toolchain that belongs to this operation's resolved Vault.
    python_root = toolchains / "python"
    if python_root.is_dir():
        python_candidates.extend(sorted(python_root.glob("*/python.exe"), reverse=True))
    configured_python = str(os.environ.get("CORTEX_PYTHON_EXE") or "").strip()
    if configured_python:
        python_candidates.append(Path(configured_python))
    try:
        current_python = Path(sys.executable)
        if current_python.is_file():
            python_candidates.append(current_python)
    except Exception:
        pass
    python_exe = next((item for item in python_candidates if item.is_file()), None)
    if python_exe is not None:
        env["CORTEX_PYTHON_EXE"] = str(python_exe)
        add_path(python_exe.parent)
        add_path(python_exe.parent / "Scripts")

    git_candidates = [
        toolchains / "git" / "mingit" / "cmd" / "git.exe",
        toolchains / "git" / "mingit" / "bin" / "git.exe",
    ]
    git_exe = next((item for item in git_candidates if item.is_file()), None)
    if git_exe is not None:
        env["CORTEX_GIT_EXE"] = str(git_exe)
        add_path(git_exe.parent)

    cargo_bin = paths["cargo_home"] / "bin"
    cargo_exe = cargo_bin / ("cargo.exe" if os.name == "nt" else "cargo")
    rustc_exe = cargo_bin / ("rustc.exe" if os.name == "nt" else "rustc")
    if cargo_bin.is_dir():
        add_path(cargo_bin)
    if cargo_exe.is_file():
        env["CORTEX_CARGO_EXE"] = str(cargo_exe)
    if rustc_exe.is_file():
        env["CORTEX_RUSTC_EXE"] = str(rustc_exe)

    inherited_path = str(os.environ.get("PATH") or "")
    if path_entries:
        env["PATH"] = os.pathsep.join([*path_entries, inherited_path] if inherited_path else path_entries)
    # Compiled Rust dependency reuse is safe only when the compiler invocation is
    # compatible. sccache keys the compiler/version/features/input content, so different
    # dependency versions naturally miss the cache instead of contaminating another project.
    sccache = shutil.which("sccache")
    if not sccache:
        home = paths.get("sccache_home")
        if home is not None:
            candidates = [home / "sccache.exe", home / "bin" / "sccache.exe", home / "sccache", home / "bin" / "sccache"]
            sccache = next((str(item) for item in candidates if item.is_file()), None)
    if sccache:
        env["RUSTC_WRAPPER"] = sccache
    compat = dependency_compatibility(root)
    env["CORTEX_DEPENDENCY_REUSE_POLICY"] = str(compat["policy"])
    env["CORTEX_DEPENDENCY_COMPATIBILITY"] = str(compat["fingerprint"])
    return env


def dependency_status(root: Path) -> dict[str, Any]:
    root = root.expanduser().resolve()
    paths = dependency_paths(root)
    vault = paths["vault_root"]
    try:
        usage = shutil.disk_usage(vault if vault.exists() else vault.parent)
        disk = {"total": usage.total, "used": usage.used, "free": usage.free}
    except Exception:
        disk = {"total": 0, "used": 0, "free": 0}
    legacy = {
        "cargoHome": str(Path.home() / ".cargo"),
        "pipCache": str(Path.home() / ".cache" / "pip"),
        "nugetPackages": str(Path.home() / ".nuget" / "packages"),
    }
    return {
        "schema": "pcc.shared_dependencies.v1",
        "version": STORAGE_VERSION,
        "pathVersion": STORAGE_PATHS_VERSION,
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "enabled": os.environ.get("CORTEX_SHARED_DEPENDENCIES", "1").strip().casefold() not in {"0", "false", "off", "no"},
        "vaultRoot": str(vault),
        "paths": {key: str(value) for key, value in paths.items()},
        "sccache": dependency_environment(root).get("RUSTC_WRAPPER"),
        "reusePolicy": dependency_compatibility(root),
        "ecosystems": {
            "rust": {
                "downloadSourceCache": str(paths["cargo_home"]),
                "compiledCache": str(paths["sccache_dir"]),
                "projectBuildOutput": str(paths["cargo_target_dir"]),
                "rule": "share exact-compatible compiler artifacts; isolate project outputs and incompatible versions/features",
            },
            "node": {"cache": str(paths["npm_cache"]), "rule": "share package tarballs by version/integrity; keep node_modules project-local"},
            "python": {"cache": str(paths["pip_cache"]), "rule": "share wheels/downloads by version/platform; keep environments project-local"},
            "gradle": {"cache": str(paths["gradle_home"]), "rule": "share module/artifact caches; keep project build directories local"},
            "nuget": {"cache": str(paths["nuget_packages"]), "rule": "share packages by id/version; keep project outputs local"},
            "vcpkg": {"downloads": str(paths["vcpkg_downloads"]), "binaryCache": str(paths["vcpkg_binary_cache"]), "rule": "share downloads/binaries by ABI; incompatible triplets/features remain distinct"},
        },
        "legacyCandidates": legacy,
        "disk": disk,
    }


def _read_latest(root: Path) -> dict[str, Any] | None:
    latest = project_vault_dir(root) / "snapshots" / "latest.json"
    if not latest.is_file():
        return None
    try:
        payload = json.loads(latest.read_text(encoding="utf-8-sig"))
    except Exception:
        return None
    return payload if isinstance(payload, dict) else None


def mirror_status(root: Path) -> dict[str, Any]:
    root = root.expanduser().resolve()
    latest = _read_latest(root)
    return {
        "schema": "pcc.project_mirror_status.v1",
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "vaultRoot": str(resolve_vault_root(root)),
        "projectVault": str(project_vault_dir(root)),
        "hasSnapshot": latest is not None,
        "latest": latest,
    }


def _is_link_like(path: Path) -> bool:
    """Reject symlinks and Windows reparse/junction entries from mirrors.

    Vault mirroring is a project-boundary operation. Following a link or reparse
    point could ingest data outside the registered project root and makes restore
    provenance ambiguous.
    """
    try:
        info = path.lstat()
    except OSError:
        return False
    if stat.S_ISLNK(info.st_mode):
        return True
    attrs = getattr(info, "st_file_attributes", 0)
    reparse = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attrs & reparse)


def _excluded(root: Path, path: Path, *, is_dir: bool) -> tuple[bool, str]:
    if _is_link_like(path):
        return True, "LINK"
    rel_path = path.relative_to(root)
    rel = rel_path.as_posix()
    first = rel.split("/", 1)[0].casefold()
    if first in EXCLUDED_REL_PREFIXES:
        return True, "OPERATIONAL"
    # Root transport residue is not governed project source. Keep archives that
    # intentionally live inside project content trees, but do not mirror root
    # patch/debug/rollup ZIPs or checksum sidecars.
    if len(rel_path.parts) == 1:
        folded = path.name.casefold()
        if folded.endswith((".zip", ".sha256", ".sha256.txt")) or folded.startswith("cortex_debugbundle_"):
            return True, "TRANSPORT"
    classification = classify_path(root, path, is_dir=is_dir)
    return classification in EXCLUDED_CLASSES, classification


def _object_path(root: Path, digest: str) -> Path:
    return object_store_dir(root) / digest[:2] / digest[2:4] / digest


def _store_object(root: Path, source: Path, digest: str) -> tuple[Path, bool]:
    target = _object_path(root, digest)
    if target.is_file() and target.stat().st_size == source.stat().st_size:
        # CAS identity is the digest, not the byte count. A same-size damaged
        # object must never be trusted or propagated into a certified mirror.
        if _sha256(target) == digest:
            return target, False
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix="vault-object-", dir=str(target.parent))
    os.close(fd)
    temp = Path(temp_name)
    try:
        shutil.copy2(source, temp)
        if _sha256(temp) != digest:
            raise RuntimeError(f"Vault object verification failed while copying {source}")
        os.replace(temp, target)
    finally:
        temp.unlink(missing_ok=True)
    return target, True


def _git_bundle(root: Path) -> dict[str, Any]:
    git = shutil.which("git")
    if not git or not (root / ".git").exists():
        return {"status": "SKIP", "reason": "git repository not initialized"}
    git_dir = project_vault_dir(root) / "git"
    git_dir.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix="project-", suffix=".bundle.tmp", dir=str(git_dir))
    os.close(fd)
    temp = Path(temp_name)
    try:
        cp = subprocess.run(
            [git, "-C", str(root), "bundle", "create", str(temp), "--all"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            timeout=300,
        )
        if cp.returncode != 0:
            return {"status": "WARN", "reason": (cp.stderr or cp.stdout).strip()[-2000:]}
        digest = _sha256(temp)
        obj, created = _store_object(root, temp, digest)
        meta = {
            "status": "PASS",
            "bytes": temp.stat().st_size,
            "sha256": digest,
            "object": obj.relative_to(resolve_vault_root(root)).as_posix(),
            "newObject": created,
            "createdUtc": _utc(),
        }
        _atomic_json(git_dir / "latest.json", meta)
        return meta
    finally:
        temp.unlink(missing_ok=True)


def mirror_project(
    root: Path,
    *,
    label: str = "working",
    source_fingerprint: str | None = None,
    source_path_count: int | None = None,
    progress: Callable[[dict[str, Any]], None] | None = None,
) -> dict[str, Any]:
    """Create an incremental content-addressed Vault mirror of a project.

    Durable project files are mirrored by SHA-256 into a global object store, so
    identical files across projects consume one object.  Build outputs,
    dependency trees, caches and VCS internals are excluded and represented by
    shared-store / Git-bundle metadata instead.
    """
    root = root.expanduser().resolve()
    if not root.is_dir():
        raise FileNotFoundError(root)
    ensure_layout(root)
    previous = _read_latest(root) or {}
    previous_by_path = {
        str(item.get("path")): item
        for item in (previous.get("files") or [])
        if isinstance(item, dict) and item.get("path")
    }
    files: list[dict[str, Any]] = []
    exclusions: list[dict[str, str]] = []
    mirrored_bytes = 0
    new_object_bytes = 0
    new_objects = 0
    reused_objects = 0
    reused_fingerprints = 0

    for current, dirs, names in os.walk(root):
        current_path = Path(current)
        kept: list[str] = []
        for name in dirs:
            path = current_path / name
            skip, classification = _excluded(root, path, is_dir=True)
            if skip:
                exclusions.append({"path": path.relative_to(root).as_posix(), "classification": classification})
            else:
                kept.append(name)
        dirs[:] = kept
        for name in names:
            path = current_path / name
            if _is_link_like(path):
                exclusions.append({"path": path.relative_to(root).as_posix(), "classification": "LINK"})
                continue
            try:
                file_stat = path.stat()
            except OSError:
                continue
            if not path.is_file():
                continue
            skip, classification = _excluded(root, path, is_dir=False)
            if skip:
                exclusions.append({"path": path.relative_to(root).as_posix(), "classification": classification})
                continue
            rel = path.relative_to(root).as_posix()
            old = previous_by_path.get(rel)
            digest = ""
            if (
                old
                and int(old.get("bytes") or -1) == int(file_stat.st_size)
                and int(old.get("mtimeNs") or -1) == int(file_stat.st_mtime_ns)
                and isinstance(old.get("sha256"), str)
                and len(str(old.get("sha256"))) == 64
            ):
                digest = str(old["sha256"])
                reused_fingerprints += 1
            else:
                digest = _sha256(path)
            obj, created = _store_object(root, path, digest)
            if created:
                new_objects += 1
                new_object_bytes += int(file_stat.st_size)
            else:
                reused_objects += 1
            mirrored_bytes += int(file_stat.st_size)
            files.append({
                "path": rel,
                "bytes": int(file_stat.st_size),
                "mtimeNs": int(file_stat.st_mtime_ns),
                "classification": classification,
                "sha256": digest,
                "object": obj.relative_to(resolve_vault_root(root)).as_posix(),
            })
            if progress and len(files) % 100 == 0:
                progress({"files": len(files), "bytes": mirrored_bytes, "newObjects": new_objects, "reusedObjects": reused_objects})

    files.sort(key=lambda item: str(item["path"]).casefold())
    snapshot_id = f"{_stamp()}-{hashlib.sha256((str(root)+_utc()).encode()).hexdigest()[:8]}"
    project_dir = project_vault_dir(root)
    bundle = _git_bundle(root)
    payload = {
        "schema": MIRROR_SCHEMA,
        "version": STORAGE_VERSION,
        "snapshotId": snapshot_id,
        "label": label,
        "createdUtc": _utc(),
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "projectName": root.name,
        "vaultRoot": str(resolve_vault_root(root)),
        "governedSource": {
            "fingerprint": source_fingerprint,
            "pathCount": source_path_count,
        } if source_fingerprint else None,
        "files": files,
        "excludedDirectories": sorted(exclusions, key=lambda item: item["path"].casefold()),
        "dependencyStore": dependency_status(root),
        "gitBundle": bundle,
        "stats": {
            "files": len(files),
            "mirroredBytes": mirrored_bytes,
            "newObjects": new_objects,
            "newObjectBytes": new_object_bytes,
            "reusedObjects": reused_objects,
            "reusedFingerprints": reused_fingerprints,
            "excludedDirectories": len(exclusions),
        },
    }
    snapshots = project_dir / "snapshots"
    snapshot_path = snapshots / f"snapshot-{snapshot_id}.json"
    _atomic_json(snapshot_path, payload)
    _atomic_json(snapshots / "latest.json", payload)
    _atomic_json(
        project_dir / "project.json",
        {
            "schema": "pcc.vault_project.v1",
            "projectKey": project_key(root),
            "name": root.name,
            "root": str(root),
            "vaultRoot": str(resolve_vault_root(root)),
            "latestSnapshotId": snapshot_id,
            "latestSnapshot": str(snapshot_path),
            "updatedUtc": payload["createdUtc"],
            "dependencyStore": payload["dependencyStore"]["paths"],
        },
    )
    return payload


def verify_latest_mirror(root: Path, *, deep_hash: bool = False) -> dict[str, Any]:
    root = root.expanduser().resolve()
    payload = _read_latest(root)
    if not payload:
        return {"schema": "pcc.project_mirror_verify.v1", "status": "FAIL", "error": "No project mirror snapshot exists"}
    missing: list[str] = []
    corrupt: list[str] = []
    checked = 0
    for item in payload.get("files") or []:
        if not isinstance(item, dict):
            continue
        digest = str(item.get("sha256") or "")
        if len(digest) != 64:
            corrupt.append(str(item.get("path") or "<unknown>"))
            continue
        obj = _object_path(root, digest)
        if not obj.is_file():
            missing.append(str(item.get("path") or "<unknown>"))
            continue
        if obj.stat().st_size != int(item.get("bytes") or -1):
            corrupt.append(str(item.get("path") or "<unknown>"))
            continue
        if deep_hash and _sha256(obj) != digest:
            corrupt.append(str(item.get("path") or "<unknown>"))
            continue
        checked += 1
    status = "PASS" if not missing and not corrupt else "FAIL"
    return {
        "schema": "pcc.project_mirror_verify.v1",
        "status": status,
        "snapshotId": payload.get("snapshotId"),
        "checkedObjects": checked,
        "missing": missing,
        "corrupt": corrupt,
        "deepHash": deep_hash,
        "projectVault": str(project_vault_dir(root)),
    }


def _tree_size(path: Path) -> int:
    total = 0
    try:
        if path.is_file():
            return int(path.stat().st_size)
        for current, dirs, files in os.walk(path):
            dirs[:] = [d for d in dirs if not (Path(current) / d).is_symlink()]
            for name in files:
                p = Path(current) / name
                try:
                    if not p.is_symlink():
                        total += int(p.stat().st_size)
                except OSError:
                    pass
    except OSError:
        pass
    return total


def reclaim_plan(root: Path) -> dict[str, Any]:
    """Read-only estimate of project-local caches made redundant by Vault paths.

    This never deletes or moves anything.  It is intentionally a plan so the
    user can certify the shared path first, then perform a later governed
    cleanup pass with rollback/quarantine semantics.
    """
    root = root.expanduser().resolve()
    candidates: list[dict[str, Any]] = []
    specs = [
        ("target", "Rust build output; future PCC Cargo output is Vault-namespaced"),
        ("build", "Generated build tree"),
        ("builds", "Generated build tree"),
        ("dist", "Generated distribution output"),
        ("out", "Generated output"),
        ("bin", "Generated binary output"),
        ("obj", "Generated object output"),
        ("node_modules", "Reacquirable Node dependency tree; shared npm download cache is in Vault"),
        (".gradle/caches", "Gradle cache; shared Gradle home is in Vault"),
        (".pytest_cache", "Python test cache"),
        (".mypy_cache", "Python analysis cache"),
        (".ruff_cache", "Python analysis cache"),
    ]
    for rel, reason in specs:
        path = root / Path(rel)
        if not path.exists():
            continue
        candidates.append({
            "path": str(path),
            "relativePath": Path(rel).as_posix(),
            "bytes": _tree_size(path),
            "reason": reason,
        })
    total = sum(int(item["bytes"]) for item in candidates)
    return {
        "schema": "pcc.storage_reclaim_plan.v1",
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "createdUtc": _utc(),
        "deleteApplied": False,
        "candidates": candidates,
        "candidateCount": len(candidates),
        "reclaimBytes": total,
        "rule": "dry-run only; certify shared paths before any governed cleanup",
    }



def _json_read(path: Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception:
        return None
    return value if isinstance(value, dict) else None


def certify_snapshot(root: Path, snapshot_id: str | None = None, *, gate: str = "full", source_marker: str = "GREEN") -> dict[str, Any]:
    """Record that a mirror snapshot completed a governed certification gate.

    Snapshot manifests stay immutable; certification is a separate record so a
    failed mark-green cannot accidentally turn a pre-GREEN mirror into a
    certified recovery point.
    """
    root = root.expanduser().resolve()
    latest = _read_latest(root)
    if not latest:
        raise RuntimeError("Cannot certify Vault snapshot: no mirror exists")
    sid = str(snapshot_id or latest.get("snapshotId") or "").strip()
    if not sid:
        raise RuntimeError("Cannot certify Vault snapshot: snapshot id missing")
    snapshot_path = project_vault_dir(root) / "snapshots" / f"snapshot-{sid}.json"
    if not snapshot_path.is_file():
        raise RuntimeError(f"Cannot certify Vault snapshot: manifest missing: {snapshot_path}")
    green_marker_path = root / ".cortex" / "last-green-quality-gate.json"
    green_marker = _json_read(green_marker_path) if green_marker_path.is_file() else None
    snapshot_payload = _json_read(snapshot_path)
    if not snapshot_payload:
        raise RuntimeError(f"Cannot certify Vault snapshot: invalid manifest: {snapshot_path}")
    mirror_fingerprint = str(((snapshot_payload.get("governedSource") or {}).get("fingerprint") or "")).strip()
    marker_fingerprint = str((green_marker or {}).get("fingerprint") or "").strip()
    if mirror_fingerprint and mirror_fingerprint != marker_fingerprint:
        raise RuntimeError(
            "Cannot certify Vault snapshot: mirror/source fingerprint does not match GREEN marker "
            f"({mirror_fingerprint} != {marker_fingerprint or '<missing>'})"
        )
    record = {
        "schema": "pcc.vault_snapshot_certification.v1",
        "createdUtc": _utc(),
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "snapshotId": sid,
        "gate": gate,
        "sourceMarker": source_marker,
        "snapshot": str(snapshot_path),
        "mirrorSourceFingerprint": mirror_fingerprint or None,
        "greenMarker": {
            "path": str(green_marker_path),
            "sha256": _sha256(green_marker_path) if green_marker_path.is_file() else None,
            "fingerprint": str((green_marker or {}).get("fingerprint") or "") or None,
            "gitHead": (green_marker or {}).get("gitHead"),
            "gitBranch": (green_marker or {}).get("gitBranch"),
        },
    }
    cert_dir = project_vault_dir(root) / "certifications"
    _atomic_json(cert_dir / f"{gate}-{sid}.json", record)
    _atomic_json(cert_dir / f"latest-{gate}.json", record)
    return record


def _certified_snapshot_ids(project_dir: Path) -> set[str]:
    values: set[str] = set()
    cert_dir = project_dir / "certifications"
    if not cert_dir.is_dir():
        return values
    for path in cert_dir.glob("*.json"):
        if path.name.startswith("latest-"):
            continue
        payload = _json_read(path)
        sid = str((payload or {}).get("snapshotId") or "").strip()
        if sid:
            values.add(sid)
    return values


def snapshot_retention_plan(root: Path) -> dict[str, Any]:
    """Plan metadata retention without changing project source or CAS objects."""
    root = root.expanduser().resolve()
    project_dir = project_vault_dir(root)
    snapshots_dir = project_dir / "snapshots"
    latest = _read_latest(root) or {}
    latest_id = str(latest.get("snapshotId") or "")
    policy = storage_policy(root)
    retention = ((policy.get("vault") or {}).get("snapshot_retention") or {}) if isinstance(policy, dict) else {}
    keep_certified = max(1, int(retention.get("certified_keep") or 20))
    keep_working = max(1, int(retention.get("working_keep") or 8))
    rows: list[dict[str, Any]] = []
    if snapshots_dir.is_dir():
        for path in snapshots_dir.glob("snapshot-*.json"):
            payload = _json_read(path) or {}
            sid = str(payload.get("snapshotId") or path.stem.removeprefix("snapshot-"))
            created = str(payload.get("createdUtc") or "")
            rows.append({
                "snapshotId": sid,
                "createdUtc": created,
                "label": str(payload.get("label") or ""),
                "path": str(path),
            })
    rows.sort(key=lambda row: (str(row.get("createdUtc") or ""), str(row.get("snapshotId") or "")), reverse=True)
    certified_ids = _certified_snapshot_ids(project_dir)
    certified = [row for row in rows if row["snapshotId"] in certified_ids]
    working = [row for row in rows if row["snapshotId"] not in certified_ids]
    keep_ids = {row["snapshotId"] for row in certified[:keep_certified]}
    keep_ids.update(row["snapshotId"] for row in working[:keep_working])
    if latest_id:
        keep_ids.add(latest_id)
    prune = [row for row in rows if row["snapshotId"] not in keep_ids]
    prune_ids = {row["snapshotId"] for row in prune}
    cert_prune: list[str] = []
    cert_dir = project_dir / "certifications"
    if cert_dir.is_dir():
        for path in cert_dir.glob("*.json"):
            if path.name.startswith("latest-"):
                continue
            payload = _json_read(path) or {}
            if str(payload.get("snapshotId") or "") in prune_ids:
                cert_prune.append(str(path))
    return {
        "schema": "pcc.vault_snapshot_retention_plan.v1",
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "createdUtc": _utc(),
        "latestSnapshotId": latest_id,
        "certifiedKeep": keep_certified,
        "workingKeep": keep_working,
        "snapshotCount": len(rows),
        "keepCount": len(rows) - len(prune),
        "pruneCount": len(prune),
        "prune": prune,
        "certificationRecordsToPrune": cert_prune,
        "applied": False,
    }


def apply_snapshot_retention(root: Path) -> dict[str, Any]:
    """Apply the metadata-only retention plan. CAS bytes are not removed here."""
    plan = snapshot_retention_plan(root)
    removed: list[str] = []
    for row in plan.get("prune") or []:
        path = Path(str(row.get("path") or ""))
        if path.is_file():
            path.unlink()
            removed.append(str(path))
    for raw in plan.get("certificationRecordsToPrune") or []:
        path = Path(str(raw))
        if path.is_file():
            path.unlink()
            removed.append(str(path))
    plan["applied"] = True
    plan["removedMetadata"] = removed
    return plan


def _extract_object_digests(value: Any, out: set[str]) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "object" and isinstance(item, str):
                match = re.search(r"(?:^|/)objects/sha256/[0-9a-fA-F]{2}/[0-9a-fA-F]{2}/([0-9a-fA-F]{64})$", item.replace("\\", "/"))
                if match:
                    out.add(match.group(1).lower())
            _extract_object_digests(item, out)
    elif isinstance(value, list):
        for item in value:
            _extract_object_digests(item, out)


def _referenced_object_digests(vault: Path) -> set[str]:
    refs: set[str] = set()
    projects = vault / "projects"
    if not projects.is_dir():
        return refs
    # Read every retained project metadata JSON, not only known snapshot names.
    # This makes GC conservative as new metadata surfaces are added later.
    for path in projects.rglob("*.json"):
        payload = _json_read(path)
        if payload is not None:
            _extract_object_digests(payload, refs)
    return refs


def _cas_objects(vault: Path) -> dict[str, Path]:
    base = vault / "objects" / "sha256"
    result: dict[str, Path] = {}
    if not base.is_dir():
        return result
    for path in base.rglob("*"):
        if not path.is_file():
            continue
        name = path.name.lower()
        if len(name) == 64 and all(ch in "0123456789abcdef" for ch in name):
            result[name] = path
    return result


def cas_gc_plan(root: Path) -> dict[str, Any]:
    """Mark-and-sweep plan for globally unreferenced Vault CAS objects."""
    root = root.expanduser().resolve()
    vault = resolve_vault_root(root)
    refs = _referenced_object_digests(vault)
    objects = _cas_objects(vault)
    garbage: list[dict[str, Any]] = []
    for digest, path in objects.items():
        if digest in refs:
            continue
        try:
            size = int(path.stat().st_size)
        except OSError:
            size = 0
        garbage.append({
            "sha256": digest,
            "bytes": size,
            "path": str(path),
            "relativePath": path.relative_to(vault).as_posix(),
        })
    garbage.sort(key=lambda row: row["sha256"])
    return {
        "schema": "pcc.vault_cas_gc_plan.v1",
        "createdUtc": _utc(),
        "vaultRoot": str(vault),
        "referencedObjects": len(refs),
        "casObjects": len(objects),
        "garbageObjects": len(garbage),
        "garbageBytes": sum(int(row["bytes"]) for row in garbage),
        "garbage": garbage,
        "applied": False,
        "rule": "plan only; stage moves unreferenced objects to Vault quarantine before any purge",
    }


def stage_cas_gc(root: Path) -> dict[str, Any]:
    """Move unreferenced CAS objects into reversible Vault quarantine."""
    root = root.expanduser().resolve()
    plan = cas_gc_plan(root)
    vault = Path(str(plan["vaultRoot"]))
    txid = f"{_stamp()}-{hashlib.sha256(_utc().encode()).hexdigest()[:8]}"
    tx_dir = vault / "quarantine" / "cas-gc" / txid
    moved: list[dict[str, Any]] = []
    for row in plan.get("garbage") or []:
        source = Path(str(row["path"]))
        if not source.is_file():
            continue
        relative = Path(str(row["relativePath"]))
        target = tx_dir / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        os.replace(source, target)
        moved.append({**row, "quarantinePath": str(target)})
    transaction = {
        "schema": "pcc.vault_cas_gc_transaction.v1",
        "transactionId": txid,
        "createdUtc": _utc(),
        "vaultRoot": str(vault),
        "status": "STAGED",
        "objects": moved,
        "objectCount": len(moved),
        "bytes": sum(int(row["bytes"]) for row in moved),
    }
    _atomic_json(tx_dir / "transaction.json", transaction)
    maintenance = vault / "maintenance" / "cas-gc"
    _atomic_json(maintenance / f"transaction-{txid}.json", transaction)
    _atomic_json(maintenance / "latest.json", transaction)
    plan["applied"] = True
    plan["transaction"] = transaction
    return plan


def _gc_transaction(root: Path, transaction_id: str | None = None) -> tuple[Path, dict[str, Any]]:
    vault = resolve_vault_root(root)
    maintenance = vault / "maintenance" / "cas-gc"
    pointer = maintenance / (f"transaction-{transaction_id}.json" if transaction_id else "latest.json")
    payload = _json_read(pointer)
    if not payload:
        raise RuntimeError(f"CAS GC transaction not found: {pointer}")
    txid = str(payload.get("transactionId") or "")
    if not txid:
        raise RuntimeError("CAS GC transaction is missing transactionId")
    return vault / "quarantine" / "cas-gc" / txid, payload


def restore_cas_gc(root: Path, transaction_id: str | None = None) -> dict[str, Any]:
    root = root.expanduser().resolve()
    tx_dir, payload = _gc_transaction(root, transaction_id)
    vault = resolve_vault_root(root)
    restored = 0
    for row in payload.get("objects") or []:
        digest = str(row.get("sha256") or "")
        source = Path(str(row.get("quarantinePath") or ""))
        if not source.is_file() or len(digest) != 64:
            continue
        target = vault / str(row.get("relativePath") or "")
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            continue
        os.replace(source, target)
        restored += 1
    payload = {**payload, "status": "RESTORED", "restoredUtc": _utc(), "restoredObjects": restored}
    maintenance = vault / "maintenance" / "cas-gc"
    _atomic_json(maintenance / f"transaction-{payload['transactionId']}.json", payload)
    _atomic_json(maintenance / "latest.json", payload)
    return payload


def purge_cas_gc(root: Path, transaction_id: str | None = None) -> dict[str, Any]:
    root = root.expanduser().resolve()
    tx_dir, payload = _gc_transaction(root, transaction_id)
    if str(payload.get("status") or "").upper() != "STAGED":
        raise RuntimeError(f"CAS GC transaction is not staged: {payload.get('status')}")
    bytes_to_free = int(payload.get("bytes") or 0)
    count = int(payload.get("objectCount") or 0)
    shutil.rmtree(tx_dir, ignore_errors=False)
    payload = {**payload, "status": "PURGED", "purgedUtc": _utc(), "purgedObjects": count, "purgedBytes": bytes_to_free}
    vault = resolve_vault_root(root)
    maintenance = vault / "maintenance" / "cas-gc"
    _atomic_json(maintenance / f"transaction-{payload['transactionId']}.json", payload)
    _atomic_json(maintenance / "latest.json", payload)
    return payload


def _dir_stats(path: Path) -> dict[str, int]:
    files = 0
    total = 0
    if not path.exists():
        return {"files": 0, "bytes": 0}
    if path.is_file():
        try:
            return {"files": 1, "bytes": int(path.stat().st_size)}
        except OSError:
            return {"files": 0, "bytes": 0}
    for current, dirs, names in os.walk(path):
        dirs[:] = [name for name in dirs if not (Path(current) / name).is_symlink()]
        for name in names:
            item = Path(current) / name
            try:
                if not item.is_symlink():
                    files += 1
                    total += int(item.stat().st_size)
            except OSError:
                pass
    return {"files": files, "bytes": total}



def quick_storage_health(root: Path) -> dict[str, Any]:
    """Non-recursive, read-only Vault health overview for the GUI.

    Never enumerate build caches, CAS trees, snapshot directories, or source files.
    This is a limited metadata overview, not a substitute for Deep Health.
    """
    root = root.expanduser().resolve()
    vault = resolve_vault_root(root)
    try:
        usage = shutil.disk_usage(vault if vault.exists() else vault.parent)
        disk = {"total": usage.total, "used": usage.used, "free": usage.free}
    except OSError:
        disk = {"total": 0, "used": 0, "free": 0}
    paths = dependency_paths(root)
    stores = {key: {"path": str(path), "exists": path.is_dir()}
              for key, path in paths.items() if key not in {"vault_root", "project_vault"}}
    total = int(disk["total"])
    free = int(disk["free"])
    warnings = ["Vault volume has less than 10% free space"] if total and free / total < 0.10 else []
    return {
        "schema": "pcc.storage_health_quick.v1",
        "mode": "QUICK_METADATA_ONLY",
        "status": "WARN" if warnings else "SUMMARY",
        "projectRoot": str(root),
        "vaultRoot": str(vault),
        "disk": disk,
        "sharedStores": stores,
        "mirrorManifestPresent": (project_vault_dir(root) / "snapshots" / "latest.json").is_file(),
        "deepChecksPerformed": False,
        "warnings": warnings,
        "note": "CAS counts, garbage collection, snapshot verification and reclaim estimates were not assessed. Use Deep Health explicitly after pausing the inventory.",
    }


def storage_health(root: Path) -> dict[str, Any]:
    """Global Vault/storage accounting suitable for PCC UI and debug bundles."""
    root = root.expanduser().resolve()
    # Health must never provision/migrate directories as a side effect of inspection.
    deps = dependency_status(root)
    vault = Path(str(deps["vaultRoot"]))
    cas = _dir_stats(vault / "objects" / "sha256")
    shared: dict[str, dict[str, Any]] = {}
    for key, path in dependency_paths(root).items():
        if key in {"vault_root", "project_vault"}:
            continue
        shared[key] = {"path": str(path), **_dir_stats(path)}
    projects = vault / "projects"
    project_count = 0
    snapshot_count = 0
    logical_latest_bytes = 0
    latest_object_digests: set[str] = set()
    stale_projects: list[dict[str, str]] = []
    if projects.is_dir():
        for project_dir in projects.iterdir():
            if not project_dir.is_dir():
                continue
            project_meta = _json_read(project_dir / "project.json") if (project_dir / "project.json").is_file() else None
            if project_meta:
                project_count += 1
                source_root = Path(str(project_meta.get("root") or ""))
                if str(source_root) and not source_root.exists():
                    stale_projects.append({"projectKey": project_dir.name, "root": str(source_root), "name": str(project_meta.get("name") or project_dir.name)})
            snapshots = project_dir / "snapshots"
            if snapshots.is_dir():
                snapshot_count += len(list(snapshots.glob("snapshot-*.json")))
            latest = _json_read(snapshots / "latest.json") if snapshots.is_dir() else None
            if latest:
                logical_latest_bytes += int(((latest.get("stats") or {}).get("mirroredBytes") or 0))
                _extract_object_digests(latest, latest_object_digests)
    latest_unique_bytes = 0
    objects = _cas_objects(vault)
    for digest in latest_object_digests:
        path = objects.get(digest)
        if path is not None:
            try:
                latest_unique_bytes += int(path.stat().st_size)
            except OSError:
                pass
    gc = cas_gc_plan(root)
    reclaim = reclaim_plan(root)
    disk = deps.get("disk") or {}
    free = int(disk.get("free") or 0)
    total = int(disk.get("total") or 0)
    free_ratio = (free / total) if total else 0.0
    warnings: list[str] = []
    if total and free_ratio < 0.10:
        warnings.append("Vault volume has less than 10% free space")
    if int(gc.get("garbageBytes") or 0) > 0:
        warnings.append("Unreferenced CAS objects are eligible for governed GC")
    latest_verify = verify_latest_mirror(root, deep_hash=False) if _read_latest(root) else {"status": "NONE"}
    if latest_verify.get("status") == "FAIL":
        warnings.append("Latest project mirror has missing or corrupt CAS objects")
    return {
        "schema": "pcc.storage_health.v1",
        "createdUtc": _utc(),
        "status": "WARN" if warnings else "PASS",
        "projectRoot": str(root),
        "projectKey": project_key(root),
        "vaultRoot": str(vault),
        "disk": disk,
        "cas": cas,
        "sharedStores": shared,
        "projects": project_count,
        "snapshots": snapshot_count,
        "logicalLatestMirroredBytes": logical_latest_bytes,
        "latestUniqueObjectBytes": latest_unique_bytes,
        "vaultLogicalDedupeBytes": max(0, logical_latest_bytes - latest_unique_bytes),
        "staleProjects": stale_projects,
        "currentProjectReclaimBytes": int(reclaim.get("reclaimBytes") or 0),
        "gcGarbageBytes": int(gc.get("garbageBytes") or 0),
        "gcGarbageObjects": int(gc.get("garbageObjects") or 0),
        "latestMirrorVerify": latest_verify,
        "warnings": warnings,
    }
