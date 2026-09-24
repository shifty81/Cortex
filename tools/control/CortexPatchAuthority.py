#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tempfile
import time
import uuid
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Iterable

SCHEMA = "cortex.root_patch.v1"
PROJECT = "Cortex"
DENIED_TOP_LEVEL = {
    ".git",
    ".cortex",
    ".project_control",
    "artifacts",
    "logs",
    "target",
    "updates",
    "handoffs",
}
TRANSPORT_NAMES = {"PATCH_MANIFEST.json", ".cortex-patch.json"}
CONTROL_RE = re.compile(
    r"^(?:project\.control\.json|PROJECT_CONTROL_CENTER\.cmd|"
    r"tools/(?:control|pcc)/[^/]+\.(?:py|ps1|psm1))$",
    re.IGNORECASE,
)
PATCH_NAME_RE = re.compile(r"(?:root[-_ ]?patch|rootpatch|incremental[-_ ]?patch|patch[-_ ]?update)", re.I)
NON_PATCH_RE = re.compile(
    r"(?:debugbundle|debug[-_ ]?bundle|handoff|source[-_ ]?(?:rollup|bundle)|rollup|backup|"
    r"support[-_ ]?bundle|archive)",
    re.I,
)
PATCH_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{2,127}$")
MAX_PATCH_FILES = 5000
MAX_PATCH_UNCOMPRESSED_BYTES = 2 * 1024 * 1024 * 1024
MAX_SINGLE_FILE_BYTES = 512 * 1024 * 1024
MAX_PATH_CHARS = 240
MAX_COMPRESSION_RATIO = 1000.0


class PatchError(RuntimeError):
    pass


def now_utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def stamp() -> str:
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def emit(kind: str, message: str) -> None:
    print(f"[{kind}] {message}")


def result_line(payload: dict[str, Any]) -> None:
    print("PCC_RESULT_JSON=" + json.dumps(payload, separators=(",", ":"), sort_keys=True))


def normalize_rel(raw: str) -> str:
    if not isinstance(raw, str) or not raw.strip():
        raise PatchError("Patch path must be a non-empty string.")
    value = raw.replace("\\", "/").strip()
    p = PurePosixPath(value)
    if p.is_absolute() or re.match(r"^[A-Za-z]:", value):
        raise PatchError(f"Rooted patch path is not allowed: {raw}")
    parts = p.parts
    if not parts or any(part in {"", ".", ".."} for part in parts):
        raise PatchError(f"Unsafe relative patch path: {raw}")
    normalized = "/".join(parts)
    if len(normalized) > MAX_PATH_CHARS:
        raise PatchError(f"Patch path exceeds {MAX_PATH_CHARS} characters: {raw}")
    if normalized in TRANSPORT_NAMES:
        raise PatchError(f"Transport metadata cannot be a payload path: {raw}")
    if parts[0].lower() in DENIED_TOP_LEVEL:
        raise PatchError(f"Operational path cannot be delivered by patch payload: {raw}")
    return normalized


def local_path(root: Path, rel: str) -> Path:
    candidate = root.joinpath(*PurePosixPath(rel).parts)
    # commonpath is lexical and avoids following a malicious existing symlink here.
    if os.path.commonpath([str(root), str(candidate)]) != str(root):
        raise PatchError(f"Patch destination escapes project root: {rel}")
    return candidate


def is_reparse_or_symlink(path: Path) -> bool:
    try:
        st = path.lstat()
    except FileNotFoundError:
        return False
    if stat.S_ISLNK(st.st_mode):
        return True
    attrs = getattr(st, "st_file_attributes", 0)
    reparse = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attrs & reparse)


def assert_safe_ancestors(root: Path, dest: Path) -> None:
    root = root.absolute()
    current = root
    try:
        rel_parts = dest.relative_to(root).parts
    except ValueError as exc:
        raise PatchError(f"Destination escapes root: {dest}") from exc
    # The destination itself may already be a normal file; reject links/reparse points too.
    for part in rel_parts:
        current = current / part
        if current.exists() or current.is_symlink():
            if is_reparse_or_symlink(current):
                raise PatchError(f"Patch destination crosses a reparse point/symlink: {current}")


def sidecar_candidates(zip_path: Path) -> tuple[Path, Path]:
    return Path(str(zip_path) + ".sha256"), Path(str(zip_path) + ".sha256.txt")


def existing_sidecar(zip_path: Path) -> Path | None:
    for candidate in sidecar_candidates(zip_path):
        if candidate.is_file():
            return candidate
    return None


def read_sidecar(zip_path: Path) -> Path:
    candidate = existing_sidecar(zip_path)
    if candidate is not None:
        text = candidate.read_text(encoding="utf-8", errors="strict").strip()
        m = re.match(r"^([A-Fa-f0-9]{64})(?:\s+\*?.+)?$", text)
        if not m:
            raise PatchError(f"Malformed SHA-256 sidecar: {candidate.name}")
        expected = m.group(1).lower()
        actual = sha256_file(zip_path)
        if actual != expected:
            raise PatchError(f"ZIP SHA-256 mismatch for {zip_path.name}")
        return candidate
    raise PatchError(f"Required SHA-256 sidecar is missing for {zip_path.name}")


