#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import time
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

from CortexRepositoryLayout import scan_repository_layout

MAINTENANCE_SCHEMA = "cortex.pcc_maintenance.v1"
DEBUG_MANIFEST_SCHEMA = "cortex.pcc_debug_manifest.v1"
LATEST_DEBUG_SCHEMA = "cortex.pcc_latest_debug.v1"


class MaintenanceError(RuntimeError):
    pass


def now_utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def local_stamp() -> str:
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def atomic_write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + f".tmp-{os.getpid()}-{time.time_ns()}")
    try:
        with tmp.open("w", encoding="utf-8", newline="\n") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    finally:
        try:
            tmp.unlink()
        except FileNotFoundError:
            pass


def atomic_write_json(path: Path, payload: Any) -> None:
    atomic_write_text(path, json.dumps(payload, indent=2, sort_keys=True) + "\n")


def process_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        if os.name == "nt":
            import ctypes

            process_query_limited_information = 0x1000
            handle = ctypes.windll.kernel32.OpenProcess(process_query_limited_information, False, pid)
            if handle:
                ctypes.windll.kernel32.CloseHandle(handle)
                return True
            return False
        os.kill(pid, 0)
        return True
    except Exception:
        return False


def _read_lock(path: Path) -> tuple[int | None, float | None]:
    try:
        age = time.time() - path.stat().st_mtime
        data = json.loads(path.read_text(encoding="utf-8"))
        pid = int(data.get("pid")) if data.get("pid") is not None else None
        return pid, age
    except Exception:
        return None, None


class OperationLock:
    """One mutating PCC operation at a time; stale locks are reclaimed by PID/age."""

    def __init__(self, root: Path, operation: str, *, stale_seconds: float = 4 * 60 * 60) -> None:
        self.root = root.resolve()
        self.operation = operation
        self.stale_seconds = stale_seconds
        self.path = self.root / ".project_control" / "pcc-operation.lock"
        self.fd: int | None = None

    def __enter__(self) -> "OperationLock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        if self.path.exists():
            pid, age = _read_lock(self.path)
            stale = (pid is not None and not process_alive(pid)) or (
                pid is None and age is not None and age > self.stale_seconds
            )
            if stale:
                try:
                    self.path.unlink()
                except OSError:
                    pass
        try:
            self.fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError as exc:
            pid, age = _read_lock(self.path)
            raise MaintenanceError(
                f"Another PCC operation is active (pid={pid}, age={age}, lock={self.path})."
            ) from exc
        payload = {
            "schema": "cortex.pcc_operation_lock.v1",
            "pid": os.getpid(),
            "operation": self.operation,
            "createdUtc": now_utc(),
        }
        os.write(self.fd, (json.dumps(payload, sort_keys=True) + "\n").encode("utf-8"))
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


@dataclass(frozen=True)
class Residue:
    path: Path
    relative: str
    kind: str
    destination_group: str
    severity: str


def _root_file_matches(path: Path) -> tuple[str, str] | None:
    name = path.name
    folded = name.casefold()
    if re.fullmatch(r"latest_debug_bundle(?:\([^)]*\)|\([^)]*\)|[^/]*)?\.txt", folded):
        return "legacy-debug-pointer", "debug"
    if folded.startswith("latest_debug_bundle") and folded.endswith(".txt"):
        return "legacy-debug-pointer", "debug"
    if folded.startswith("cortex_debugbundle_") and folded.endswith(".zip"):
        return "root-debug-bundle", "debug"
    if re.fullmatch(r"cortex-root-.*\.(?:log|transcript\.log)", folded):
        return "legacy-root-log", "logs"
    if re.fullmatch(r"cortex-pcc-.*\.(?:log|jsonl)", folded):
        return "legacy-pcc-log", "logs"
    if folded in {"latest_control_center.log", "latest_root_session.txt"}:
        return "legacy-control-evidence", "logs"
    return None


