"""Explicit, read-only acceptance report for the existing Cortex volume index.

This tool never calls run_scan(), refresh_directory(), refresh_tree() or _initialize().
It does not provision a new volume marker, index, project registry or Vault catalog.
Detailed counts and SQLite quick_check are opt-in, off the GUI thread.
"""
from __future__ import annotations

import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3
from typing import Any

from CortexPersistentVolumeInventory import (
    _connection, _status, _stored_volume_id, index_location,
)

KINDS = ("file", "directory", "symlink", "other")
REQUIRED_TABLES = {"meta", "scan_state", "entries", "directories", "errors"}


def audit_index(db_path: Path | str, volume_root: Path | str, *, volume_id: str,
                verify_counts: bool = False, quick_check: bool = False) -> dict[str, Any]:
    """Snapshot metadata and, when explicitly requested, compare physical row counts.

    Read-only SQLite URI ensures a missing index cannot be created by the audit.
    BEGIN maintains a single WAL snapshot across summary and optional checks.
    """
    with closing(_connection(Path(db_path), writable=False)) as conn:
        conn.execute("PRAGMA query_only=ON")
        conn.execute("BEGIN")
        try:
            tables = {str(row[0]) for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table'")}
            missing = sorted(REQUIRED_TABLES - tables)
            if missing:
                raise ValueError("Inventory has missing required tables: " + ", ".join(missing))
            actual_volume = _stored_volume_id(conn)
            if actual_volume != volume_id:
                raise ValueError("Volume marker does not match the existing inventory; refusing audit")
            summary = _status(conn, Path(volume_root))
            report: dict[str, Any] = {
                "schema": "cortex.volume_inventory.acceptance.v1",
                "volume_id": actual_volume,
                "database": str(Path(db_path)),
                "state": summary["state"],
                "recorded_counts": summary["counts"],
                "recorded_entries": summary["entries"],
                "access_gaps": summary["errors"],
                "pending_scan_directories": summary["pending_directories"],
                "database_user_version": int(conn.execute("PRAGMA user_version").fetchone()[0]),
                "recursive_job": None,
                "counts_verified": None,
                "sqlite_quick_check": None,
                "source_read_only": True,
            }
            if "recursive_refresh" in tables:
                job = conn.execute("SELECT root,state,refreshed,unreadable FROM recursive_refresh WHERE id=1").fetchone()
                if job is not None:
                    pending = int(conn.execute(
                        "SELECT COUNT(*) FROM recursive_refresh_queue WHERE finished=0").fetchone()[0])
                    report["recursive_job"] = {**dict(job), "pending": pending}
            pending_job = report["recursive_job"]
            report["completed"] = (summary["state"] == "complete"
                                   and summary["pending_directories"] == 0
                                   and (pending_job is None
                                        or (pending_job["state"] not in ("running", "paused")
                                            and pending_job["pending"] == 0)))
            if verify_counts:
                actual = {kind: 0 for kind in KINDS}
                for row in conn.execute("SELECT type,COUNT(*) FROM entries GROUP BY type"):
                    if row[0] not in actual:
                        raise ValueError("Unrecognized inventory entry type: " + str(row[0]))
                    actual[row[0]] = int(row[1])
                report["actual_counts"] = actual
                report["counts_verified"] = actual == summary["counts"]
                report["actual_entries"] = sum(actual.values())
            if quick_check:
                check = [str(row[0]) for row in conn.execute("PRAGMA quick_check")]
                report["sqlite_quick_check"] = check == ["ok"]
                if check != ["ok"]:
                    report["sqlite_quick_check_detail"] = check[:10]
            report["passed"] = (report["database_user_version"] == 1
                                and report["completed"]
                                and report["counts_verified"] is not False
                                and report["sqlite_quick_check"] is not False)
            return report
        finally:
            conn.rollback()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Read-only acceptance of Cortex's existing portable inventory")
    parser.add_argument("--cortex-root", type=Path, required=True, help="Existing Cortex repository on the marked volume")
    parser.add_argument("--verify-counts", action="store_true", help="Explicitly scan the index to compare recorded and actual row counts")
    parser.add_argument("--quick-check", action="store_true", help="Explicit SQLite PRAGMA quick_check (can be slow on large indexes)")
    parser.add_argument("--json", action="store_true", help="Emit a single machine-readable JSON report")
    args = parser.parse_args(argv)
    try:
        mount_root, db, volume_id = index_location(args.cortex_root)
        report = audit_index(db, mount_root, volume_id=volume_id,
                             verify_counts=args.verify_counts, quick_check=args.quick_check)
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
        report = {"schema": "cortex.volume_inventory.acceptance.v1", "passed": False,
                  "source_read_only": True, "error": str(exc)}
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(("[PASS]" if report.get("passed") else "[FAIL]") + " Existing inventory acceptance")
        for k, value in report.items():
            if k not in ("schema",):
                print(f" {k}: {value}")
    return 0 if report.get("passed") else 2


if __name__ == "__main__":
    raise SystemExit(main())