def write_sidecar_atomic(zip_path: Path) -> tuple[Path, str]:
    """Create the canonical transport sidecar after internal patch validation succeeds.

    This is recovery for an omitted transport checksum, not a bypass for a bad one.
    Existing malformed or mismatched sidecars are never replaced automatically.
    """
    target = Path(str(zip_path) + ".sha256")
    digest = sha256_file(zip_path)
    tmp = target.with_name(target.name + ".tmp-" + uuid.uuid4().hex)
    try:
        tmp.write_text(f"{digest}  {zip_path.name}\n", encoding="ascii")
        if sha256_file(zip_path) != digest:
            raise PatchError(f"ZIP changed while recovering SHA-256 sidecar: {zip_path.name}")
        os.replace(tmp, target)
    finally:
        try:
            if tmp.exists():
                tmp.unlink()
        except OSError:
            pass
    return target, digest


def zip_regular_names(zf: zipfile.ZipFile) -> list[str]:
    names: list[str] = []
    total_uncompressed = 0
    for info in zf.infolist():
        raw = info.filename.replace("\\", "/")
        if not raw or raw.endswith("/"):
            continue
        if raw.startswith("/") or re.match(r"^[A-Za-z]:", raw):
            raise PatchError(f"Unsafe rooted ZIP entry: {raw}")
        p = PurePosixPath(raw)
        if any(part in {"", ".", ".."} for part in p.parts):
            raise PatchError(f"Unsafe ZIP traversal entry: {raw}")
        normalized = "/".join(p.parts)
        if len(normalized) > MAX_PATH_CHARS:
            raise PatchError(f"ZIP entry path exceeds {MAX_PATH_CHARS} characters: {normalized}")
        mode = (info.external_attr >> 16) & 0xFFFF
        if mode and (stat.S_ISLNK(mode) or stat.S_ISCHR(mode) or stat.S_ISBLK(mode) or stat.S_ISFIFO(mode)):
            raise PatchError(f"Non-regular ZIP entry is not allowed: {raw}")
        if info.file_size > MAX_SINGLE_FILE_BYTES:
            raise PatchError(f"ZIP entry exceeds per-file safety budget: {raw}")
        total_uncompressed += info.file_size
        if total_uncompressed > MAX_PATCH_UNCOMPRESSED_BYTES:
            raise PatchError("Patch exceeds total uncompressed safety budget.")
        if info.compress_size > 0 and info.file_size > 1024 * 1024:
            ratio = info.file_size / info.compress_size
            if ratio > MAX_COMPRESSION_RATIO:
                raise PatchError(f"Suspicious ZIP compression ratio for {raw}: {ratio:.1f}:1")
        names.append(normalized)
    if len(names) > MAX_PATCH_FILES + 1:
        raise PatchError(f"Patch exceeds file-count safety budget ({MAX_PATCH_FILES}).")
    folded = [name.casefold() for name in names]
    if len(folded) != len(set(folded)):
        raise PatchError("ZIP contains duplicate file entries (case-insensitive).")
    return names


def parse_manifest(zf: zipfile.ZipFile) -> tuple[dict[str, Any], str]:
    names = zip_regular_names(zf)
    matches = [name for name in names if name.casefold() == "patch_manifest.json"]
    if len(matches) != 1 or matches[0] != "PATCH_MANIFEST.json":
        raise PatchError("Recognized Cortex patches require exactly one top-level PATCH_MANIFEST.json with canonical casing.")
    manifest_name = matches[0]
    try:
        manifest = json.loads(zf.read(manifest_name).decode("utf-8-sig"))
    except Exception as exc:
        raise PatchError(f"PATCH_MANIFEST.json is invalid JSON: {exc}") from exc
    if not isinstance(manifest, dict):
        raise PatchError("PATCH_MANIFEST.json must be a JSON object.")
    if manifest.get("schema") != SCHEMA:
        raise PatchError(f"Unsupported patch schema: {manifest.get('schema')!r}")
    if str(manifest.get("project", "")).strip().casefold() != PROJECT.casefold():
        raise PatchError(f"Patch project must be {PROJECT}.")
    for field in ("patchId", "title", "series", "sequence"):
        if field not in manifest or str(manifest[field]).strip() == "":
            raise PatchError(f"Patch manifest field is required: {field}")
    patch_id = str(manifest["patchId"]).strip()
    if not PATCH_ID_RE.fullmatch(patch_id):
        raise PatchError("patchId must be 3-128 characters using only letters, digits, dot, underscore or hyphen.")
    title = str(manifest["title"]).strip()
    if len(title) > 240:
        raise PatchError("Patch title exceeds 240 characters.")
    apply_mode = str(manifest.get("applyMode", "overwrite")).strip().lower()
    if apply_mode not in {"overwrite", "transactional"}:
        raise PatchError(f"Unsupported applyMode: {apply_mode}")
    files = manifest.get("files")
    if not isinstance(files, list) or not files:
        raise PatchError("Patch manifest files must be a non-empty array.")
    if len(files) > MAX_PATCH_FILES:
        raise PatchError(f"Patch manifest exceeds file-count safety budget ({MAX_PATCH_FILES}).")
    remove = manifest.get("remove", [])
    if remove is None:
        remove = []
    if not isinstance(remove, list):
        raise PatchError("Patch manifest remove must be an array.")
    depends = manifest.get("dependsOn", [])
    if depends is None:
        depends = []
    if not isinstance(depends, list) or any(not isinstance(x, str) or not x.strip() for x in depends):
        raise PatchError("dependsOn must be an array of non-empty patch IDs.")
    dep_folded: set[str] = set()
    for dep in depends:
        dep = dep.strip()
        if not PATCH_ID_RE.fullmatch(dep):
            raise PatchError(f"Invalid dependency patch ID: {dep}")
        folded = dep.casefold()
        if folded == patch_id.casefold():
            raise PatchError("Patch cannot depend on itself.")
        if folded in dep_folded:
            raise PatchError(f"Duplicate dependency patch ID: {dep}")
        dep_folded.add(folded)
    manifest["patchId"] = patch_id
    manifest["title"] = title
    manifest["dependsOn"] = [str(x).strip() for x in depends]
    return manifest, manifest_name


