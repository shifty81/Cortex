#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

CONTROL = Path(__file__).resolve().parent
if str(CONTROL) not in sys.path:
    sys.path.insert(0, str(CONTROL))

from PCCSharedEnvironment import resolve_toolchain_environment


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    args = parser.parse_args()

    root = Path(args.root).expanduser().resolve()
    env, report = resolve_toolchain_environment(
        root,
        os.environ.copy(),
        control_file=Path(__file__),
        include_msvc=True,
        timeout=20.0,
    )
    env["RUSTUP_AUTO_INSTALL"] = "0"

    tools = report.get("tools") or {}
    cargo = str(tools.get("cargo") or "").strip()
    if not cargo:
        print("[FAIL] Portable Cargo was not resolved by ToolchainBroker.", file=sys.stderr)
        return 2

    print(f"[INFO] ToolchainBroker={report.get('brokerVersion')}")
    print(f"[INFO] Cargo={cargo}")
    print(f"[INFO] CARGO_HOME={env.get('CARGO_HOME', '')}")
    print(f"[INFO] RUSTUP_HOME={env.get('RUSTUP_HOME', '')}")
    print(f"[INFO] CARGO_TARGET_DIR={env.get('CARGO_TARGET_DIR', '')}")

    applied = subprocess.run(
        [cargo, "fmt", "--all"],
        cwd=str(root),
        env=env,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if applied.returncode != 0:
        print(f"[FAIL] cargo fmt --all exited {applied.returncode}.", file=sys.stderr)
        return int(applied.returncode or 1)

    verified = subprocess.run(
        [cargo, "fmt", "--all", "--", "--check"],
        cwd=str(root),
        env=env,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if verified.returncode != 0:
        print(f"[FAIL] cargo fmt verification exited {verified.returncode}.", file=sys.stderr)
        return int(verified.returncode or 1)

    print("[PASS] Canonical Rust formatting applied and verified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
