#!/usr/bin/env python3
"""Verify converged root without treating tracked runtime placeholders as drift."""
from __future__ import annotations
import argparse
import json
from pathlib import Path


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


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    args = ap.parse_args(argv)
    root = Path(args.root).resolve()
    policy = json.loads((root / "config/cortex/repository_layout.v1.json").read_text(encoding="utf-8"))
    protected = {str(x).replace("\\", "/") for x in policy.get("preserve_runtime_placeholders", [])}
    allowed_files = set(policy["root_files"]) | set(policy.get("keep_review_root_files", []))
    allowed_dirs = set(policy["root_directories"])
    placeholder_roots = {rel.split("/")[0] for rel in protected}
    bad = []
    for item in root.iterdir():
        if item.name == ".git":
            continue
        if item.is_file() and item.name not in allowed_files:
            bad.append(item.name)
        elif item.is_dir() and item.name not in allowed_dirs:
            if item.name not in placeholder_roots or not any(
                rel == item.name and _placeholder_only(root, rel, protected)
                for rel in policy["runtime_state"]
            ):
                bad.append(item.name + "/")
    runtime = [rel for rel in policy["runtime_state"]
               if (root / rel).exists() and not _placeholder_only(root, rel, protected)]
    if bad or runtime:
        print("[FAIL] Repository layout not converged.")
        for item in bad:
            print("  ROOT " + item)
        for item in runtime:
            print("  RUNTIME-IN-REPO " + item)
        return 2
    print("[PASS] Repository root conforms to cortex.repository_layout.v1 (tracked placeholders allowed)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
