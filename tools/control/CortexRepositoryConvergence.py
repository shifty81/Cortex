#!/usr/bin/env python3
"""Explicit, verified repository runtime/archive convergence.

Project-local runtime state is migrated into the repository's ``.cortex`` state
area. Portable-volume runtime state is migrated to the physical Cortex/Vault
volume next to the repository. Runtime copies are SHA-256 verified before any
source file is removed. Tracked placeholders are never moved.
"""
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import shutil
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

VERSION = "CORTEX-REPO-CONVERGENCE-W14A3-0.3"


def _digest(path: Path) -> bytes:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for part in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(part)
    return hasher.digest()


def _is_link(path: Path) -> bool:
    return path.is_symlink() or bool(getattr(path, "is_junction", lambda: False)())


def _check_destination(boundary: Path, target: Path) -> None:
    boundary = boundary.resolve()
    target = target.resolve(strict=False)
    rel = target.relative_to(boundary)
    probe = boundary
    for item in rel.parts:
        probe = probe / item
        if probe.exists() and _is_link(probe):
            raise RuntimeError(f"Refusing linked destination: {probe}")


def _tracked_paths(root: Path, rel: str) -> set[str]:
    if not (root / ".git").exists():
        return set()
    opts: dict[str, Any] = {"stdout": subprocess.PIPE, "stderr": subprocess.PIPE, "check": False}
    if os.name == "nt":
        opts["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    result = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--", rel], **opts)
    if result.returncode:
        raise RuntimeError(
            "Cannot establish tracked-file authority; repository convergence stopped: "
            + result.stderr.decode("utf-8", "replace")
        )
    return {
        raw.decode("utf-8", "surrogateescape").replace("\\", "/")
        for raw in result.stdout.split(b"\0")
        if raw
    }


def _source_files(src: Path) -> list[Path]:
    if _is_link(src):
        raise RuntimeError(f"Refusing linked migration source: {src}")
    if src.is_file():
        return [src]
    result: list[Path] = []
    for item in src.rglob("*"):
        if _is_link(item):
            raise RuntimeError(f"Refusing linked migration content: {item}")
        if item.is_file():
            result.append(item)
    return sorted(result)


def _copy_verified(src: Path, dst: Path, boundary: Path) -> None:
    _check_destination(boundary, dst)
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists():
        if not dst.is_file() or _digest(dst) != _digest(src):
            raise RuntimeError(f"Destination collision; refusing overwrite: {dst}")
    else:
        shutil.copy2(src, dst)
    if _digest(dst) != _digest(src):
        raise RuntimeError(f"Verification failed: {src} -> {dst}")


def _prune_empty_dirs(src: Path) -> None:
    if not src.is_dir() or _is_link(src):
        return
    for item in sorted(src.rglob("*"), key=lambda p: len(p.parts), reverse=True):
        if item.is_dir() and not _is_link(item):
            try:
                item.rmdir()
            except OSError:
                pass
    try:
        src.rmdir()
    except OSError:
        pass


def _mapping_rows(value: Any) -> Iterable[tuple[str, str]]:
    if isinstance(value, dict):
        for rel, destination in value.items():
            yield str(rel), str(destination)
    elif isinstance(value, list):
        for rel in value:
            yield str(rel), ""


def _load_policy(root: Path) -> dict[str, Any]:
    return json.loads((root / "config/cortex/repository_layout.v1.json").read_text(encoding="utf-8"))


def _runtime_actions(root: Path, policy: dict[str, Any], scopes: set[str]) -> list[dict[str, Any]]:
    volume = root.parent.resolve()
    protected = {str(x).replace("\\", "/") for x in policy.get("preserve_runtime_placeholders", [])}
    actions: list[dict[str, Any]] = []
    specifications = [
        ("project", policy.get("runtime_state", {}), volume, volume, False),
        ("volume", policy.get("volume_runtime_state", {}), volume, volume, True),
    ]
    for scope, mapping, destination_base, boundary, remove_empty_root in specifications:
        if scope not in scopes:
            continue
        for src_rel, dst_rel in _mapping_rows(mapping):
            src = root / src_rel
            if not src.exists() and not _is_link(src):
                continue
            tracked = _tracked_paths(root, src_rel)
            movable = [
                p for p in _source_files(src)
                if p.relative_to(root).as_posix() not in tracked
                and p.relative_to(root).as_posix() not in protected
            ]
            if not movable and (not remove_empty_root or tracked):
                continue
            actions.append({
                "kind": "runtime",
                "scope": scope,
                "source": src,
                "sourceRel": src_rel,
                "destination": destination_base / dst_rel,
                "boundary": boundary,
                "files": movable,
                "tracked": tracked,
            })
    return actions


