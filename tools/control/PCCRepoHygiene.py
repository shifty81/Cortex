#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import zipfile
from dataclasses import dataclass, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

VERSION = "PCC-REPO-HYGIENE-0.1"

MANUAL_OVERWRITE_RE = re.compile(r"(?i)(manual[-_ ]?overwrite|folder[-_ ]?overwrite|overwrite[-_ ]?(?:fix|repair|patch))")
DEBUG_RE = re.compile(r"(?i)(latest[_ -]?debug[_ -]?bundle|debug[_ -]?bundle)")
PATCH_NAME_RE = re.compile(r"(?i)(root[-_ ]?patch|rootpatch|incremental[-_ ]?patch|patch[-_ ]?update)")
NON_PATCH_RE = re.compile(r"(?i)(debugbundle|debug[-_ ]?bundle|source[-_ ]?(?:rollup|bundle)|rollup|backup|audit[-_ ]?package)")
SIDECAR_SUFFIXES = (".sha256", ".sha256.txt")

LOCAL_EXCLUDE_BEGIN = "# BEGIN UNIVERSAL PCC LOCAL OPERATIONAL EXCLUDES"
LOCAL_EXCLUDE_END = "# END UNIVERSAL PCC LOCAL OPERATIONAL EXCLUDES"
LOCAL_EXCLUDE_PATTERNS = (
    "/artifacts/",
    "/.project_control/",
    "/.cortex/",
    "/*ManualOverwrite*.zip*",
    "/*Manual_Overwrite*.zip*",
    "/*Manual-Overwrite*.zip*",
    "/*RootPatch*.zip*",
    "/*Root_Patch*.zip*",
    "/*Root-Patch*.zip*",
    "/LATEST_DEBUG_BUNDLE*",
    "/*DebugBundle*.zip*",
)


def _git_exclude_path(root: Path) -> Path | None:
    git = shutil.which("git")
    if not git or not (root / ".git").exists():
        return None
    try:
        cp = subprocess.run(
            [git, "-C", str(root), "rev-parse", "--git-path", "info/exclude"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding="utf-8",
            errors="replace", timeout=10, check=False, creationflags=(getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0),
        )
    except Exception:
        return None
    if cp.returncode != 0 or not cp.stdout.strip():
        return None
    path = Path(cp.stdout.strip())
    if not path.is_absolute():
        path = root / path
    return path.resolve()


def ensure_local_git_excludes(root: Path) -> dict[str, Any]:
    path = _git_exclude_path(root)
    if path is None:
        return {"updated": False, "path": "", "reason": "git unavailable or repository not initialized"}
    path.parent.mkdir(parents=True, exist_ok=True)
    existing = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
    block = LOCAL_EXCLUDE_BEGIN + "\n" + "\n".join(LOCAL_EXCLUDE_PATTERNS) + "\n" + LOCAL_EXCLUDE_END
    pattern = re.compile(re.escape(LOCAL_EXCLUDE_BEGIN) + r".*?" + re.escape(LOCAL_EXCLUDE_END), re.S)
    if pattern.search(existing):
        updated_text = pattern.sub(block, existing)
    else:
        prefix = existing.rstrip()
        updated_text = (prefix + "\n\n" if prefix else "") + block + "\n"
    changed = updated_text != existing
    if changed:
        temp = path.with_name(path.name + ".pcc.tmp")
        temp.write_text(updated_text, encoding="utf-8")
        os.replace(temp, path)
    return {"updated": changed, "path": str(path), "reason": "managed local PCC operational excludes"}


@dataclass
class MoveRecord:
    source: str
    destination: str
    reason: str


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def stamp() -> str:
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def _artifact_root(root: Path) -> Path:
    return root / "artifacts"


def _receipt_roots(root: Path) -> list[Path]:
    return [
        root / "artifacts" / "patches" / "receipts",
        root / "updates" / "receipts",
        root / ".project_control" / "receipts",
        root / ".cortex" / "patches" / "receipts",
    ]


def _read_applied_patch_ids(root: Path) -> set[str]:
    ids: set[str] = set()
    for folder in _receipt_roots(root):
        if not folder.is_dir():
            continue
        for path in folder.glob("*.json"):
            try:
                data = json.loads(path.read_text(encoding="utf-8-sig"))
            except Exception:
                continue
            status = str(data.get("status") or data.get("result") or "").strip().casefold()
            if status not in {"applied", "pass", "success", "green"}:
                continue
            patch_id = str(data.get("patchId") or data.get("patch_id") or "").strip()
            if patch_id:
                ids.add(patch_id.casefold())
    return ids


def _zip_patch_id(path: Path) -> str:
    try:
        with zipfile.ZipFile(path, "r") as zf:
            names = {name.replace("\\", "/").casefold(): name for name in zf.namelist()}
            actual = names.get("patch_manifest.json")
            if not actual:
                return ""
            data = json.loads(zf.read(actual).decode("utf-8-sig"))
            return str(data.get("patchId") or data.get("patch_id") or "").strip()
    except Exception:
        return ""


def _is_patch_transport(path: Path) -> bool:
    if path.suffix.casefold() != ".zip":
        return False
    low = path.name.casefold()
    if NON_PATCH_RE.search(low):
        return False
    if PATCH_NAME_RE.search(low):
        return True
    return bool(_zip_patch_id(path))


def _sidecars_for(path: Path) -> list[Path]:
    return [candidate for candidate in (Path(str(path) + suffix) for suffix in SIDECAR_SUFFIXES) if candidate.is_file()]


def _unique_destination(dest: Path) -> Path:
    if not dest.exists():
        return dest
    stem, suffix = dest.stem, dest.suffix
    counter = 2
    while True:
        candidate = dest.with_name(f"{stem}_{counter}{suffix}")
        if not candidate.exists():
            return candidate
        counter += 1


def _move_one(source: Path, destination: Path, reason: str, *, apply: bool) -> MoveRecord:
    destination = _unique_destination(destination)
    if apply:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(source), str(destination))
    return MoveRecord(str(source), str(destination), reason)


