#!/usr/bin/env python3
"""Build/status authority for the standalone transactional Cortex worker.

This module intentionally builds only the `cortex` binary package.  It uses the
same bounded ToolchainBroker environment as PCC project operations and never
runs as an implicit side effect of chat startup.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path
from typing import Any, Iterable, Mapping

from PCCSharedEnvironment import BROKER_VERSION, resolve_toolchain_environment

WORKER_VERSION = "PCC-CORTEX-WORKER-0.1"


def _candidate_executable(env: Mapping[str, str], root: Path, *, release: bool = False) -> Path:
    target_raw = str(env.get("CARGO_TARGET_DIR") or "").strip()
    target = Path(target_raw).expanduser() if target_raw else (root / "target")
    profile = "release" if release else "debug"
    name = "cortex.exe" if os.name == "nt" else "cortex"
    return (target / profile / name).resolve()


def _tool_name(path: Any) -> str:
    value = str(path or "").strip()
    return Path(value).name if value else "missing"


def worker_status(root: Path) -> dict[str, Any]:
    project_root = root.expanduser().resolve()
    try:
        env, report = resolve_toolchain_environment(project_root, os.environ.copy(), timeout=20.0)
    except Exception as exc:
        return {
            "schema": "pcc.cortex_worker_status.v1",
            "workerVersion": WORKER_VERSION,
            "projectRoot": str(project_root),
            "status": "BROKER_ERROR",
            "error": str(exc),
        }

    debug = _candidate_executable(env, project_root, release=False)
    release = _candidate_executable(env, project_root, release=True)
    selected = debug if debug.is_file() else release if release.is_file() else None
    return {
        "schema": "pcc.cortex_worker_status.v1",
        "workerVersion": WORKER_VERSION,
        "projectRoot": str(project_root),
        "status": "READY" if selected else "MISSING",
        "selected": str(selected) if selected else None,
        "debug": str(debug),
        "debugPresent": debug.is_file(),
        "release": str(release),
        "releasePresent": release.is_file(),
        "toolchain": report,
    }


def _write_receipt(root: Path, payload: dict[str, Any]) -> Path:
    out = root / "artifacts" / "cortex-worker"
    out.mkdir(parents=True, exist_ok=True)
    path = out / "latest.json"
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(path)
    return path


def build_worker(root: Path, *, release: bool = False) -> int:
    project_root = root.expanduser().resolve()
    started = time.time()
    print(f"[CortexWorker] {WORKER_VERSION}", flush=True)
    print(f"[CortexWorker] Toolchain broker {BROKER_VERSION}: resolving build environment...", flush=True)
    try:
        env, report = resolve_toolchain_environment(project_root, os.environ.copy(), timeout=20.0)
    except Exception as exc:
        print(f"[BLOCKED_ENVIRONMENT] Toolchain resolution failed: {exc}", flush=True)
        return 3

    tools = report.get("tools") or {}
    msvc = report.get("msvc") or {}
    print(
        "[CortexWorker] Environment: "
        f"cargo={_tool_name(tools.get('cargo'))} "
        f"rustc={_tool_name(tools.get('rustc'))} "
        f"msvc={msvc.get('status', 'unknown')}",
        flush=True,
    )

    cargo = str(tools.get("cargo") or "").strip()
    rustc = str(tools.get("rustc") or "").strip()
    if not cargo or not rustc:
        print("[BLOCKED_ENVIRONMENT] Rust toolchain is unavailable; source was not implicated.", flush=True)
        return 3
    if os.name == "nt" and str(msvc.get("status") or "") not in {"active", "activated"}:
        print(
            "[BLOCKED_ENVIRONMENT] windows.msvc.amd64 is not active. "
            f"ToolchainBroker status={msvc.get('status', 'unknown')}",
            flush=True,
        )
        print("[INFO] Project source was not modified and was not implicated.", flush=True)
        return 3

    manifest = project_root / "Cargo.toml"
    if not manifest.is_file():
        print(f"[FAIL] Cortex workspace manifest is missing: {manifest}", flush=True)
        return 2

    executable = _candidate_executable(env, project_root, release=release)
    argv = [cargo, "build", "--manifest-path", str(manifest), "-p", "cortex"]
    if release:
        argv.append("--release")
    print("[CortexWorker] Building only the transactional Cortex worker; FULL workspace gate is not being run.", flush=True)
    print("[CortexWorker] Command: " + " ".join(argv), flush=True)
    print(f"[CortexWorker] Target: {executable}", flush=True)

    try:
        proc = subprocess.Popen(
            argv,
            cwd=str(project_root),
            env=env,
            stdin=subprocess.DEVNULL,
        )
        rc = int(proc.wait())
    except Exception as exc:
        print(f"[FAIL] Cortex worker build could not start: {exc}", flush=True)
        return 2

    receipt: dict[str, Any] = {
        "schema": "pcc.cortex_worker_build_receipt.v1",
        "workerVersion": WORKER_VERSION,
        "projectRoot": str(project_root),
        "profile": "release" if release else "debug",
        "target": str(executable),
        "returnCode": rc,
        "startedUnix": started,
        "finishedUnix": time.time(),
        "toolchain": report,
        "status": "PASS" if rc == 0 and executable.is_file() else "FAIL",
    }
    receipt_path = _write_receipt(project_root, receipt)

    if rc != 0:
        print(f"[FAIL] Cortex worker build exited {rc}.", flush=True)
        print(f"[INFO] Receipt: {receipt_path}", flush=True)
        return rc if rc > 0 else 2
    if not executable.is_file():
        print("[FAIL] Cargo returned success but the expected Cortex worker was not produced.", flush=True)
        print(f"[INFO] Expected: {executable}", flush=True)
        return 2

    print(f"[PASS] Transactional Cortex worker ready: {executable}", flush=True)
    print(f"[PASS] Receipt: {receipt_path}", flush=True)
    print("[INFO] New Cortex chat/apply/repair requests can now use the worker directly without cargo run.", flush=True)
    return 0


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Transactional Cortex worker status/build authority")
    parser.add_argument("command", choices=["status", "build", "build-release"])
    parser.add_argument("--root", required=True)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(list(argv) if argv is not None else None)
    root = Path(args.root)
    if args.command == "status":
        payload = worker_status(root)
        if args.json:
            print(json.dumps(payload, indent=2, sort_keys=True))
        else:
            print(f"CORTEX WORKER {WORKER_VERSION}")
            print(f"Status  : {payload.get('status')}")
            print(f"Selected: {payload.get('selected') or '<none>'}")
            toolchain = payload.get("toolchain") or {}
            tools = toolchain.get("tools") or {}
            msvc = toolchain.get("msvc") or {}
            print(f"Cargo   : {_tool_name(tools.get('cargo'))}")
            print(f"Rustc   : {_tool_name(tools.get('rustc'))}")
            print(f"MSVC    : {msvc.get('status', 'unknown')}")
        return 0 if payload.get("status") == "READY" else 1
    return build_worker(root, release=(args.command == "build-release"))


if __name__ == "__main__":
    raise SystemExit(main())
