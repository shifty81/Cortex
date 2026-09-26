#!/usr/bin/env python3
"""Explicit, verified repo-to-volume migration. Never remove tracked runtime placeholders."""
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

VERSION = "CORTEX-REPO-CONVERGENCE-R8F-0.2"


def _digest(path: Path) -> bytes:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for part in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(part)
    return hasher.digest()


def _is_link(path: Path) -> bool:
    return path.is_symlink() or bool(getattr(path, "is_junction", lambda: False)())


def _check_destination(volume: Path, target: Path) -> None:
    # A configured external destination must not escape through a junction/symlink.
    rel = target.relative_to(volume)
    probe = volume
    for item in rel.parts:
        probe = probe / item
        if _is_link(probe):
            raise RuntimeError(f"Refusing linked destination: {probe}")


def _tracked_paths(root: Path, rel: str) -> set[str]:
    opts = {"stdout": subprocess.PIPE, "stderr": subprocess.PIPE, "check": False}
    if os.name == "nt":
        opts["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    result = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--", rel], **opts)
    if result.returncode:
        raise RuntimeError("Cannot establish tracked-file authority; repository convergence stopped: "
                           + result.stderr.decode("utf-8", "replace"))
    return {raw.decode("utf-8", "surrogateescape").replace("\\", "/")
            for raw in result.stdout.split(b"\0") if raw}


def _source_files(src: Path) -> list[Path]:
    if _is_link(src):
        raise RuntimeError(f"Refusing linked migration source: {src}")
    if src.is_file():
        return [src]
    result = []
    for item in src.rglob("*"):
        if _is_link(item):
            raise RuntimeError(f"Refusing linked migration content: {item}")
        if item.is_file():
            result.append(item)
    return sorted(result)


def _copy_verified(src: Path, dst: Path, volume: Path) -> None:
    _check_destination(volume, dst)
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists():
        if not dst.is_file() or _digest(dst) != _digest(src):
            raise RuntimeError(f"Destination collision; refusing overwrite: {dst}")
    else:
        shutil.copy2(src, dst)
    if _digest(dst) != _digest(src):
        raise RuntimeError(f"Verification failed: {src} -> {dst}")


def _prune_empty_dirs(src: Path) -> None:
    if not src.is_dir():
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


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args(argv)
    root = Path(args.root).resolve(strict=True)
    policy = json.loads((root / "config/cortex/repository_layout.v1.json").read_text(encoding="utf-8"))
    volume = root.parent
    archive = volume / "Vault" / "history" / "Cortex" / "repository-convergence" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    protected = {str(x).replace("\\", "/") for x in policy.get("preserve_runtime_placeholders", [])}
    actions: list[tuple[str, Path, Path, list[Path]]] = []

    for src_rel, dst_rel in policy["runtime_state"].items():
        src = root / src_rel
        if not src.exists() and not _is_link(src):
            continue
        tracked = _tracked_paths(root, src_rel)
        movable = [p for p in _source_files(src)
                   if p.relative_to(root).as_posix() not in tracked
                   and p.relative_to(root).as_posix() not in protected]
        if movable:
            actions.append(("runtime", src, volume / dst_rel, movable))

    migration = root / "migration"
    if migration.exists():
        tracked = _tracked_paths(root, "migration")
        candidates = [p for p in _source_files(migration) if p.relative_to(root).as_posix() not in tracked]
        if candidates:
            actions.append(("archive", migration, archive / "migration", candidates))
    for item in root.iterdir():
        if item.is_file() and any(fnmatch.fnmatch(item.name, mask) for mask in policy["archive_root_globs"]):
            if item.relative_to(root).as_posix() not in _tracked_paths(root, item.name):
                actions.append(("archive", item, archive / "root-history" / item.name, [item]))

    print(f"{VERSION} {'APPLY' if args.apply else 'PLAN'}")
    for kind, src, dst, files in actions:
        print(f"{kind.upper():7} {src} -> {dst} ({len(files)} file(s); tracked placeholders preserved)")
    if not args.apply:
        print(f"[PASS] Plan ready: {len(actions)} action(s); no files changed.")
        return 0
    if not actions:
        print("[PASS] Nothing to migrate; no receipt or source changes.")
        return 0

    # Verify ALL actions before deleting ANY source content. A destination collision
    # can leave a harmless duplicate copy, but never removes the source.
    receipt = {"schema": "cortex.repository_convergence.receipt.v1", "version": VERSION,
               "createdUtc": datetime.now(timezone.utc).isoformat(), "actions": []}
    for kind, src, dst, files in actions:
        for p in files:
            target = dst / p.relative_to(src) if src.is_dir() else dst
            _copy_verified(p, target, volume)
    for kind, src, dst, files in actions:
        for p in files:
            p.unlink()
        if src.is_dir():
            _prune_empty_dirs(src)
        receipt["actions"].append({"kind": kind, "source": str(src), "destination": str(dst),
                                   "files": len(files), "trackedPreserved": kind == "runtime"})

    state = volume / ".cortex" / "receipts" / "repository-convergence"
    _check_destination(volume, state)
    state.mkdir(parents=True, exist_ok=True)
    name = datetime.now(timezone.utc).strftime("r8f-%Y%m%dT%H%M%S%fZ.json")
    output = state / name
    output.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"[PASS] Applied {len(actions)} verified action(s). Receipt: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