@dataclass
class FileSpec:
    path: str
    sha256: str
    size: int
    before_sha256: str | None
    before_size: int | None
    must_exist: bool | None


@dataclass
class Validation:
    zip_path: Path
    sidecar_path: Path | None
    manifest: dict[str, Any]
    files: list[FileSpec]
    remove: list[str]
    restart_required: bool
    manifest_sha256: str
    sort_key: tuple[Any, ...]

    @property
    def patch_id(self) -> str:
        return str(self.manifest["patchId"])


def natural_key(value: Any) -> tuple[Any, ...]:
    parts = re.split(r"(\d+)", str(value).lower())
    return tuple(int(p) if p.isdigit() else p for p in parts if p != "")


def validate_patch(zip_path: Path, *, require_sidecar: bool = True) -> Validation:
    sidecar: Path | None = None
    if require_sidecar:
        sidecar = read_sidecar(zip_path)
    elif Path(str(zip_path) + ".sha256").is_file() or Path(str(zip_path) + ".sha256.txt").is_file():
        sidecar = read_sidecar(zip_path)

    with zipfile.ZipFile(zip_path, "r") as zf:
        manifest, manifest_name = parse_manifest(zf)
        manifest_bytes = zf.read(manifest_name)
        manifest_sha = sha256_bytes(manifest_bytes)
        names = zip_regular_names(zf)
        regular_set = {name.casefold(): name for name in names}

        file_specs: list[FileSpec] = []
        declared_folded: set[str] = set()
        for item in manifest["files"]:
            if not isinstance(item, dict):
                raise PatchError("Each manifest files entry must be an object.")
            rel = normalize_rel(item.get("path", ""))
            folded = rel.casefold()
            if folded in declared_folded:
                raise PatchError(f"Duplicate manifest payload path: {rel}")
            declared_folded.add(folded)
            digest = str(item.get("sha256", "")).lower()
            if not re.fullmatch(r"[a-f0-9]{64}", digest):
                raise PatchError(f"Invalid SHA-256 for payload: {rel}")
            try:
                size = int(item.get("bytes"))
            except Exception as exc:
                raise PatchError(f"Invalid byte count for payload: {rel}") from exc
            if size < 0:
                raise PatchError(f"Invalid byte count for payload: {rel}")
            actual_name = regular_set.get(folded)
            if actual_name is None:
                raise PatchError(f"Manifest payload is missing from ZIP: {rel}")
            data = zf.read(actual_name)
            if len(data) != size:
                raise PatchError(f"Byte count mismatch for {rel}: manifest={size}, zip={len(data)}")
            actual_sha = sha256_bytes(data)
            if actual_sha != digest:
                raise PatchError(f"SHA-256 mismatch for payload: {rel}")

            before_sha = item.get("beforeSha256")
            if before_sha is not None:
                before_sha = str(before_sha).lower()
                if not re.fullmatch(r"[a-f0-9]{64}", before_sha):
                    raise PatchError(f"Invalid beforeSha256 for payload: {rel}")
            before_size = item.get("beforeBytes")
            if before_size is not None:
                try:
                    before_size = int(before_size)
                except Exception as exc:
                    raise PatchError(f"Invalid beforeBytes for payload: {rel}") from exc
                if before_size < 0:
                    raise PatchError(f"Invalid beforeBytes for payload: {rel}")
            must_exist = item.get("mustExist")
            if must_exist is not None and not isinstance(must_exist, bool):
                raise PatchError(f"mustExist must be true/false for payload: {rel}")

            file_specs.append(FileSpec(rel, digest, size, before_sha, before_size, must_exist))

        remove: list[str] = []
        remove_folded: set[str] = set()
        for raw in manifest.get("remove", []) or []:
            rel = normalize_rel(raw)
            folded = rel.casefold()
            if folded in remove_folded:
                raise PatchError(f"Duplicate remove path: {rel}")
            if folded in declared_folded:
                raise PatchError(f"Path cannot be both written and removed: {rel}")
            remove_folded.add(folded)
            remove.append(rel)

        allowed = {"patch_manifest.json", *declared_folded}
        extras = [name for name in names if name.casefold() not in allowed]
        if extras:
            raise PatchError("ZIP contains undeclared regular files: " + ", ".join(extras[:8]))

        restart_required = any(CONTROL_RE.match(spec.path) for spec in file_specs) or any(
            CONTROL_RE.match(rel) for rel in remove
        )
        sort_key = (natural_key(manifest["series"]), natural_key(manifest["sequence"]), zip_path.name.lower())
        return Validation(
            zip_path=zip_path,
            sidecar_path=sidecar,
            manifest=manifest,
            files=file_specs,
            remove=remove,
            restart_required=restart_required,
            manifest_sha256=manifest_sha,
            sort_key=sort_key,
        )


def looks_like_patch(zip_path: Path) -> bool:
    name = zip_path.name
    if NON_PATCH_RE.search(name):
        return False
    if PATCH_NAME_RE.search(name):
        return True
    try:
        with zipfile.ZipFile(zip_path, "r") as zf:
            names = [n.replace("\\", "/").lower() for n in zf.namelist()]
            return "patch_manifest.json" in names
    except Exception:
        return False