def scan_root_hygiene(root: Path) -> dict[str, Any]:
    root = root.resolve()
    residues: list[Residue] = []
    for path in sorted(root.iterdir(), key=lambda p: p.name.casefold()):
        if not path.is_file():
            continue
        matched = _root_file_matches(path)
        if matched:
            kind, group = matched
            residues.append(Residue(path, path.name, kind, group, "violation"))

    legacy_sessions = root / "logs" / "sessions"
    if legacy_sessions.is_dir():
        for path in sorted(legacy_sessions.iterdir(), key=lambda p: p.name.casefold()):
            if not path.is_file():
                continue
            folded = path.name.casefold()
            if folded.startswith("cortex-pcc-") or folded.startswith("cortex-root-"):
                residues.append(
                    Residue(path, path.relative_to(root).as_posix(), "legacy-log-location", "logs", "advisory")
                )

    layout: dict[str, Any] | None = None
    policy = root / "config" / "cortex" / "repository_layout.v1.json"
    if policy.is_file():
        try:
            layout = scan_repository_layout(root)
        except Exception as exc:
            layout = {
                "schema": "cortex.repository_layout.scan.v1",
                "projectRoot": str(root),
                "clean": False,
                "violationCount": 1,
                "items": [{"path": str(policy.relative_to(root)), "kind": "layout-policy-error",
                           "severity": "violation", "error": str(exc), "archiveEligible": False}],
            }

    seen = {r.relative.casefold() for r in residues}
    if layout is not None:
        for item in layout.get("items", []) or []:
            rel = str(item.get("path") or "").rstrip("/")
            if not rel or rel.casefold() in seen:
                continue
            kind = str(item.get("kind") or "repository-layout")
            if kind == "runtime-in-repository":
                group = "runtime-state"
            elif bool(item.get("archiveEligible")):
                group = "layout-archive"
            else:
                group = "layout-blocked"
            residues.append(Residue(root / rel, str(item.get("path") or rel), kind, group, "violation"))
            seen.add(rel.casefold())

    violations = [r for r in residues if r.severity == "violation"]
    advisories = [r for r in residues if r.severity == "advisory"]
    return {
        "schema": "cortex.pcc_root_hygiene.v2",
        "projectRoot": str(root),
        "clean": len(violations) == 0,
        "violationCount": len(violations),
        "advisoryCount": len(advisories),
        "layout": layout,
        "items": [
            {
                "path": r.relative,
                "kind": r.kind,
                "destinationGroup": r.destination_group,
                "severity": r.severity,
            }
            for r in residues
        ],
    }


def _unique_destination(path: Path) -> Path:
    if not path.exists():
        return path
    stem, suffix = path.stem, path.suffix
    for i in range(1, 10_000):
        candidate = path.with_name(f"{stem}_{i}{suffix}")
        if not candidate.exists():
            return candidate
    raise MaintenanceError(f"Could not allocate unique destination for {path}")


def repair_root_hygiene(root: Path) -> dict[str, Any]:
    root = root.resolve()
    before = scan_root_hygiene(root)
    moved: list[dict[str, Any]] = []
    runtime_migration: dict[str, Any] = {"migrated": 0, "invalid": 0}
    volume_runtime_migration: dict[str, Any] = {"migrated": 0, "actions": []}

    # PCC presentation-cache state is project-local. The migration is loss-averse:
    # malformed JSON remains in place so repository hygiene stays RED rather than
    # silently discarding evidence that could not be read.
    try:
        from PCCChatStore import migrate_legacy_conversations
        runtime_migration = migrate_legacy_conversations(root)
    except Exception as exc:
        runtime_migration = {"migrated": 0, "invalid": 0, "error": str(exc)}

    # Legacy global Cortex runtime roots (Memory/Search/Tasks) belong to the marked
    # portable volume, not inside the source repository. Copy and hash-verify every
    # destination before deleting any legacy source bytes. A collision fails closed
    # and leaves the source tree RED for explicit inspection.
    try:
        from CortexRepositoryConvergence import migrate_runtime_state
        volume_runtime_migration = migrate_runtime_state(root, scopes=("volume",))
    except Exception as exc:
        volume_runtime_migration = {"migrated": 0, "actions": [], "error": str(exc)}

    for item in before["items"]:
        src = root / str(item["path"]).rstrip("/")
        if not src.is_file():
            continue
        group = item["destinationGroup"]
        if group == "debug" and item["kind"] == "root-debug-bundle":
            dest_dir = root / "artifacts" / "debug"
        elif group == "logs":
            dest_dir = root / "artifacts" / "logs" / "sessions"
        elif group == "layout-archive":
            dest_dir = root / "artifacts" / "maintenance" / "legacy-root-source"
        elif group == "layout-blocked" or group == "runtime-state":
            continue
        else:
            dest_dir = root / "artifacts" / "maintenance" / "legacy-root-residue"
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = _unique_destination(dest_dir / src.name)
        shutil.move(str(src), str(dest))
        moved.append(
            {
                "from": item["path"],
                "to": dest.relative_to(root).as_posix(),
                "bytes": dest.stat().st_size,
                "sha256": sha256_file(dest),
            }
        )

    after = scan_root_hygiene(root)
    receipt = {
        "schema": MAINTENANCE_SCHEMA,
        "operation": "root-hygiene-repair",
        "createdUtc": now_utc(),
        "before": {"violationCount": before["violationCount"], "advisoryCount": before["advisoryCount"]},
        "after": {"violationCount": after["violationCount"], "advisoryCount": after["advisoryCount"]},
        "runtimeMigration": runtime_migration,
        "volumeRuntimeMigration": volume_runtime_migration,
        "moved": moved,
    }
    receipt_dir = root / "artifacts" / "maintenance" / "receipts"
    receipt_path = receipt_dir / f"root-hygiene-{local_stamp()}-{time.time_ns() % 1_000_000:06d}.json"
    atomic_write_json(receipt_path, receipt)
    receipt["receipt"] = str(receipt_path)
    return receipt


