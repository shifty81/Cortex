#!/usr/bin/env python3
"""Conservative source-preservation guard for Cortex CLI and PCC interfaces.

Static presence checks catch accidental removals. They are NOT behavioral parity
certification, and intentionally do not freeze file hashes or require Desktop GUI.
"""
from __future__ import annotations

import argparse
import ast
import json
from pathlib import Path
from typing import Any

MANIFEST = "config/cortex/capability-preservation.v1.json"


def audit(root: Path, manifest: dict[str, Any]) -> dict[str, Any]:
    failures: list[str] = []
    checked = 0
    for entry in manifest.get("checks", []):
        relative = str(entry["path"])
        path = root / relative
        checked += 1
        if not path.is_file():
            failures.append(f"{relative}: required source file absent")
            continue
        source = path.read_text(encoding="utf-8-sig")
        for token in entry.get("contains", []):
            if token not in source:
                failures.append(f"{relative}: missing preserved entry-point token {token!r}")
        if "definitions" in entry:
            try:
                tree = ast.parse(source, filename=relative)
            except SyntaxError as exc:
                failures.append(f"{relative}: Python syntax error: {exc}")
                continue
            names = {node.name for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))}
            for definition in entry["definitions"]:
                if definition not in names:
                    failures.append(f"{relative}: missing preserved definition {definition}")
    return {"schema": "cortex.capability_preservation.audit.v1", "status": "FAIL" if failures else "PASS",
            "checkedFiles": checked, "failures": failures,
            "note": "Static entry-point retention only; FULL and live GUI/CLI behavior tests remain required."}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Read-only Cortex preserved-interface source guard")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--json", action="store_true")
    options = parser.parse_args(argv)
    root = options.root.expanduser().resolve()
    manifest_file = root / MANIFEST
    if not manifest_file.is_file():
        print(f"[FAIL] Required capability manifest missing: {manifest_file}")
        return 2
    manifest = json.loads(manifest_file.read_text(encoding="utf-8-sig"))
    if manifest.get("schema") != "cortex.capability_preservation.v1":
        print("[FAIL] Invalid capability preservation manifest schema")
        return 2
    result = audit(root, manifest)
    if options.json:
        print(json.dumps(result, indent=2))
    else:
        print(f"[{'PASS' if result['status'] == 'PASS' else 'FAIL'}] "
              f"Cortex interface-retention guard: {result['checkedFiles']} source files inspected")
        for failure in result["failures"]:
            print("[FAIL] " + failure)
        print("[INFO] Static preservation checks do not certify functional or data parity.")
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