def receipt_dir(root: Path) -> Path:
    return root / "artifacts" / "patches" / "receipts"


def applied_patch_records(root: Path) -> dict[str, dict[str, Any]]:
    """Read applied receipts case-insensitively and retain trusted source ZIP hashes.

    Historical Cortex receipts used both "APPLIED" and "applied".  Receipt status is
    therefore normalized instead of treating casing as part of the persistence contract.
    """
    out: dict[str, dict[str, Any]] = {}
    d = receipt_dir(root)
    if not d.is_dir():
        return out
    for receipt_path in d.glob("*.json"):
        try:
            data = json.loads(receipt_path.read_text(encoding="utf-8"))
            if str(data.get("status", "")).strip().casefold() != "applied":
                continue
            patch_id = str(data.get("patchId", "")).strip()
            if not patch_id:
                continue
            key = patch_id.casefold()
            record = out.setdefault(key, {"patchId": patch_id, "hashes": set(), "receipts": []})
            record["receipts"].append(str(receipt_path))
            for field in ("zipSha256", "sourceZipSha256"):
                digest = str(data.get(field, "")).strip().lower()
                if re.fullmatch(r"[a-f0-9]{64}", digest):
                    record["hashes"].add(digest)
        except Exception:
            continue
    return out


def applied_patch_ids(root: Path) -> set[str]:
    return {str(record["patchId"]) for record in applied_patch_records(root).values()}


_DOWNLOAD_COPY_RE = re.compile(r" \(\d+\)(?=\.zip$)", re.IGNORECASE)


def transport_preference_key(root: Path, validation: Validation) -> tuple[Any, ...]:
    """Choose one deterministic transport when browsers leave byte-identical copies."""
    path = validation.zip_path
    try:
        in_root = path.parent.resolve() == root.resolve()
    except OSError:
        in_root = path.parent.absolute() == root.absolute()
    browser_copy = bool(_DOWNLOAD_COPY_RE.search(path.name))
    return (
        0 if in_root else 1,
        0 if not browser_copy else 1,
        len(path.name),
        path.name.casefold(),
        str(path).casefold(),
    )


def id_map(values: Iterable[str]) -> dict[str, str]:
    return {value.casefold(): value for value in values}


def order_by_dependencies(validations: list[Validation], already: set[str]) -> tuple[list[Validation], list[dict[str, str]]]:
    by_id = {v.patch_id.casefold(): v for v in validations}
    already_folded = {x.casefold() for x in already}
    errors: list[dict[str, str]] = []
    for v in validations:
        for dep in v.manifest.get("dependsOn", []) or []:
            if dep.casefold() not in by_id and dep.casefold() not in already_folded:
                errors.append({"path": str(v.zip_path), "name": v.zip_path.name, "error": f"Missing dependency: {dep}"})
    if errors:
        bad = {e["path"] for e in errors}
        return [v for v in validations if str(v.zip_path) not in bad], errors

    ordered: list[Validation] = []
    remaining = dict(by_id)
    satisfied = set(already_folded)
    while remaining:
        ready = [
            v for v in remaining.values()
            if all(dep.casefold() in satisfied for dep in (v.manifest.get("dependsOn", []) or []))
        ]
        if not ready:
            cycle = ", ".join(sorted(v.patch_id for v in remaining.values()))
            for v in remaining.values():
                errors.append({"path": str(v.zip_path), "name": v.zip_path.name, "error": f"Dependency cycle/order deadlock involving: {cycle}"})
            break
        ready.sort(key=lambda v: v.sort_key)
        for v in ready:
            ordered.append(v)
            satisfied.add(v.patch_id.casefold())
            remaining.pop(v.patch_id.casefold(), None)
    return ordered, errors