def debug_manifest_for_tree(work: Path) -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    for path in sorted(work.rglob("*"), key=lambda p: p.as_posix().casefold()):
        if not path.is_file() or path.name == "MANIFEST.json":
            continue
        entries.append(
            {
                "path": path.relative_to(work).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )
    return {
        "schema": DEBUG_MANIFEST_SCHEMA,
        "createdUtc": now_utc(),
        "fileCount": len(entries),
        "files": entries,
    }


def verify_debug_bundle(zip_path: Path) -> dict[str, Any]:
    if not zip_path.is_file():
        raise MaintenanceError(f"Debug bundle does not exist: {zip_path}")
    with zipfile.ZipFile(zip_path, "r") as zf:
        names = [i.filename for i in zf.infolist() if not i.is_dir()]
        if "MANIFEST.json" not in names:
            raise MaintenanceError("Debug bundle is missing MANIFEST.json")
        if len({n.casefold() for n in names}) != len(names):
            raise MaintenanceError("Debug bundle contains duplicate case-insensitive paths")
        manifest = json.loads(zf.read("MANIFEST.json").decode("utf-8-sig"))
        if manifest.get("schema") != DEBUG_MANIFEST_SCHEMA:
            raise MaintenanceError(f"Unsupported debug bundle manifest schema: {manifest.get('schema')!r}")
        specs = manifest.get("files")
        if not isinstance(specs, list):
            raise MaintenanceError("Debug bundle manifest files must be an array")
        expected = {str(x.get("path", "")): x for x in specs if isinstance(x, dict)}
        actual = set(names) - {"MANIFEST.json"}
        if set(expected) != actual:
            raise MaintenanceError("Debug bundle manifest file set does not match ZIP contents")
        for rel, spec in expected.items():
            data = zf.read(rel)
            if len(data) != int(spec.get("bytes", -1)):
                raise MaintenanceError(f"Debug bundle byte mismatch: {rel}")
            digest = hashlib.sha256(data).hexdigest()
            if digest != str(spec.get("sha256", "")).lower():
                raise MaintenanceError(f"Debug bundle SHA-256 mismatch: {rel}")
    return {
        "schema": "cortex.pcc_debug_verify.v1",
        "path": str(zip_path),
        "bytes": zip_path.stat().st_size,
        "sha256": sha256_file(zip_path),
        "verified": True,
        "fileCount": len(expected),
    }


def write_debug_sidecar(zip_path: Path) -> Path:
    sidecar = Path(str(zip_path) + ".sha256")
    atomic_write_text(sidecar, f"{sha256_file(zip_path)}  {zip_path.name}\n")
    return sidecar


def write_latest_debug_pointer(debug_dir: Path, zip_path: Path, *, reason: str, exit_code: int,
                               failed_stage: str, verification: dict[str, Any]) -> tuple[Path, Path]:
    debug_dir.mkdir(parents=True, exist_ok=True)
    text_path = debug_dir / "LATEST_DEBUG_BUNDLE.txt"
    json_path = debug_dir / "LATEST_DEBUG_BUNDLE.json"
    payload = {
        "schema": LATEST_DEBUG_SCHEMA,
        "createdUtc": now_utc(),
        "path": str(zip_path),
        "name": zip_path.name,
        "reason": reason,
        "exitCode": exit_code,
        "failedStage": failed_stage or None,
        "sha256": verification.get("sha256"),
        "bytes": verification.get("bytes"),
        "verified": bool(verification.get("verified")),
    }
    atomic_write_json(json_path, payload)
    atomic_write_text(
        text_path,
        "".join(
            [
                f"Path={zip_path}\n",
                f"Created={payload['createdUtc']}\n",
                f"Reason={reason}\n",
                f"ExitCode={exit_code}\n",
                f"FailedStage={failed_stage}\n",
                f"SHA256={payload['sha256']}\n",
                f"Verified={str(payload['verified']).lower()}\n",
            ]
        ),
    )
    return text_path, json_path


def _debug_zip_candidates(root: Path) -> list[Path]:
    d = root / "artifacts" / "debug"
    if not d.is_dir():
        return []
    return sorted(
        [p for p in d.glob("Cortex_DebugBundle_*.zip") if p.is_file()],
        key=lambda p: (p.stat().st_mtime_ns, p.name.casefold()),
        reverse=True,
    )


def _session_log_candidates(root: Path) -> list[Path]:
    d = root / "artifacts" / "logs" / "sessions"
    if not d.is_dir():
        return []
    return sorted(
        [p for p in d.iterdir() if p.is_file() and p.name.startswith("cortex-pcc-PCC-")],
        key=lambda p: (p.stat().st_mtime_ns, p.name.casefold()),
        reverse=True,
    )


def retention_plan(root: Path, *, keep_debug: int = 30, keep_log_files: int = 200) -> dict[str, Any]:
    if keep_debug < 1 or keep_log_files < 2:
        raise MaintenanceError("Retention counts must preserve at least one debug bundle and two session-log files.")
    debug = _debug_zip_candidates(root)
    logs = _session_log_candidates(root)
    delete_debug = debug[keep_debug:]
    delete_logs = logs[keep_log_files:]
    sidecars: list[Path] = []
    for zp in delete_debug:
        sc = Path(str(zp) + ".sha256")
        if sc.is_file():
            sidecars.append(sc)
    targets = [*delete_debug, *sidecars, *delete_logs]
    return {
        "schema": "cortex.pcc_retention_plan.v1",
        "keepDebug": keep_debug,
        "keepLogFiles": keep_log_files,
        "deleteCount": len(targets),
        "reclaimBytes": sum(p.stat().st_size for p in targets if p.is_file()),
        "targets": [str(p) for p in targets],
    }


def prune_artifacts(root: Path, *, keep_debug: int = 30, keep_log_files: int = 200,
                    apply: bool = False) -> dict[str, Any]:
    plan = retention_plan(root, keep_debug=keep_debug, keep_log_files=keep_log_files)
    deleted: list[str] = []
    if apply:
        for raw in plan["targets"]:
            path = Path(raw)
            try:
                path.unlink()
                deleted.append(str(path))
            except FileNotFoundError:
                pass
        receipt = {
            "schema": MAINTENANCE_SCHEMA,
            "operation": "artifact-prune",
            "createdUtc": now_utc(),
            "plan": plan,
            "deleted": deleted,
        }
        receipt_path = root / "artifacts" / "maintenance" / "receipts" / f"artifact-prune-{local_stamp()}-{time.time_ns() % 1_000_000:06d}.json"
        atomic_write_json(receipt_path, receipt)
        plan["receipt"] = str(receipt_path)
    plan["applied"] = apply
    plan["deleted"] = deleted
    return plan


def doctor(root: Path) -> dict[str, Any]:
    root = root.resolve()
    hygiene = scan_root_hygiene(root)
    usage = shutil.disk_usage(root)
    op_lock = root / ".project_control" / "pcc-operation.lock"
    lock_info: dict[str, Any] | None = None
    if op_lock.exists():
        pid, age = _read_lock(op_lock)
        lock_info = {"path": str(op_lock), "pid": pid, "ageSeconds": age, "processAlive": process_alive(pid or -1)}
    debug = _debug_zip_candidates(root)
    latest_json = root / "artifacts" / "debug" / "LATEST_DEBUG_BUNDLE.json"
    latest: dict[str, Any] | None = None
    if latest_json.is_file():
        try:
            latest = json.loads(latest_json.read_text(encoding="utf-8"))
        except Exception as exc:
            latest = {"error": str(exc)}
    return {
        "schema": "cortex.pcc_doctor.v1",
        "createdUtc": now_utc(),
        "projectRoot": str(root),
        "hygiene": hygiene,
        "disk": {"total": usage.total, "used": usage.used, "free": usage.free},
        "operationLock": lock_info,
        "artifacts": {
            "debugBundleCount": len(debug),
            "sessionLogFileCount": len(_session_log_candidates(root)),
            "latestDebug": latest,
        },
        "healthy": bool(hygiene["clean"]) and (usage.free >= 512 * 1024 * 1024),
    }