def migrate_runtime_state(root: Path, *, scopes: Iterable[str] = ("project", "volume")) -> dict[str, Any]:
    """Apply verified runtime-state migrations and return a structured receipt.

    All destination files for all selected actions are verified before any source
    file is removed. Different-content collisions therefore fail closed and keep
    the legacy source intact.
    """
    root = root.resolve(strict=True)
    policy = _load_policy(root)
    selected = {str(scope) for scope in scopes}
    actions = _runtime_actions(root, policy, selected)
    if not actions:
        return {"schema": "cortex.runtime_migration.v1", "migrated": 0, "actions": [], "scopes": sorted(selected)}

    # Copy/verify the entire migration set before removing any source bytes.
    for action in actions:
        src: Path = action["source"]
        dst: Path = action["destination"]
        boundary: Path = action["boundary"]
        files: list[Path] = action["files"]
        for path in files:
            target = dst / path.relative_to(src) if src.is_dir() else dst
            _copy_verified(path, target, boundary)

    migrated = 0
    rows: list[dict[str, Any]] = []
    for action in actions:
        src: Path = action["source"]
        files: list[Path] = action["files"]
        for path in files:
            path.unlink()
            migrated += 1
        if src.is_dir():
            _prune_empty_dirs(src)
        rows.append({
            "scope": action["scope"],
            "source": action["sourceRel"],
            "destination": str(action["destination"]),
            "files": len(files),
            "trackedPreserved": len(action["tracked"]),
            "sourceRemoved": not src.exists(),
        })
    return {
        "schema": "cortex.runtime_migration.v1",
        "migrated": migrated,
        "actions": rows,
        "scopes": sorted(selected),
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args(argv)
    root = Path(args.root).resolve(strict=True)
    policy = _load_policy(root)
    volume = root.parent.resolve()
    archive = volume / "Vault" / "history" / "Cortex" / "repository-convergence" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    runtime_actions = _runtime_actions(root, policy, {"project", "volume"})
    archive_actions: list[tuple[str, Path, Path, list[Path]]] = []

    migration = root / "migration"
    if migration.exists():
        tracked = _tracked_paths(root, "migration")
        candidates = [p for p in _source_files(migration) if p.relative_to(root).as_posix() not in tracked]
        if candidates:
            archive_actions.append(("archive", migration, archive / "migration", candidates))
    for item in root.iterdir():
        if item.is_file() and any(fnmatch.fnmatch(item.name, mask) for mask in policy.get("archive_root_globs", [])):
            if item.relative_to(root).as_posix() not in _tracked_paths(root, item.name):
                archive_actions.append(("archive", item, archive / "root-history" / item.name, [item]))

    print(f"{VERSION} {'APPLY' if args.apply else 'PLAN'}")
    for action in runtime_actions:
        print(
            f"RUNTIME {action['scope']:7} {action['source']} -> {action['destination']} "
            f"({len(action['files'])} file(s); {len(action['tracked'])} tracked preserved)"
        )
    for kind, src, dst, files in archive_actions:
        print(f"{kind.upper():7} {src} -> {dst} ({len(files)} file(s))")
    total = len(runtime_actions) + len(archive_actions)
    if not args.apply:
        print(f"[PASS] Plan ready: {total} action(s); no files changed.")
        return 0
    if not total:
        print("[PASS] Nothing to migrate; no receipt or source changes.")
        return 0

    runtime_result = migrate_runtime_state(root)
    for kind, src, dst, files in archive_actions:
        for path in files:
            target = dst / path.relative_to(src) if src.is_dir() else dst
            _copy_verified(path, target, volume)
    for kind, src, dst, files in archive_actions:
        for path in files:
            path.unlink()
        if src.is_dir():
            _prune_empty_dirs(src)

    receipt = {
        "schema": "cortex.repository_convergence.receipt.v2",
        "version": VERSION,
        "createdUtc": datetime.now(timezone.utc).isoformat(),
        "runtime": runtime_result,
        "archiveActions": [
            {"kind": kind, "source": str(src), "destination": str(dst), "files": len(files)}
            for kind, src, dst, files in archive_actions
        ],
    }
    state = volume / ".cortex" / "receipts" / "repository-convergence"
    _check_destination(volume, state)
    state.mkdir(parents=True, exist_ok=True)
    name = datetime.now(timezone.utc).strftime("w14a3-%Y%m%dT%H%M%S%fZ.json")
    output = state / name
    output.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"[PASS] Applied {total} verified action(s). Receipt: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
