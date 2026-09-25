#!/usr/bin/env python3
"""Read-only failure classifier for universal PCC/Cortex repair routing."""
from __future__ import annotations

import argparse
import json
import os
import re
from pathlib import Path
from typing import Any, Iterable

DOCTOR_VERSION = "PCC-BUILD-DOCTOR-0.3"
MAX_LOG_BYTES = 512 * 1024


def _tail_text(path: Path, limit: int = MAX_LOG_BYTES) -> str:
    try:
        size = path.stat().st_size
        with path.open("rb") as fh:
            if size > limit:
                fh.seek(-limit, 2)
            data = fh.read(limit)
        return data.decode("utf-8", errors="replace")
    except Exception:
        return ""


def _latest_logs(root: Path, limit: int = 12) -> list[Path]:
    log_root = root / "artifacts" / "logs"
    if not log_root.is_dir():
        return []
    rows: list[Path] = []
    for pattern in ("*.log", "*.txt", "*.jsonl"):
        rows.extend(path for path in log_root.rglob(pattern) if path.is_file())
    try:
        rows.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    except OSError:
        pass
    return rows[:limit]


def _find_failed_stage(text: str) -> str:
    patterns = [
        r"QUICK PROJECT GATE FAILED at ([A-Za-z0-9_.-]+)",
        r"failedStage[\"'=:\s]+([A-Za-z0-9_.-]+)",
        r"\[FAIL\].*? at ([A-Za-z0-9_.-]+)",
        r"=== END ([A-Za-z0-9_.-]+): FAIL",
    ]
    for pattern in patterns:
        match = re.search(pattern, text, flags=re.I)
        if match:
            return match.group(1)
    return ""


def _terminal_gate(text: str) -> dict[str, str] | None:
    """Return the last authoritative QUICK/FULL terminal state in one log."""
    candidates: list[tuple[int, str, str]] = []
    for match in re.finditer(r"=== END\s+(full|quick|fast):\s+(PASS|FAIL(?:\s*\([^\n]*\))?)", text, flags=re.I):
        candidates.append((match.start(), match.group(1).lower(), "PASS" if match.group(2).upper().startswith("PASS") else "FAIL"))
    for match in re.finditer(r"FULL QUALITY GATE GREEN / SOURCE CERTIFIED", text, flags=re.I):
        candidates.append((match.start(), "full", "PASS"))
    for match in re.finditer(r"QUICK PROJECT GATE GREEN", text, flags=re.I):
        candidates.append((match.start(), "quick", "PASS"))
    for match in re.finditer(r"QUICK PROJECT GATE FAILED", text, flags=re.I):
        candidates.append((match.start(), "quick", "FAIL"))
    if not candidates:
        return None
    _, gate, status = max(candidates, key=lambda row: row[0])
    return {"gate": gate, "status": status}


