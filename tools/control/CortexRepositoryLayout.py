#!/usr/bin/env python3
"""Verify the converged Cortex repository/source layout.

This module is both the canonical layout scanner used by PCC and a small CLI.
It separates repository/source violations, generated local outputs, project-local
runtime state, and portable-volume runtime state so every surface reports the
same source-authority truth.
"""
from __future__ import annotations

import argparse
import fnmatch
import json
from pathlib import Path
from typing import Any, Iterable


def _placeholder_only(root: Path, rel: str, protected: set[str]) -> bool:
    src = root / rel
    if not src.exists() or src.is_symlink() or bool(getattr(src, "is_junction", lambda: False)()):
        return False
    if src.is_file():
        return rel in protected
    for item in src.rglob("*"):
        if item.is_symlink() or bool(getattr(item, "is_junction", lambda: False)()):
            return False
        if item.is_file() and item.relative_to(root).as_posix() not in protected:
            return False
    return True


def load_policy(root: Path) -> dict[str, Any]:
    path = root / "config" / "cortex" / "repository_layout.v1.json"
    if not path.is_file():
        raise FileNotFoundError(path)
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"invalid Cortex repository layout policy: {path}")
    schema = str(value.get("schema") or "").strip()
    if schema and schema != "cortex.repository_layout.v1":
        raise ValueError(f"invalid Cortex repository layout policy: {path}")
    return value


def _matches_archive_glob(name: str, patterns: list[str]) -> bool:
    folded = name.casefold()
    return any(fnmatch.fnmatch(folded, str(pattern).casefold()) for pattern in patterns)


def _mapping_rows(value: Any) -> Iterable[tuple[str, str]]:
    if isinstance(value, dict):
        for rel, destination in value.items():
            yield str(rel), str(destination)
    elif isinstance(value, list):
        for rel in value:
            yield str(rel), ""


def scan_repository_layout(root: Path) -> dict[str, Any]:
    root = root.resolve()
    policy = load_policy(root)
    protected = {str(x).replace("\\", "/") for x in policy.get("preserve_runtime_placeholders", [])}
    allowed_files = set(policy["root_files"]) | set(policy.get("keep_review_root_files", []))
    allowed_dirs = (
        set(policy["root_directories"])
        | set(policy.get("operational_directories", []))
        | set(policy.get("generated_root_directories", []))
    )
    placeholder_roots = {rel.split("/")[0] for rel in protected}
    archive_globs = [str(x) for x in policy.get("archive_root_globs", [])]
    project_runtime_rows = list(_mapping_rows(policy.get("runtime_state", {})))
    volume_runtime_rows = list(_mapping_rows(policy.get("volume_runtime_state", {})))
    runtime_source_roots = {
        rel.replace("\\", "/").split("/")[0]
        for rel, _ in project_runtime_rows + volume_runtime_rows
        if rel.strip()
    }

    items: list[dict[str, Any]] = []
    for item in sorted(root.iterdir(), key=lambda p: p.name.casefold()):
        if item.name == ".git":
            continue
        if item.is_file() and item.name not in allowed_files:
            items.append({
                "path": item.name,
                "kind": "root-file",
                "severity": "violation",
                "archiveEligible": _matches_archive_glob(item.name, archive_globs),
            })
        elif item.is_dir() and item.name not in allowed_dirs and item.name not in runtime_source_roots:
            runtime_placeholder = item.name in placeholder_roots and any(
                rel == item.name and _placeholder_only(root, rel, protected)
                for rel, _ in project_runtime_rows
            )
            if not runtime_placeholder:
                items.append({
                    "path": item.name + "/",
                    "kind": "root-directory",
                    "severity": "violation",
                    "archiveEligible": False,
                })

    for scope, rows in (("volume", project_runtime_rows), ("volume", volume_runtime_rows)):
        for rel, destination in rows:
            rel = rel.replace("\\", "/")
            if (root / rel).exists() and not _placeholder_only(root, rel, protected):
                items.append({
                    "path": rel,
                    "kind": "runtime-in-repository",
                    "severity": "violation",
                    "archiveEligible": False,
                    "runtimeDestination": destination,
                    "runtimeScope": scope,
                })

    return {
        "schema": "cortex.repository_layout.scan.v1",
        "projectRoot": str(root),
        "clean": not items,
        "violationCount": len(items),
        "items": items,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    args = ap.parse_args(argv)
    root = Path(args.root).resolve()
    try:
        report = scan_repository_layout(root)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"[FAIL] Repository layout policy could not be evaluated: {exc}")
        return 2
    if not report["clean"]:
        print("[FAIL] Repository layout not converged.")
        for item in report["items"]:
            prefix = "RUNTIME-IN-REPO" if item["kind"] == "runtime-in-repository" else "ROOT"
            suffix = ""
            if item.get("runtimeDestination"):
                suffix = f" -> {item.get('runtimeScope', 'project')}:{item['runtimeDestination']}"
            print(f"  {prefix} {item['path']}{suffix}")
        return 2
    print("[PASS] Repository root conforms to cortex.repository_layout.v1 (generated outputs and tracked placeholders allowed)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
