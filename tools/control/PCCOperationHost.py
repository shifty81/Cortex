#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path
from typing import Sequence

from PCCRepoHygiene import prepare
from PCCSharedEnvironment import BROKER_VERSION, resolve_toolchain_environment

VERSION = "PCC-OPERATION-HOST-0.4"

# Operations that should begin and end from a transport-clean repository root.
CLEAN_OPERATIONS = {
    "full", "quick", "fast", "build", "build-release", "self-test",
    "commit-green", "commit-push-green", "push", "git-pull",
    "debug-bundle", "doctor", "root-hygiene", "root-hygiene-fix",
}


def _print_hygiene(label: str, root: Path) -> int:
    try:
        result = prepare(root, apply=True)
    except Exception as exc:
        print(f"[FAIL] {label} repository transport hygiene failed: {exc}", flush=True)
        return 1
    moved = int(result.get("moved", 0) or 0)
    if moved:
        print(f"[PASS] {label} repository transport hygiene moved {moved} operational artifact(s).", flush=True)
        for row in result.get("moves", []):
            print(f"  MOVE {Path(row['source']).name} -> {row['destination']}", flush=True)
    else:
        print(f"[PASS] {label} repository transport hygiene clean.", flush=True)
    pending = result.get("pendingPatchTransports") or []
    if pending:
        print(f"[INFO] {len(pending)} pending patch transport(s) preserved for project update authority.", flush=True)
    return 0


def _tool_name(path: object) -> str:
    value = str(path or "").strip()
    return Path(value).name if value else "missing"


def _run(argv: Sequence[str], root: Path) -> int:
    print(f"[PCC] Toolchain broker {BROKER_VERSION}: resolving child environment...", flush=True)
    try:
        env, report = resolve_toolchain_environment(root, os.environ.copy(), timeout=20.0)
    except Exception as exc:
        # Resolution must never create a silent operation-host deadlock.  The
        # provider still gets a chance to report its own structured blocker.
        print(f"[WARN] Toolchain broker failed open to inherited environment: {exc}", flush=True)
        env = os.environ.copy()
        report = {"elapsedMs": 0, "tools": {}, "msvc": {"status": "broker_error"}}

    tools = report.get("tools") or {}
    msvc = report.get("msvc") or {}
    print(
        "[PCC] Toolchain resolution complete "
        f"({int(report.get('elapsedMs', 0) or 0)} ms) | "
        f"python={_tool_name(tools.get('python'))} "
        f"git={_tool_name(tools.get('git'))} "
        f"cargo={_tool_name(tools.get('cargo'))} "
        f"rustc={_tool_name(tools.get('rustc'))} "
        f"msvc={msvc.get('status', 'unknown')}",
        flush=True,
    )
    if str(msvc.get("status") or "") in {"activation_failed", "activation_timeout", "activation_incomplete"}:
        detail = str(msvc.get("error") or "").strip()
        vsdev = str(msvc.get("vsDevCmd") or "").strip()
        print(f"[WARN] MSVC activation detail: vsDevCmd={vsdev or '<not found>'}", flush=True)
        if detail:
            print(f"[WARN] MSVC activation error: {detail}", flush=True)

    env["PCC_OPERATION_HOST_ACTIVE"] = "1"
    env["PYTHONUNBUFFERED"] = "1"
    display = " ".join(str(item) for item in argv[:4])
    if len(argv) > 4:
        display += " ..."
    print(f"[PCC] Provider launch: {display}", flush=True)
    proc = subprocess.Popen(list(argv), cwd=str(root), stdin=subprocess.DEVNULL, env=env)
    print(f"[PCC] Provider PID: {proc.pid}", flush=True)
    rc = int(proc.wait())
    print(f"[PCC] Provider exit: {rc}", flush=True)
    return rc


def main(argv: Sequence[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Universal PCC operation host")
    ap.add_argument("--root", required=True)
    ap.add_argument("--operation", required=True)
    ap.add_argument("child", nargs=argparse.REMAINDER)
    ns = ap.parse_args(argv)
    root = Path(ns.root).expanduser().resolve()
    operation = str(ns.operation)
    child = list(ns.child)
    if child and child[0] == "--":
        child = child[1:]
    if not child:
        print("[FAIL] PCC operation host received no provider command.", flush=True)
        return 2

    do_clean = operation in CLEAN_OPERATIONS
    if do_clean and _print_hygiene("Pre-operation", root) != 0:
        return 1

    print(f"[PCC] Operation host {VERSION}: {operation}", flush=True)
    rc = 1
    try:
        rc = _run(child, root)
    finally:
        if do_clean:
            clean_rc = _print_hygiene("Post-operation", root)
            if rc == 0 and clean_rc != 0:
                rc = clean_rc
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