def classify_failure(text: str, *, stage: str = "", exit_code: int | None = None) -> dict[str, Any]:
    low = text.casefold()
    stage_low = stage.casefold()

    category = "UNKNOWN"
    code = "unknown_failure"
    source_implicated = False
    repair_scope = "diagnose"
    capability = ""
    confidence = "low"

    if stage_low == "patch-authority" or "invalid recognized patch" in low or "unsupported patch schema" in low:
        category, code, repair_scope, confidence = "PATCH", "patch_transport_classification", "patch_queue", "high"
    elif "dubious ownership" in low or "safe.directory" in low:
        category, code, repair_scope, confidence = "GIT", "git_checkout_trust", "git_checkout", "high"
    elif any(token in low for token in ("authentication failed", "could not read username", "permission denied (publickey)", "waiting_for_auth")):
        category, code, repair_scope, confidence = "AUTH", "source_control_auth", "authentication", "high"
    elif any(token in low for token in (
        "link.exe/cl.exe missing", "link.exe missing", "cl.exe missing",
        "msvc amd64 developer environment is not active", "windows linker toolchain",
    )):
        category, code, repair_scope, capability, confidence = (
            "ENVIRONMENT", "windows_msvc_inactive", "toolchain_environment", "windows.msvc.amd64", "high"
        )
    elif any(token in low for token in ("cargo not found", "rustc not found", "python 3.11", "cmake not found", "node not found")):
        category, code, repair_scope, confidence = "ENVIRONMENT", "required_tool_missing", "toolchain_environment", "high"
    elif "timed out" in low or "timeout" in low or "operation host" in low and "cancellation requested" in low:
        category, code, repair_scope, confidence = "CONTROL_PLANE", "operation_timeout_or_stall", "pcc_control_plane", "medium"
    elif any(token in low for token in ("failed to download", "dependency resolution", "could not resolve", "package not found", "failed to get `")):
        category, code, repair_scope, confidence = "DEPENDENCY", "dependency_resolution", "dependencies", "medium"
    elif "test result: failed" in low or stage_low.startswith("cargo-test") or stage_low == "test":
        category, code, repair_scope, source_implicated, confidence = "TEST", "test_failure", "source_or_test", True, "high"
    elif "clippy" in stage_low or "cargo-clippy" in low:
        category, code, repair_scope, source_implicated, confidence = "SOURCE_LINT", "lint_failure", "source", True, "high"
    elif "cargo fmt" in low or "cargo-fmt" in stage_low or "rustfmt" in low:
        category, code, repair_scope, source_implicated, confidence = "SOURCE_FORMAT", "format_failure", "source", True, "high"
    elif re.search(r"\berror\[e\d{4}\]", low) or "could not compile" in low or stage_low in {"cargo-check", "cargo-build", "build"}:
        category, code, repair_scope, source_implicated, confidence = "SOURCE_COMPILE", "compile_failure", "source", True, "medium"
    elif "panic" in low or "runtime verify" in low or stage_low.startswith("runtime"):
        category, code, repair_scope, source_implicated, confidence = "RUNTIME", "runtime_failure", "runtime_or_source", True, "medium"

    return {
        "schema": "pcc.failure_classification.v1",
        "doctorVersion": DOCTOR_VERSION,
        "category": category,
        "code": code,
        "stage": stage,
        "exitCode": exit_code,
        "sourceImplicated": source_implicated,
        "repairScope": repair_scope,
        "capability": capability or None,
        "confidence": confidence,
    }


def _blockers(text: str) -> list[dict[str, Any]]:
    low = text.casefold()
    rows: list[dict[str, Any]] = []

    def add(category: str, code: str, scope: str, capability: str | None = None) -> None:
        item = {"category": category, "code": code, "repairScope": scope}
        if capability:
            item["capability"] = capability
        if item not in rows:
            rows.append(item)

    if "invalid recognized patch" in low or "unsupported patch schema" in low:
        add("PATCH", "patch_transport_classification", "patch_queue")
    if any(token in low for token in ("link.exe/cl.exe missing", "msvc amd64 developer environment is not active", "msvc=activation_failed", "windows linker toolchain")):
        add("ENVIRONMENT", "windows_msvc_inactive", "toolchain_environment", "windows.msvc.amd64")
    if "dubious ownership" in low or "safe.directory" in low:
        add("GIT", "git_checkout_trust", "git_checkout")
    if any(token in low for token in ("authentication failed", "could not read username", "permission denied (publickey)", "waiting_for_auth")):
        add("AUTH", "source_control_auth", "authentication")
    return rows


def _marker_mtime(root: Path) -> float:
    marker = root / ".cortex" / "last-green-quality-gate.json"
    try:
        return marker.stat().st_mtime
    except OSError:
        return 0.0