def write_json_atomic(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp-" + uuid.uuid4().hex)
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def archive_transport(root: Path, validation: Validation, status: str, receipt: dict[str, Any]) -> Path:
    folder = root / "artifacts" / "patches" / ("applied" if status == "applied" else "failed")
    folder.mkdir(parents=True, exist_ok=True)
    target = folder / f"{stamp()}_{validation.zip_path.name}"
    if target.exists():
        target = folder / f"{stamp()}_{uuid.uuid4().hex[:8]}_{validation.zip_path.name}"
    side_target = Path(str(target) + ".sha256")
    copied_zip = False
    copied_side = False
    try:
        shutil.copy2(validation.zip_path, target)
        copied_zip = True
        if sha256_file(target) != sha256_file(validation.zip_path):
            raise PatchError(f"Archived ZIP verification failed: {target}")
        if validation.sidecar_path and validation.sidecar_path.exists():
            shutil.copy2(validation.sidecar_path, side_target)
            copied_side = True
            if validation.sidecar_path.read_bytes() != side_target.read_bytes():
                raise PatchError(f"Archived sidecar verification failed: {side_target}")
        # Delete transport only after archive copies are verified.
        validation.zip_path.unlink()
        if validation.sidecar_path and validation.sidecar_path.exists():
            validation.sidecar_path.unlink()
        receipt["archivePath"] = str(target)
        return target
    except Exception:
        if copied_zip:
            try: target.unlink()
            except OSError: pass
        if copied_side:
            try: side_target.unlink()
            except OSError: pass
        raise


def preimage_info(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"exists": False, "bytes": None, "sha256": None}
    if not path.is_file():
        raise PatchError(f"Patch target is not a regular file: {path}")
    return {"exists": True, "bytes": path.stat().st_size, "sha256": sha256_file(path)}


def precondition_issues(root: Path, validation: Validation) -> list[str]:
    issues: list[str] = []
    for spec in validation.files:
        dest = local_path(root, spec.path)
        try:
            assert_safe_ancestors(root, dest)
            info = preimage_info(dest)
        except Exception as exc:
            issues.append(f"{spec.path}: {exc}")
            continue

        if spec.must_exist is True and not info["exists"]:
            issues.append(f"{spec.path}: required target is missing")
            continue
        if spec.must_exist is False and info["exists"]:
            issues.append(
                f"{spec.path}: expected a new target, but a file already exists "
                f"(actual sha256={info['sha256']}, bytes={info['bytes']})"
            )
            continue
        if spec.before_sha256 is not None and (not info["exists"] or info["sha256"] != spec.before_sha256):
            issues.append(
                f"{spec.path}: SHA-256 mismatch "
                f"(expected {spec.before_sha256}, actual {info['sha256']})"
            )
        if spec.before_size is not None and (not info["exists"] or info["bytes"] != spec.before_size):
            issues.append(
                f"{spec.path}: byte-count mismatch "
                f"(expected {spec.before_size}, actual {info['bytes']})"
            )

    for rel in validation.remove:
        dest = local_path(root, rel)
        try:
            assert_safe_ancestors(root, dest)
        except Exception as exc:
            issues.append(f"{rel}: {exc}")
            continue
        if dest.exists() and not dest.is_file():
            issues.append(f"{rel}: remove target is not a regular file")
    return issues


def check_preconditions(root: Path, validation: Validation) -> None:
    issues = precondition_issues(root, validation)
    if not issues:
        return
    detail = "\n".join(f"  - {issue}" for issue in issues)
    raise PatchError(
        f"Patch preflight found {len(issues)} preimage conflict(s); no source files were modified:\n{detail}"
    )


def extract_declared(validation: Validation, stage: Path) -> None:
    with zipfile.ZipFile(validation.zip_path, "r") as zf:
        by_fold = {n.replace("\\", "/").casefold(): n for n in zf.namelist() if n and not n.endswith("/")}
        for spec in validation.files:
            name = by_fold[spec.path.casefold()]
            data = zf.read(name)
            dest = stage.joinpath(*PurePosixPath(spec.path).parts)
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(data)
            if dest.stat().st_size != spec.size or sha256_file(dest) != spec.sha256:
                raise PatchError(f"Staged payload verification failed: {spec.path}")


def copy_backup(src: Path, backup: Path) -> None:
    backup.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, backup)


