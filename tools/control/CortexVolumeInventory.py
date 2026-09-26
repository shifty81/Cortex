"""R8A: read-only, drive-letter-independent volume inventory for Cortex.

No files, directories, databases, or registry entries are created or changed.
Output is JSON on stdout; callers own any persistence/ingestion decisions.
"""
from __future__ import annotations

import argparse
import json
import os
import stat
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA = "cortex.volume_inventory.r8a.v1"


def inventory(root: os.PathLike[str] | str, *, max_entries: int = 2_000_000) -> dict[str, Any]:
    """Walk an existing volume/root without following links or crossing mount points.

    Inventory paths are relative to root, retaining original spelling/case. Every
    discoverable entry is included, including hidden entries, .git, lowercase
    projects, and top-level Git. Unreadable entries are recorded as errors.
    """
    if max_entries < 1:
        raise ValueError("max_entries must be positive")
    base = Path(root).expanduser().resolve(strict=True)
    if not base.is_dir():
        raise NotADirectoryError(str(base))
    base_stat = base.stat()
    base_drive = os.path.normcase(os.path.splitdrive(str(base))[0])
    entries: list[dict[str, Any]] = []
    errors: list[dict[str, str]] = []
    stack = [(base, "")]
    truncated = False
    while stack:
        folder, relative = stack.pop()
        try:
            with os.scandir(folder) as scan:
                children = sorted(scan, key=lambda item: (item.name.casefold(), item.name))
        except OSError as exc:
            errors.append({"path": relative or ".", "error": f"{type(exc).__name__}: {exc}"})
            continue
        dirs = []
        for child in children:
            if len(entries) >= max_entries:
                truncated = True
                break
            rel = f"{relative}/{child.name}" if relative else child.name
            try:
                info = child.stat(follow_symlinks=False)
                mode = info.st_mode
                kind = ("symlink" if stat.S_ISLNK(mode) else "directory" if stat.S_ISDIR(mode)
                        else "file" if stat.S_ISREG(mode) else "other")
                record: dict[str, Any] = {
                    "path": rel.replace(os.sep, "/"), "type": kind,
                    "size_bytes": info.st_size if kind == "file" else None,
                    "modified_ns": info.st_mtime_ns,
                }
                entries.append(record)
                if kind == "directory":
                    # Windows st_dev/ismount semantics can mark ordinary folders as
                    # boundaries. Use the drive identity there, and explicitly
                    # exclude reparse points (junctions/mounted folders) instead.
                    attrs = getattr(info, "st_file_attributes", 0)
                    reparse = bool(attrs & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0))
                    child_drive = os.path.normcase(os.path.splitdrive(child.path)[0])
                    if os.name == "nt":
                        if reparse:
                            record["boundary"] = "reparse_point"
                        elif child_drive != base_drive:
                            record["boundary"] = "different_device"
                        else:
                            dirs.append((Path(child.path), rel))
                    elif info.st_dev != base_stat.st_dev:
                        record["boundary"] = "different_device"
                    elif os.path.ismount(child.path) and Path(child.path) != base:
                        record["boundary"] = "mount_point"
                    else:
                        dirs.append((Path(child.path), rel))
            except OSError as exc:
                errors.append({"path": rel.replace(os.sep, "/"), "error": f"{type(exc).__name__}: {exc}"})
        if truncated:
            break
        stack.extend(reversed(dirs))
    counts = {kind: sum(e["type"] == kind for e in entries) for kind in ("file", "directory", "symlink", "other")}
    return {
        "schema": SCHEMA, "root": str(base), "scanned_at_utc": datetime.now(timezone.utc).isoformat(),
        "entries": entries, "counts": counts, "errors": errors, "truncated": truncated,
        "max_entries": max_entries, "read_only": True,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Read-only whole-volume metadata inventory; JSON to stdout only")
    parser.add_argument("root", help="Existing drive root or directory; no hard-coded drive letter")
    parser.add_argument("--max-entries", type=int, default=2_000_000)
    args = parser.parse_args(argv)
    try:
        result = inventory(args.root, max_entries=args.max_entries)
    except (OSError, ValueError) as exc:
        print(f"Inventory failed: {exc}", file=sys.stderr)
        return 2
    json.dump(result, sys.stdout, ensure_ascii=False, separators=(",", ":"))
    sys.stdout.write("\n")
    return 1 if result["errors"] or result["truncated"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
