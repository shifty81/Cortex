"""Bounded, read-only GUI presentation of an in-memory volume inventory."""
from __future__ import annotations

from typing import Any


def render_inventory(report: dict[str, Any], *, query: str = "", limit: int = 80) -> str:
    if limit < 1:
        raise ValueError("limit must be positive")
    entries = report.get("entries") or []
    counts = report.get("counts") or {}
    errors = report.get("errors") or []
    parts = [
        f"Volume root: {report.get('root', '?')}",
        f"Read-only metadata entries: {len(entries):,} | Files: {counts.get('file', 0):,} | "
        f"Directories: {counts.get('directory', 0):,} | Links: {counts.get('symlink', 0):,}",
        f"Status: {'CANCELLED / PARTIAL' if report.get('cancelled') else 'LIMIT REACHED / PARTIAL' if report.get('truncated') else 'INCOMPLETE / ERRORS' if errors else 'COMPLETE'}"
        f" | Unreadable entries: {len(errors):,}",
        "No files imported, moved, hashed, registered, or written to the Vault database.",
    ]
    needle = query.strip().casefold()
    if needle:
        matches = (entry for entry in entries if needle in str(entry.get("path", "")).casefold())
    else:
        # The default view shows just top-level roots; use the filter to inspect any descendant.
        matches = (entry for entry in entries if "/" not in str(entry.get("path", "")))
    sample = []
    total = 0
    for entry in matches:
        total += 1
        if len(sample) < limit:
            label = str(entry.get("path", ""))
            kind = str(entry.get("type", "other"))
            boundary = " [not traversed: " + str(entry["boundary"]) + "]" if entry.get("boundary") else ""
            sample.append(f"  [{kind}] {label}{boundary}")
    parts.extend([f"{'Matching entries for ' + repr(query.strip()) if needle else 'Top-level entries'}: {total:,} (showing up to {limit})", *sample])
    if errors:
        parts.append("Unreadable sample:")
        parts.extend(f"  {e.get('path', '?')}: {e.get('error', '')}" for e in errors[:5])
    if report.get("truncated"):
        parts.append("Partial inventory: raise the limit or repeat after cancellation to obtain complete coverage.")
    return "\n".join(parts)