def _move_with_sidecars(source: Path, destination_dir: Path, reason: str, *, apply: bool) -> list[MoveRecord]:
    records: list[MoveRecord] = []
    destination = destination_dir / source.name
    records.append(_move_one(source, destination, reason, apply=apply))
    for sidecar in _sidecars_for(source):
        records.append(_move_one(sidecar, destination_dir / sidecar.name, reason + " sidecar", apply=apply))
    return records


def classify_root(root: Path) -> dict[str, Any]:
    root = root.resolve()
    applied_ids = _read_applied_patch_ids(root)
    manual: list[str] = []
    consumed_patches: list[str] = []
    pending_patches: list[str] = []
    debug_residue: list[str] = []
    other_zip: list[str] = []
    for path in root.iterdir():
        if not path.is_file():
            continue
        low = path.name.casefold()
        if path.suffix.casefold() == ".zip":
            if MANUAL_OVERWRITE_RE.search(path.name):
                manual.append(path.name)
                continue
            if _is_patch_transport(path):
                patch_id = _zip_patch_id(path)
                if patch_id and patch_id.casefold() in applied_ids:
                    consumed_patches.append(path.name)
                else:
                    pending_patches.append(path.name)
                continue
            if DEBUG_RE.search(path.name):
                debug_residue.append(path.name)
                continue
            other_zip.append(path.name)
            continue
        if DEBUG_RE.search(path.name) and path.suffix.casefold() in {".txt", ".json", ".sha256"}:
            debug_residue.append(path.name)
    return {
        "schema": "pcc.repo_hygiene_scan.v1",
        "root": str(root),
        "manualOverwrite": sorted(manual),
        "consumedPatchTransports": sorted(consumed_patches),
        "pendingPatchTransports": sorted(pending_patches),
        "debugResidue": sorted(debug_residue),
        "otherZip": sorted(other_zip),
        "looseOperationalCount": len(manual) + len(consumed_patches) + len(debug_residue),
        "sourceMutationPerformed": False,
    }


def prepare(root: Path, *, apply: bool = True) -> dict[str, Any]:
    root = root.resolve()
    git_excludes = (
        ensure_local_git_excludes(root)
        if apply
        else {"updated": False, "path": str(_git_exclude_path(root) or ""), "reason": "dry-run; no Git exclude mutation"}
    )
    scan = classify_root(root)
    session = stamp()
    moves: list[MoveRecord] = []
    artifact = _artifact_root(root)

    for name in scan["manualOverwrite"]:
        path = root / name
        if path.is_file():
            moves.extend(_move_with_sidecars(path, artifact / "transports" / "manual-overwrites" / "archived" / session, "manual overwrite transport", apply=apply))

    for name in scan["consumedPatchTransports"]:
        path = root / name
        if path.is_file():
            moves.extend(_move_with_sidecars(path, artifact / "patches" / "consumed-transports" / session, "already-applied patch transport", apply=apply))

    for name in scan["debugResidue"]:
        path = root / name
        if not path.is_file():
            continue
        low = path.name.casefold()
        if path.suffix.casefold() == ".zip":
            dest_dir = artifact / "debug" / "legacy-root"
        else:
            dest_dir = artifact / "debug" / "legacy-root"
        moves.append(_move_one(path, dest_dir / path.name, "root debug residue", apply=apply))

    result = {
        "schema": "pcc.repo_hygiene_result.v1",
        "version": VERSION,
        "root": str(root),
        "mode": "apply" if apply else "dry-run",
        "timestampUtc": utc_now(),
        "moves": [asdict(row) for row in moves],
        "moved": len(moves),
        "pendingPatchTransports": scan["pendingPatchTransports"],
        "otherZip": scan["otherZip"],
        "sourceMutationPerformed": False,
        "localGitExcludes": git_excludes,
    }
    if apply:
        reports = artifact / "repository" / "hygiene"
        reports.mkdir(parents=True, exist_ok=True)
        latest = reports / "latest.json"
        latest.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def print_result(result: dict[str, Any]) -> None:
    moved = int(result.get("moved", 0) or 0)
    if moved:
        print(f"[PASS] Repository transport hygiene moved {moved} operational artifact(s).")
        for row in result.get("moves", []):
            print(f"  MOVE {Path(row['source']).name} -> {row['destination']} [{row['reason']}]")
    else:
        print("[PASS] Repository transport hygiene: no loose operational artifacts to move.")
    pending = result.get("pendingPatchTransports") or []
    if pending:
        print(f"[INFO] Pending project patch transport(s) preserved for update authority: {len(pending)}")
        for name in pending:
            print(f"  PENDING {name}")
    print("PCC_REPO_HYGIENE_JSON=" + json.dumps(result, separators=(",", ":"), sort_keys=True))


def main() -> int:
    ap = argparse.ArgumentParser(description="Universal Project Control Center repository transport hygiene")
    ap.add_argument("action", choices=("scan", "prepare", "finalize"))
    ap.add_argument("--root", required=True)
    ap.add_argument("--dry-run", action="store_true")
    ns = ap.parse_args()
    root = Path(ns.root).expanduser().resolve()
    if not root.is_dir():
        print(f"[FAIL] Project root does not exist: {root}")
        return 2
    if ns.action == "scan":
        print(json.dumps(classify_root(root), separators=(",", ":"), sort_keys=True))
        return 0
    result = prepare(root, apply=not ns.dry_run)
    print_result(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
