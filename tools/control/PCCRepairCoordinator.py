#!/usr/bin/env python3
"""Deterministic PCC failure-repair coordinator.

The coordinator is intentionally conservative:
- plan is read-only;
- apply requires explicit --yes;
- source-affecting repairs require the prebuilt transactional Cortex worker;
- all source mutations stay inside the Cortex source transaction;
- targeted validation and QUICK must pass before commit;
- optional FULL certification must pass before commit when requested;
- failed validation rolls the transaction back automatically.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Iterable, Mapping

from PCCBuildDoctor import latest_failure
from PCCCortexWorker import worker_status
from PCCSharedEnvironment import resolve_toolchain_environment

COORDINATOR_VERSION = "PCC-REPAIR-COORDINATOR-0.1"
SOURCE_REPAIR_CATEGORIES = {
    "SOURCE_COMPILE",
    "SOURCE_LINT",
    "SOURCE_FORMAT",
    "TEST",
    "DEPENDENCY",
    "RUNTIME",
}
NON_SOURCE_BLOCKED = {"PATCH", "GIT", "AUTH", "CONTROL_PLANE", "UNKNOWN"}


def _write_receipt(root: Path, payload: dict[str, Any]) -> Path:
    out = root / "artifacts" / "repair"
    out.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    path = out / f"repair-{stamp}.json"
    latest = out / "latest.json"
    text = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    path.write_text(text, encoding="utf-8")
    latest.write_text(text, encoding="utf-8")
    return path


def _run_live(
    argv: list[str],
    *,
    cwd: Path,
    env: Mapping[str, str] | None = None,
    label: str,
) -> tuple[int, str]:
    print(f"[RepairCoordinator] START {label}", flush=True)
    print("[RepairCoordinator] Command: " + " ".join(str(x) for x in argv), flush=True)
    lines: list[str] = []
    try:
        proc = subprocess.Popen(
            argv,
            cwd=str(cwd),
            env=dict(env) if env is not None else None,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
    except Exception as exc:
        line = f"[FAIL] {label} could not start: {exc}"
        print(line, flush=True)
        return 2, line
    assert proc.stdout is not None
    for line in proc.stdout:
        lines.append(line)
        print(line, end="", flush=True)
    rc = int(proc.wait())
    print(f"[RepairCoordinator] END {label}: {'PASS' if rc == 0 else f'FAIL ({rc})'}", flush=True)
    return rc, "".join(lines)


def _target_validation(category: str, cargo: str) -> tuple[str, list[str]]:
    category = category.upper()
    if category == "SOURCE_FORMAT":
        return "targeted-format-check", [cargo, "fmt", "--all", "--", "--check"]
    if category == "SOURCE_LINT":
        return "targeted-clippy", [cargo, "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]
    if category == "TEST":
        return "targeted-tests", [cargo, "test", "--workspace", "--all-targets"]
    # Compile, dependency and runtime repair all need at least a complete check
    # before the universal QUICK/FULL gates are allowed to run.
    return "targeted-cargo-check", [cargo, "check", "--workspace", "--all-targets"]


def _runtime_and_env(root: Path) -> tuple[Path | None, dict[str, str], dict[str, Any]]:
    env, report = resolve_toolchain_environment(root, os.environ.copy(), timeout=20.0)
    status = worker_status(root)
    selected = str(status.get("selected") or "").strip()
    runtime = Path(selected).resolve() if selected else None
    return runtime, env, {"toolchain": report, "worker": status}


def repair_plan(root: Path, *, certify: bool = False) -> dict[str, Any]:
    project_root = root.expanduser().resolve()
    doctor = latest_failure(project_root)
    cls = doctor.get("classification") or {}
    category = str(cls.get("category") or "UNKNOWN").upper()
    status = str(doctor.get("status") or "")

    if status == "NO_ACTIVE_FAILURE":
        return {
            "schema": "pcc.repair_plan.v1",
            "coordinatorVersion": COORDINATOR_VERSION,
            "projectRoot": str(project_root),
            "status": "NO_REPAIR_NEEDED",
            "doctor": doctor,
            "mutatesSource": False,
            "requiresApproval": False,
            "steps": [],
        }

    if status in {"NO_EVIDENCE", "FAILURE_UNCLASSIFIED"} or category in NON_SOURCE_BLOCKED:
        guidance = {
            "PATCH": "Use patch-status / governed patch review; source auto-repair is intentionally blocked.",
            "GIT": "Use explicit Git checkout trust/rebind or Git diagnostics; source auto-repair is intentionally blocked.",
            "AUTH": "Authentication requires explicit user/provider action and cannot be auto-repaired.",
            "CONTROL_PLANE": "Control-plane failures require a protected PCC repair pass, not ordinary project source repair.",
            "UNKNOWN": "Collect/inspect additional evidence before mutation.",
        }.get(category, "Collect/inspect additional evidence before mutation.")
        return {
            "schema": "pcc.repair_plan.v1",
            "coordinatorVersion": COORDINATOR_VERSION,
            "projectRoot": str(project_root),
            "status": "BLOCKED_FOR_REVIEW",
            "doctor": doctor,
            "mutatesSource": False,
            "requiresApproval": False,
            "guidance": guidance,
            "steps": [],
        }

    if category == "ENVIRONMENT":
        return {
            "schema": "pcc.repair_plan.v1",
            "coordinatorVersion": COORDINATOR_VERSION,
            "projectRoot": str(project_root),
            "status": "ENVIRONMENT_RECHECK",
            "doctor": doctor,
            "mutatesSource": False,
            "requiresApproval": False,
            "steps": [
                "Resolve the operation ToolchainBroker environment without editing source.",
                "Re-run QUICK to determine whether the environment blocker is still active.",
                *( ["Run FULL only if QUICK returns GREEN."] if certify else [] ),
            ],
        }

    worker = worker_status(project_root)
    ready = worker.get("status") == "READY"
    return {
        "schema": "pcc.repair_plan.v1",
        "coordinatorVersion": COORDINATOR_VERSION,
        "projectRoot": str(project_root),
        "status": "READY" if ready else "WORKER_REQUIRED",
        "doctor": doctor,
        "worker": worker,
        "mutatesSource": True,
        "requiresApproval": True,
        "certifyFull": bool(certify),
        "steps": [
            "Open/reuse one durable Cortex source transaction.",
            "Give the transactional repair agent the bounded BuildDoctor evidence.",
            f"Run targeted validation for {category}.",
            "Run QUICK PROJECT GATE.",
            *( ["Run FULL QUALITY GATE and require GREEN."] if certify else [] ),
            "Commit the source transaction only after every required validation passes.",
            "Rollback automatically if repair or validation fails.",
        ],
    }


def _run_pcc_gate(root: Path, env: Mapping[str, str], gate: str) -> tuple[int, str]:
    pcc = root / "tools" / "control" / "CortexPCC.py"
    argv = [sys.executable, str(pcc), gate, "--root", str(root), "--no-evidence"]
    return _run_live(argv, cwd=root, env=env, label=f"PCC {gate.upper()}")


def _tx(runtime: Path, root: Path, env: Mapping[str, str], command: str) -> tuple[int, str]:
    argv = [str(runtime), "--workspace", str(root), "tx", command]
    return _run_live(argv, cwd=root, env=env, label=f"transaction {command}")


def _rollback_best_effort(runtime: Path | None, root: Path, env: Mapping[str, str] | None) -> None:
    if runtime is None or env is None:
        return
    print("[RepairCoordinator] Repair did not certify; rolling back the active transaction.", flush=True)
    _tx(runtime, root, env, "rollback")


def apply_repair(root: Path, *, yes: bool, certify: bool = False, user_message: str = "") -> int:
    project_root = root.expanduser().resolve()
    plan = repair_plan(project_root, certify=certify)
    print(f"PCC REPAIR COORDINATOR {COORDINATOR_VERSION}", flush=True)
    print(f"Plan status : {plan.get('status')}", flush=True)

    if plan.get("status") == "NO_REPAIR_NEEDED":
        print("[PASS] No active failed gate remains to repair. Newer GREEN evidence supersedes older failures.", flush=True)
        _write_receipt(project_root, {**plan, "result": "NO_OP", "finishedUnix": time.time()})
        return 0
    if plan.get("status") == "BLOCKED_FOR_REVIEW":
        print(f"[BLOCKED] {plan.get('guidance')}", flush=True)
        _write_receipt(project_root, {**plan, "result": "BLOCKED_FOR_REVIEW", "finishedUnix": time.time()})
        return 3

    try:
        runtime, env, runtime_report = _runtime_and_env(project_root)
    except Exception as exc:
        print(f"[BLOCKED_ENVIRONMENT] Toolchain/worker resolution failed: {exc}", flush=True)
        _write_receipt(project_root, {**plan, "result": "BLOCKED_ENVIRONMENT", "error": str(exc), "finishedUnix": time.time()})
        return 3

    if plan.get("status") == "ENVIRONMENT_RECHECK":
        rc, output = _run_pcc_gate(project_root, env, "quick")
        receipt: dict[str, Any] = {**plan, "runtime": runtime_report, "quickReturnCode": rc, "finishedUnix": time.time()}
        if rc != 0:
            receipt["result"] = "ENVIRONMENT_STILL_BLOCKED"
            _write_receipt(project_root, receipt)
            return rc or 3
        if certify:
            full_rc, _ = _run_pcc_gate(project_root, env, "full")
            receipt["fullReturnCode"] = full_rc
            receipt["result"] = "FULL_GREEN" if full_rc == 0 else "FULL_FAILED"
            _write_receipt(project_root, receipt)
            return full_rc
        receipt["result"] = "QUICK_GREEN"
        _write_receipt(project_root, receipt)
        return 0

    if not yes:
        print("[BLOCKED_APPROVAL] Source repair requires explicit --yes approval.", flush=True)
        print("[INFO] Run repair-plan first to review the bounded repair scope.", flush=True)
        return 4
    if runtime is None or not runtime.is_file():
        print("[BLOCKED] Transactional Cortex worker is not available. Run cortex-worker-build first.", flush=True)
        return 3

    doctor = plan.get("doctor") or {}
    cls = doctor.get("classification") or {}
    category = str(cls.get("category") or "UNKNOWN").upper()
    bridge = project_root / "tools" / "control" / "CortexPythonBridge.py"
    if not bridge.is_file():
        print(f"[FAIL] Cortex Python bridge missing: {bridge}", flush=True)
        return 2

    evidence = json.dumps(doctor, ensure_ascii=False, indent=2)
    prompt = (
        "Repair the active PCC failure using one bounded transactional source change. "
        "Do not alter launchers, control-plane files, Git configuration, or unrelated source unless the evidence explicitly implicates them. "
        "The controller will validate and rollback automatically if the candidate does not improve the failing gate.\n\n"
        f"BuildDoctor evidence:\n{evidence}"
    )
    if user_message.strip():
        prompt += f"\n\nOperator instruction:\n{user_message.strip()}"

    conversation_id = f"repair-{int(time.time())}-{uuid.uuid4().hex[:8]}"
    bridge_argv = [
        sys.executable,
        str(bridge),
        "--cortex-root", str(project_root),
        "--workspace", str(project_root),
        "--conversation-id", conversation_id,
        "--owner-pid", str(os.getpid()),
        "repair",
        prompt,
    ]

    started = time.time()
    repair_rc, repair_output = _run_live(bridge_argv, cwd=project_root, env=env, label="transactional Cortex repair")
    receipt: dict[str, Any] = {
        **plan,
        "runtime": runtime_report,
        "startedUnix": started,
        "repairReturnCode": repair_rc,
        "repairOutputTail": repair_output[-12000:],
        "conversationId": conversation_id,
    }
    if repair_rc != 0:
        _rollback_best_effort(runtime, project_root, env)
        receipt["result"] = "REPAIR_AGENT_FAILED"
        receipt["finishedUnix"] = time.time()
        path = _write_receipt(project_root, receipt)
        print(f"[FAIL] Repair agent did not produce a certifiable candidate. Receipt: {path}", flush=True)
        return repair_rc or 2

    cargo = str((runtime_report.get("toolchain") or {}).get("tools", {}).get("cargo") or "").strip()
    if not cargo:
        _rollback_best_effort(runtime, project_root, env)
        print("[BLOCKED_ENVIRONMENT] Cargo disappeared after the candidate repair; transaction rolled back.", flush=True)
        return 3

    validation_label, validation_argv = _target_validation(category, cargo)
    validation_rc, validation_output = _run_live(validation_argv, cwd=project_root, env=env, label=validation_label)
    receipt["targetValidation"] = {
        "label": validation_label,
        "returnCode": validation_rc,
        "outputTail": validation_output[-12000:],
    }
    if validation_rc != 0:
        _rollback_best_effort(runtime, project_root, env)
        receipt["result"] = "TARGET_VALIDATION_FAILED_ROLLED_BACK"
        receipt["finishedUnix"] = time.time()
        path = _write_receipt(project_root, receipt)
        print(f"[FAIL] Candidate did not pass targeted validation and was rolled back. Receipt: {path}", flush=True)
        return validation_rc or 2

    quick_rc, quick_output = _run_pcc_gate(project_root, env, "quick")
    receipt["quick"] = {"returnCode": quick_rc, "outputTail": quick_output[-12000:]}
    if quick_rc != 0:
        _rollback_best_effort(runtime, project_root, env)
        receipt["result"] = "QUICK_FAILED_ROLLED_BACK"
        receipt["finishedUnix"] = time.time()
        path = _write_receipt(project_root, receipt)
        print(f"[FAIL] Candidate failed QUICK and was rolled back. Receipt: {path}", flush=True)
        return quick_rc or 2

    if certify:
        full_rc, full_output = _run_pcc_gate(project_root, env, "full")
        receipt["full"] = {"returnCode": full_rc, "outputTail": full_output[-12000:]}
        if full_rc != 0:
            _rollback_best_effort(runtime, project_root, env)
            receipt["result"] = "FULL_FAILED_ROLLED_BACK"
            receipt["finishedUnix"] = time.time()
            path = _write_receipt(project_root, receipt)
            print(f"[FAIL] Candidate did not restore FULL GREEN and was rolled back. Receipt: {path}", flush=True)
            return full_rc or 2

    commit_rc, commit_output = _tx(runtime, project_root, env, "commit")
    receipt["transactionCommit"] = {"returnCode": commit_rc, "outputTail": commit_output[-8000:]}
    if commit_rc != 0:
        _rollback_best_effort(runtime, project_root, env)
        receipt["result"] = "TRANSACTION_COMMIT_FAILED_ROLLED_BACK"
        receipt["finishedUnix"] = time.time()
        path = _write_receipt(project_root, receipt)
        print(f"[FAIL] Transaction commit failed; candidate was rolled back. Receipt: {path}", flush=True)
        return commit_rc or 2

    receipt["result"] = "REPAIRED_FULL_GREEN" if certify else "REPAIRED_QUICK_GREEN"
    receipt["finishedUnix"] = time.time()
    path = _write_receipt(project_root, receipt)
    print(f"[PASS] Repair transaction committed: {receipt['result']}", flush=True)
    print(f"[PASS] Repair receipt: {path}", flush=True)
    return 0


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Deterministic PCC/Cortex repair coordinator")
    parser.add_argument("command", choices=["plan", "plan-full", "apply", "apply-full"])
    parser.add_argument("--root", required=True)
    parser.add_argument("--yes", action="store_true")
    parser.add_argument("--message", default="")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)
    root = Path(args.root)
    certify = args.command in {"plan-full", "apply-full"}
    if args.command.startswith("plan"):
        payload = repair_plan(root, certify=certify)
        if args.json:
            print(json.dumps(payload, indent=2, sort_keys=True))
        else:
            print(f"PCC REPAIR COORDINATOR {COORDINATOR_VERSION}")
            print(f"Status : {payload.get('status')}")
            cls = (payload.get("doctor") or {}).get("classification") or {}
            if cls:
                print(f"Failure: {cls.get('category')} / {cls.get('code')}")
            for step in payload.get("steps") or []:
                print(f"  - {step}")
            if payload.get("guidance"):
                print(f"Guidance: {payload.get('guidance')}")
        return 0 if payload.get("status") not in {"WORKER_REQUIRED", "BLOCKED_FOR_REVIEW"} else 3
    return apply_repair(root, yes=args.yes, certify=certify, user_message=args.message)


if __name__ == "__main__":
    raise SystemExit(main())