def atomic_copy(src: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_name(dest.name + ".pcc-tmp-" + uuid.uuid4().hex)
    try:
        shutil.copy2(src, tmp)
        os.replace(tmp, dest)
    finally:
        try:
            if tmp.exists():
                tmp.unlink()
        except OSError:
            pass


def rollback(root: Path, backup_root: Path, created: Iterable[str], backed_up: Iterable[str]) -> list[str]:
    errors: list[str] = []
    for rel in created:
        dest = local_path(root, rel)
        try:
            if dest.exists() or dest.is_symlink():
                if dest.is_file() or dest.is_symlink():
                    dest.unlink()
        except Exception as exc:
            errors.append(f"delete-created {rel}: {exc}")
    for rel in backed_up:
        src = backup_root.joinpath(*PurePosixPath(rel).parts)
        dest = local_path(root, rel)
        try:
            assert_safe_ancestors(root, dest.parent)
            atomic_copy(src, dest)
        except Exception as exc:
            errors.append(f"restore {rel}: {exc}")
    return errors


def apply_one(root: Path, validation: Validation) -> dict[str, Any]:
    # Applicability is checked before transaction staging. A pure preimage conflict
    # must not consume/archive the transport or claim that rollback was needed.
    check_preconditions(root, validation)

    txid = f"PCCPATCH-{datetime.now().strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:10]}"
    control_root = root / ".project_control"
    stage = control_root / "patch-stage" / txid
    backup_root = control_root / "patch-backups" / txid / "files"
    stage.mkdir(parents=True, exist_ok=True)
    backup_root.mkdir(parents=True, exist_ok=True)

    receipt: dict[str, Any] = {
        "schema": "cortex.patch_receipt.v1",
        "transactionId": txid,
        "patchId": validation.patch_id,
        "title": validation.manifest.get("title"),
        "series": validation.manifest.get("series"),
        "sequence": validation.manifest.get("sequence"),
        "manifestSha256": validation.manifest_sha256,
        "zipSha256": sha256_file(validation.zip_path),
        "sourceZip": str(validation.zip_path),
        "startedUtc": now_utc(),
        "status": "applying",
        "restartRequired": validation.restart_required,
        "files": [],
        "remove": list(validation.remove),
    }
    created: list[str] = []
    backed_up: list[str] = []

    try:
        extract_declared(validation, stage)

        before_evidence: list[dict[str, Any]] = []
        for rel in [spec.path for spec in validation.files] + list(validation.remove):
            info = preimage_info(local_path(root, rel))
            before_evidence.append({"path": rel, **info})
        receipt["before"] = before_evidence

        # Re-check immediately before the live phase to narrow the preimage race window.
        check_preconditions(root, validation)

        # Snapshot every live target that will be overwritten or removed before changing anything.
        targets = [spec.path for spec in validation.files] + list(validation.remove)
        seen: set[str] = set()
        for rel in targets:
            folded = rel.casefold()
            if folded in seen:
                continue
            seen.add(folded)
            dest = local_path(root, rel)
            assert_safe_ancestors(root, dest)
            if dest.exists():
                copy_backup(dest, backup_root.joinpath(*PurePosixPath(rel).parts))
                backed_up.append(rel)
            else:
                created.append(rel)

        for spec in validation.files:
            src = stage.joinpath(*PurePosixPath(spec.path).parts)
            dest = local_path(root, spec.path)
            assert_safe_ancestors(root, dest)
            assert_safe_ancestors(root, dest.parent)
            atomic_copy(src, dest)

        for rel in validation.remove:
            dest = local_path(root, rel)
            assert_safe_ancestors(root, dest)
            if dest.exists():
                dest.unlink()

        # Post-state verification is authoritative.
        file_evidence: list[dict[str, Any]] = []
        for spec in validation.files:
            dest = local_path(root, spec.path)
            if not dest.is_file():
                raise PatchError(f"Post-apply file missing: {spec.path}")
            size = dest.stat().st_size
            digest = sha256_file(dest)
            if size != spec.size or digest != spec.sha256:
                raise PatchError(f"Post-apply verification failed: {spec.path}")
            file_evidence.append({"path": spec.path, "bytes": size, "sha256": digest})
        for rel in validation.remove:
            if local_path(root, rel).exists():
                raise PatchError(f"Post-apply remove verification failed: {rel}")

        receipt["files"] = file_evidence
        receipt["completedUtc"] = now_utc()
        receipt["status"] = "applied"
        archive_transport(root, validation, "applied", receipt)
        receipt_path = receipt_dir(root) / f"{validation.patch_id}.json"
        write_json_atomic(receipt_path, receipt)
        emit("PASS", f"APPLIED: {validation.zip_path.name} [{validation.patch_id}]")
        return receipt
    except Exception as exc:
        rollback_errors = rollback(root, backup_root, created, backed_up)
        receipt["completedUtc"] = now_utc()
        receipt["status"] = "failed"
        receipt["error"] = str(exc)
        receipt["rollbackErrors"] = rollback_errors
        try:
            archive_transport(root, validation, "failed", receipt)
        except Exception as archive_exc:
            receipt["archiveError"] = str(archive_exc)
        failed_receipt = root / "artifacts" / "patches" / "failed" / f"{validation.patch_id}_{txid}.receipt.json"
        write_json_atomic(failed_receipt, receipt)
        raise PatchError(
            f"Patch {validation.patch_id} failed and rollback was attempted: {exc}"
            + (f"; rollback errors: {rollback_errors}" if rollback_errors else "")
        ) from exc
    finally:
        shutil.rmtree(stage, ignore_errors=True)


def scan(root: Path) -> dict[str, Any]:
    roots = [root, root / "updates" / "inbox"]
    candidates: list[Path] = []
    for base in roots:
        if base.is_dir():
            candidates.extend(p for p in base.glob("*.zip") if p.is_file())
    unique = sorted({p.absolute() for p in candidates}, key=lambda p: str(p).lower())

    candidate_validations: list[Validation] = []
    valid: list[Validation] = []
    invalid: list[dict[str, str]] = []
    ignored: list[str] = []
    recovered_sidecars: list[dict[str, str]] = []
    duplicate_transports: list[dict[str, str]] = []
    already_applied_transports: list[dict[str, str]] = []

    applied_records = applied_patch_records(root)
    already = {str(record["patchId"]) for record in applied_records.values()}

    # Phase 1: validate every recognized transport independently.  Do not let filename
    # ordering decide which duplicate patch ID becomes authoritative.
    for path in unique:
        if not looks_like_patch(path):
            ignored.append(str(path))
            continue
        try:
            recovered: dict[str, str] | None = None
            if existing_sidecar(path) is None:
                # First prove the ZIP is internally complete and self-consistent. Only then
                # recover the omitted transport checksum. A present-but-bad sidecar still
                # fails closed through normal validation below.
                validation = validate_patch(path, require_sidecar=False)
                sidecar_path, digest = write_sidecar_atomic(path)
                validation.sidecar_path = sidecar_path
                recovered = {
                    "path": str(path),
                    "name": path.name,
                    "sidecar": str(sidecar_path),
                    "sha256": digest,
                }
            else:
                validation = validate_patch(path, require_sidecar=True)
            candidate_validations.append(validation)
            if recovered is not None:
                recovered_sidecars.append(recovered)
        except Exception as exc:
            invalid.append({"path": str(path), "name": path.name, "error": str(exc)})

    # Phase 2: group by semantic patch ID. Exact byte-for-byte browser/download copies
    # are one transport, not a queue conflict. Divergent payloads sharing an ID fail closed.
    by_id: dict[str, list[Validation]] = {}
    for validation in candidate_validations:
        by_id.setdefault(validation.patch_id.casefold(), []).append(validation)

    for key in sorted(by_id):
        group = by_id[key]
        applied = applied_records.get(key)
        if applied is not None:
            # Replay protection is semantic, not byte-based.  Once a patch ID has an APPLIED
            # receipt, any recognized transport that presents that ID again is invalid.
            # Byte-identical pending browser copies are consumed/archived during the successful
            # apply path so they never survive to become false replay failures on restart.
            known_hashes: set[str] = set(applied.get("hashes", set()))
            for validation in group:
                digest = sha256_file(validation.zip_path)
                detail = "same transport SHA-256" if digest in known_hashes else "different/unknown transport SHA-256"
                invalid.append(
                    {
                        "path": str(validation.zip_path),
                        "name": validation.zip_path.name,
                        "error": f"Patch replay rejected: {validation.patch_id} already has an APPLIED receipt ({detail})",
                    }
                )
            continue

        if len(group) == 1:
            valid.append(group[0])
            continue

        hashes = {sha256_file(validation.zip_path) for validation in group}
        if len(hashes) != 1:
            for validation in group:
                invalid.append(
                    {
                        "path": str(validation.zip_path),
                        "name": validation.zip_path.name,
                        "error": (
                            f"Conflicting pending transports share patch ID {validation.patch_id} "
                            "but have different SHA-256 values"
                        ),
                    }
                )
            continue

        canonical = sorted(group, key=lambda validation: transport_preference_key(root, validation))[0]
        valid.append(canonical)
        canonical_sha = sha256_file(canonical.zip_path)
        for duplicate in group:
            if duplicate is canonical:
                continue
            ignored.append(str(duplicate.zip_path))
            duplicate_transports.append(
                {
                    "patchId": duplicate.patch_id,
                    "path": str(duplicate.zip_path),
                    "name": duplicate.zip_path.name,
                    "sha256": canonical_sha,
                    "canonicalPath": str(canonical.zip_path),
                    "reason": "Byte-identical duplicate transport",
                }
            )

    valid.sort(key=lambda v: v.sort_key)
    valid, dependency_errors = order_by_dependencies(valid, already)
    if dependency_errors:
        invalid.extend(dependency_errors)
        bad_names = {x["path"] for x in dependency_errors}
        valid = [v for v in valid if str(v.zip_path) not in bad_names]

    return {
        "Applied": 0,
        "Pending": len(valid),
        "Invalid": len(invalid),
        "Ignored": len(ignored),
        "RecoveredSidecars": len(recovered_sidecars),
        "RecoveredSidecarDetails": recovered_sidecars,
        "DuplicateTransports": len(duplicate_transports),
        "DuplicateTransportDetails": duplicate_transports,
        "AlreadyAppliedTransports": len(already_applied_transports),
        "AlreadyAppliedTransportDetails": already_applied_transports,
        "RestartRequired": False,
        "Archives": [],
        "ValidPatches": [
            {
                "patchId": v.patch_id,
                "title": v.manifest.get("title"),
                "series": v.manifest.get("series"),
                "sequence": v.manifest.get("sequence"),
                "path": str(v.zip_path),
                "restartRequired": v.restart_required,
            }
            for v in valid
        ],
        "InvalidPatches": invalid,
        "IgnoredZips": ignored,
        "_validations": valid,
    }


def cleanup_transport_residue(root: Path) -> None:
    # Operational cleanup occurs only during an explicit apply, never on startup scan.
    for name in TRANSPORT_NAMES:
        p = root / name
        if p.is_file():
            tracked = False
            git_dir = root / ".git"
            if git_dir.exists() and shutil.which("git"):
                import subprocess

                cp = subprocess.run(
                    ["git", "-C", str(root), "ls-files", "--error-unmatch", "--", name],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                )
                tracked = cp.returncode == 0
            if not tracked:
                try:
                    p.unlink()
                    emit("WARN", f"Removed untracked root patch-transport residue: {name}")
                except OSError:
                    pass


def process_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        if os.name == "nt":
            import ctypes
            PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
            handle = ctypes.windll.kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
            if handle:
                ctypes.windll.kernel32.CloseHandle(handle)
                return True
            return False
        os.kill(pid, 0)
        return True
    except Exception:
        return False


def lock_owner(path: Path) -> tuple[int | None, float | None]:
    try:
        age = time.time() - path.stat().st_mtime
        data = json.loads(path.read_text(encoding="utf-8"))
        pid = int(data.get("pid")) if data.get("pid") is not None else None
        return pid, age
    except Exception:
        return None, None


class ApplyLock:
    def __init__(self, root: Path) -> None:
        self.path = root / ".project_control" / "patch-intake.lock"
        self.fd: int | None = None

    def __enter__(self) -> "ApplyLock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        if self.path.exists():
            pid, age = lock_owner(self.path)
            stale = (pid is not None and not process_alive(pid)) or (pid is None and age is not None and age > 4 * 60 * 60)
            if stale:
                try:
                    self.path.unlink()
                    emit("WARN", f"Removed stale patch-intake lock (pid={pid}, age={age}).")
                except OSError:
                    pass
        try:
            self.fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError as exc:
            raise PatchError(f"Another patch intake appears active: {self.path}") from exc
        payload = json.dumps({"pid": os.getpid(), "createdUtc": now_utc()}) + "\n"
        os.write(self.fd, payload.encode("utf-8"))
        os.fsync(self.fd)
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
        try:
            self.path.unlink()
        except OSError:
            pass


def public_summary(summary: dict[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in summary.items() if not k.startswith("_")}


def do_scan(root: Path) -> int:
    summary = scan(root)
    emit("INFO", "START Root incremental patch scan")
    for item in summary.get("RecoveredSidecarDetails", []) or []:
        emit("WARN", f"RECOVERED SHA-256 SIDECAR: {item['name']} -> {Path(item['sidecar']).name}")
    for item in summary.get("DuplicateTransportDetails", []) or []:
        emit("WARN", f"IGNORED BYTE-IDENTICAL DUPLICATE: {item['name']} [{item['patchId']}]")
    for item in summary.get("AlreadyAppliedTransportDetails", []) or []:
        emit("WARN", f"IGNORED ALREADY-APPLIED TRANSPORT: {item['name']} [{item['patchId']}]")
    if summary["Pending"]:
        for p in summary["ValidPatches"]:
            emit("INFO", f"VALID PATCH: {p['patchId']} - {Path(p['path']).name}")
    if summary["Invalid"]:
        for p in summary["InvalidPatches"]:
            emit("FAIL", f"INVALID PATCH: {p['name']} - {p['error']}")
    if not summary["Pending"] and not summary["Invalid"]:
        emit("PASS", "Root patch scan: no pending patch ZIPs detected.")
    result_line(public_summary(summary))
    return 0 if summary["Invalid"] == 0 else 2


def archive_redundant_pending_transports(root: Path, patch_id: str, details: Iterable[dict[str, str]]) -> list[str]:
    """Archive byte-identical pending copies after the canonical transport is applied.

    This preserves strict replay protection while also handling browser-created `(1).zip`
    copies safely.  We only receive entries that scan already proved byte-identical to the
    canonical pending patch.
    """
    archived: list[str] = []
    folder = root / "artifacts" / "patches" / "applied" / "redundant-transports"
    folder.mkdir(parents=True, exist_ok=True)
    for item in details:
        if str(item.get("patchId") or "").casefold() != patch_id.casefold():
            continue
        raw = str(item.get("path") or "").strip()
        if not raw:
            continue
        path = Path(raw)
        if not path.is_file():
            continue
        target = folder / f"{stamp()}_{path.name}"
        if target.exists():
            target = folder / f"{stamp()}_{uuid.uuid4().hex[:8]}_{path.name}"
        shutil.move(str(path), str(target))
        for suffix in (".sha256", ".sha256.txt"):
            side = Path(str(path) + suffix)
            if side.is_file():
                shutil.move(str(side), str(Path(str(target) + suffix)))
        archived.append(str(target))
        emit("WARN", f"ARCHIVED REDUNDANT TRANSPORT: {path.name} [{patch_id}]")
    return archived


def do_apply(root: Path) -> int:
    emit("INFO", "START Root incremental patch intake")
    with ApplyLock(root):
        return _do_apply_locked(root)


def _do_apply_locked(root: Path) -> int:
    cleanup_transport_residue(root)
    summary = scan(root)
    if summary["Invalid"]:
        for p in summary["InvalidPatches"]:
            emit("FAIL", f"INVALID PATCH: {p['name']} - {p['error']}")
        emit("FAIL", "Patch queue is fail-closed: no patches were applied because invalid recognized patches exist.")
        result_line(public_summary(summary))
        return 2
    validations: list[Validation] = summary["_validations"]
    if not validations:
        emit("PASS", "Root patch intake: no pending patch ZIPs detected.")
        result_line(public_summary(summary))
        return 0

    applied: list[dict[str, Any]] = []
    archives: list[str] = []
    restart_required = False
    already = applied_patch_ids(root)
    for validation in validations:
        already_folded = {x.casefold() for x in already}
        missing = [d for d in (validation.manifest.get("dependsOn", []) or []) if d.casefold() not in already_folded]
        if missing:
            emit("FAIL", f"Dependency not yet applied for {validation.patch_id}: {', '.join(missing)}")
            break
        emit("INFO", f"PATCH DETECTED: {validation.zip_path.name} [{validation.patch_id}]")
        try:
            receipt = apply_one(root, validation)
        except PatchError as exc:
            emit("FAIL", str(exc))
            out = {
                "Applied": len(applied),
                "Pending": max(0, len(validations) - len(applied)),
                "Invalid": 1,
                "Ignored": summary["Ignored"],
                "RestartRequired": restart_required,
                "Archives": archives,
                "ValidPatches": summary["ValidPatches"],
                "InvalidPatches": [{"name": validation.zip_path.name, "path": str(validation.zip_path), "error": str(exc)}],
                "IgnoredZips": summary["IgnoredZips"],
            }
            result_line(out)
            return 1
        applied.append(receipt)
        already.add(validation.patch_id)
        if receipt.get("archivePath"):
            archives.append(str(receipt["archivePath"]))
        archives.extend(archive_redundant_pending_transports(
            root, validation.patch_id, summary.get("DuplicateTransportDetails", []) or []
        ))
        if validation.restart_required:
            restart_required = True
            emit("WARN", "Control-center files changed; stopping patch queue until the Root Utility relaunches.")
            break

    remaining = len(validations) - len(applied)
    out = {
        "Applied": len(applied),
        "Pending": max(0, remaining),
        "Invalid": 0,
        "Ignored": summary["Ignored"],
        "RestartRequired": restart_required,
        "Archives": archives,
        "ValidPatches": summary["ValidPatches"],
        "InvalidPatches": [],
        "IgnoredZips": summary["IgnoredZips"],
    }
    emit("PASS", f"Root patch intake complete: {len(applied)} applied, {max(0, remaining)} remaining.")
    result_line(out)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Cortex manifest-authoritative root patch authority.")
    parser.add_argument("action", choices=["scan", "apply"])
    parser.add_argument("--root", required=True)
    args = parser.parse_args()
    root = Path(args.root).absolute()
    if not root.is_dir():
        raise PatchError(f"Project root does not exist: {root}")
    if args.action == "scan":
        return do_scan(root)
    return do_apply(root)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PatchError as exc:
        emit("FAIL", str(exc))
        result_line(
            {
                "Applied": 0,
                "Pending": 0,
                "Invalid": 1,
                "Ignored": 0,
                "RestartRequired": False,
                "Archives": [],
                "ValidPatches": [],
                "InvalidPatches": [{"name": "<authority>", "path": "", "error": str(exc)}],
                "IgnoredZips": [],
            }
        )
        raise SystemExit(1)