def latest_failure(root: Path) -> dict[str, Any]:
    project_root = root.expanduser().resolve()
    logs = _latest_logs(project_root)
    evidence: list[dict[str, Any]] = []
    combined_parts: list[str] = []
    selected: Path | None = None
    selected_text = ""
    latest_terminal: dict[str, Any] | None = None

    for path in logs:
        text = _tail_text(path)
        if not text:
            continue
        try:
            mtime = path.stat().st_mtime
            size = path.stat().st_size
        except OSError:
            mtime = 0.0
            size = 0
        evidence.append({"path": str(path), "bytesInspected": min(size, MAX_LOG_BYTES), "modifiedUnix": mtime})
        terminal = _terminal_gate(text)
        if terminal is not None:
            candidate = {**terminal, "path": str(path), "modifiedUnix": mtime}
            if latest_terminal is None or mtime > float(latest_terminal.get("modifiedUnix") or 0.0):
                latest_terminal = candidate
        if selected is None and ("[FAIL]" in text or "=== END" in text and "FAIL" in text or "Cancellation requested" in text):
            selected = path
            selected_text = text
        combined_parts.append(text)

    combined = "\n".join(combined_parts)
    if not combined:
        return {
            "schema": "pcc.failure_doctor.v1",
            "doctorVersion": DOCTOR_VERSION,
            "projectRoot": str(project_root),
            "status": "NO_EVIDENCE",
            "classification": classify_failure(""),
            "evidence": [],
        }

    # A newer successful terminal gate or GREEN marker invalidates older failure
    # evidence.  This keeps Repair Coordinator from repairing stale history.
    marker_time = _marker_mtime(project_root)
    failure_time = 0.0
    if selected is not None:
        try:
            failure_time = selected.stat().st_mtime
        except OSError:
            failure_time = 0.0
    if latest_terminal and latest_terminal.get("status") == "PASS" and float(latest_terminal.get("modifiedUnix") or 0.0) >= failure_time:
        return {
            "schema": "pcc.failure_doctor.v1",
            "doctorVersion": DOCTOR_VERSION,
            "projectRoot": str(project_root),
            "status": "NO_ACTIVE_FAILURE",
            "latestGate": latest_terminal,
            "greenMarkerNewerThanFailure": bool(marker_time >= failure_time and marker_time > 0),
            "classification": classify_failure(""),
            "blockers": [],
            "evidence": evidence,
            "excerpt": [],
        }
    if marker_time > 0 and marker_time >= failure_time and selected is not None:
        return {
            "schema": "pcc.failure_doctor.v1",
            "doctorVersion": DOCTOR_VERSION,
            "projectRoot": str(project_root),
            "status": "NO_ACTIVE_FAILURE",
            "latestGate": latest_terminal,
            "greenMarkerNewerThanFailure": True,
            "classification": classify_failure(""),
            "blockers": [],
            "evidence": evidence,
            "excerpt": [],
        }

    primary_text = selected_text or combined
    stage = _find_failed_stage(primary_text)
    classification = classify_failure(primary_text, stage=stage)
    blockers = _blockers(combined)
    status = "FAILURE_CLASSIFIED" if classification["category"] != "UNKNOWN" else "FAILURE_UNCLASSIFIED"
    excerpt_lines = [line for line in primary_text.splitlines() if "FAIL" in line.upper() or "BLOCK" in line.upper() or "CANCEL" in line.upper()]
    return {
        "schema": "pcc.failure_doctor.v1",
        "doctorVersion": DOCTOR_VERSION,
        "projectRoot": str(project_root),
        "status": status,
        "selectedLog": str(selected) if selected else None,
        "latestGate": latest_terminal,
        "greenMarkerNewerThanFailure": False,
        "classification": classification,
        "blockers": blockers,
        "evidence": evidence,
        "excerpt": excerpt_lines[-24:],
    }


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Read-only PCC build failure classifier")
    parser.add_argument("--root", required=True)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)
    payload = latest_failure(Path(args.root))
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        cls = payload.get("classification") or {}
        print(f"PCC BUILD DOCTOR {DOCTOR_VERSION}")
        print(f"Status       : {payload.get('status')}")
        if payload.get("status") == "NO_ACTIVE_FAILURE":
            latest_gate = payload.get("latestGate") or {}
            if latest_gate:
                print(f"Latest gate  : {latest_gate.get('gate')} {latest_gate.get('status')}")
            print("Repair scope : none")
            print("Source       : no active failure to repair")
            return 0
        print(f"Category     : {cls.get('category')}")
        print(f"Code         : {cls.get('code')}")
        print(f"Stage        : {cls.get('stage') or '<unknown>'}")
        print(f"Repair scope : {cls.get('repairScope')}")
        print(f"Source       : {'implicated' if cls.get('sourceImplicated') else 'not implicated'}")
        if cls.get("capability"):
            print(f"Capability   : {cls.get('capability')}")
        for line in payload.get("excerpt") or []:
            print(f"  {line}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
