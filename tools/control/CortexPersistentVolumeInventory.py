"""R8H: persistent, bounded-memory metadata inventory for an explicitly selected volume.

Only Cortex's dedicated inventory SQLite index is written.  Source content, project
registrations, the existing Vault catalog and filesystem metadata are never changed.
The durable directory queue permits idempotent resumption after cancellation/crash.
"""
from __future__ import annotations

import os
from contextlib import closing, contextmanager
import re
import sqlite3
import stat
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

SCHEMA_VERSION = 1
BATCH_SIZE = 512
PAGE_SIZE = 200
MAX_QUERY_SECONDS = 8.0


def _utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def index_location(cortex_root: Path | str) -> tuple[Path, Path, str]:
    """Use the established portable Cortex volume marker; never initialize a new one."""
    from PCCVolumeAuthority import resolve_volume_context
    ctx = resolve_volume_context(cortex_root, create=False)
    volume_id = str(ctx.volume_id)
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", volume_id):
        raise ValueError("Invalid volume identity for an inventory index")
    return ctx.mount_root, ctx.state_root / "inventory" / volume_id / "inventory.sqlite3", volume_id


def _connection(db_path: Path, *, writable: bool) -> sqlite3.Connection:
    if writable:
        db_path.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(str(db_path), timeout=30)
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("PRAGMA synchronous=NORMAL")
    else:
        if not db_path.is_file():
            raise FileNotFoundError(str(db_path))
        # URI mode=ro makes it impossible for an inspector/filter to create an index.
        connection = sqlite3.connect(db_path.resolve().as_uri() + "?mode=ro", uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    connection.execute(f"PRAGMA busy_timeout={30000 if writable else 5000}")
    return connection


def _initialize(conn: sqlite3.Connection, volume_id: str) -> None:
    version = conn.execute("PRAGMA user_version").fetchone()[0]
    if version not in (0, SCHEMA_VERSION):
        raise RuntimeError(f"Unsupported inventory schema: {version}")
    conn.executescript("""
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY, value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS scan_state (
            id INTEGER PRIMARY KEY CHECK(id=1), state TEXT NOT NULL,
            started_utc TEXT NOT NULL, updated_utc TEXT NOT NULL,
            completed_utc TEXT, indexed_files INTEGER NOT NULL DEFAULT 0,
            indexed_directories INTEGER NOT NULL DEFAULT 0,
            indexed_symlinks INTEGER NOT NULL DEFAULT 0,
            indexed_other INTEGER NOT NULL DEFAULT 0,
            last_directory TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS entries (
            path TEXT PRIMARY KEY, parent TEXT NOT NULL, path_fold TEXT NOT NULL,
            type TEXT NOT NULL, size_bytes INTEGER, modified_ns INTEGER,
            boundary TEXT
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_inventory_parent ON entries(parent,path_fold,path);
        CREATE INDEX IF NOT EXISTS idx_inventory_fold ON entries(path_fold,path);
        CREATE TABLE IF NOT EXISTS directories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            finished INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_inventory_pending ON directories(finished,id);
        CREATE TABLE IF NOT EXISTS errors (
            path TEXT NOT NULL, operation TEXT NOT NULL,
            message TEXT NOT NULL, PRIMARY KEY(path,operation)
        ) WITHOUT ROWID;
    """)
    existing = conn.execute("SELECT value FROM meta WHERE key='volume_id'").fetchone()
    if existing and existing[0] != volume_id:
        raise ValueError("Inventory index volume identity mismatch; refusing to reuse another volume's index")
    conn.execute("INSERT OR IGNORE INTO meta(key,value) VALUES('volume_id',?)", (volume_id,))
    conn.execute("PRAGMA user_version=1")
    conn.commit()


def _stored_volume_id(conn: sqlite3.Connection) -> str:
    row = conn.execute("SELECT value FROM meta WHERE key='volume_id'").fetchone()
    if row is None:
        raise ValueError("Inventory index has no volume identity")
    return str(row[0])


def _counts(conn: sqlite3.Connection) -> dict[str, int]:
    row = conn.execute("SELECT * FROM scan_state WHERE id=1").fetchone()
    if row is None:
        return {"file": 0, "directory": 0, "symlink": 0, "other": 0}
    return {kind: int(row[f"indexed_{column}"]) for kind, column in (
        ("file", "files"), ("directory", "directories"), ("symlink", "symlinks"), ("other", "other"))}


def _status(conn: sqlite3.Connection, mount_root: Path) -> dict[str, Any]:
    state = conn.execute("SELECT * FROM scan_state WHERE id=1").fetchone()
    counts = _counts(conn)
    errors = int(conn.execute("SELECT COUNT(*) FROM errors").fetchone()[0])
    pending = int(conn.execute("SELECT COUNT(*) FROM directories WHERE finished=0").fetchone()[0])
    return {
        "schema": "cortex.volume_inventory.persistent.v1", "root": str(mount_root),
        "volume_id": _stored_volume_id(conn), "state": state["state"] if state else "not_started",
        "started_utc": state["started_utc"] if state else None,
        "updated_utc": state["updated_utc"] if state else None,
        "completed_utc": state["completed_utc"] if state else None,
        "current_directory": state["last_directory"] if state else "",
        "counts": counts, "entries": sum(counts.values()), "errors": errors,
        "pending_directories": pending, "partial": bool(state and (state["state"] != "complete" or errors)),
        "source_read_only": True,
    }


def index_status(db_path: Path | str, mount_root: Path | str, *, volume_id: str | None = None) -> dict[str, Any]:
    db_path, mount_root = Path(db_path), Path(mount_root)
    if not db_path.is_file():
        return {"state": "not_started", "root": str(mount_root), "entries": 0,
                "counts": {k: 0 for k in ("file", "directory", "symlink", "other")},
                "errors": 0, "pending_directories": 0, "partial": False, "source_read_only": True}
    with closing(_connection(db_path, writable=False)) as conn:
        actual = _stored_volume_id(conn)
        if volume_id is not None and actual != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        return _status(conn, mount_root)


def _escape_like(text: str) -> str:
    return text.casefold().replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


@contextmanager
def _query_connection(db_path: Path, *, cancelled: Callable[[], bool] | None = None):
    """Bound SQL work and allow a superseded UI query to yield to the latest request.

    Queries remain read-only. SQLite invokes the handler during long scans even when
    the caller is waiting on a large leading-wildcard COUNT or ORDER BY.
    """
    with closing(_connection(db_path, writable=False)) as conn:
        started = time.monotonic()
        reason = [""]

        def progress() -> int:
            if cancelled is not None and cancelled():
                reason[0] = "superseded"
                return 1
            if time.monotonic() - started > MAX_QUERY_SECONDS:
                reason[0] = "timeout"
                return 1
            return 0

        conn.set_progress_handler(progress, 1000)
        try:
            if cancelled is not None and cancelled():
                raise RuntimeError("Inventory query superseded by a newer request")
            yield conn
        except sqlite3.OperationalError as exc:
            if "interrupted" in str(exc).lower():
                if reason[0] == "superseded":
                    raise RuntimeError("Inventory query superseded by a newer request") from exc
                if reason[0] == "timeout":
                    raise TimeoutError("Broad inventory search exceeded 8 seconds; narrow the search or use Projects/Git/Cortex shortcuts") from exc
            raise
        finally:
            conn.set_progress_handler(None, 0)


def query_entries(db_path: Path | str, *, volume_id: str, query: str = "", page: int = 0,
                  page_size: int = PAGE_SIZE, match_mode: str = "substring",
                  cancelled: Callable[[], bool] | None = None,
                  exact_total: bool = True) -> dict[str, Any]:
    """Bounded, cancellable pages. Root-folder shortcuts use an indexed prefix range.

    Explicit Filter retains literal substring semantics, but its duration is bounded.
    GUI substring previews can skip the full matching-count query; an extra fetched row
    provides next-page truth while retaining the original exact-count API by default.
    No full inventory is materialized in Python or written outside the index.
    """
    if page < 0 or not 1 <= page_size <= 500:
        raise ValueError("Invalid inventory page")
    if match_mode not in ("prefix", "substring"):
        raise ValueError("Invalid inventory matching mode")
    with _query_connection(Path(db_path), cancelled=cancelled) as conn:
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        needle = query.strip()
        if needle and match_mode == "prefix":
            # Using a BINARY range over case-folded paths allows idx_inventory_fold
            # to seek instead of scanning 2M+ rows with LIKE '%projects/%'.
            lower = needle.casefold()
            predicate, params = "path_fold >= ? AND path_fold < ?", (lower, lower + chr(0x10ffff))
        elif needle:
            predicate, params = "path_fold LIKE ? ESCAPE '\\'", ("%" + _escape_like(needle) + "%",)
        else:
            predicate, params = "parent=?", ("",)
        # An exact COUNT on a leading-wildcard LIKE scans the entire volume index.
        # Show the first page immediately for interactive searches instead.
        count_now = exact_total or not needle or match_mode == "prefix"
        total = (int(conn.execute(f"SELECT COUNT(*) FROM entries WHERE {predicate}", params).fetchone()[0])
                 if count_now else None)
        limit = page_size if count_now else page_size + 1
        fetched = conn.execute(
            f"SELECT path,type,size_bytes,modified_ns,boundary FROM entries WHERE {predicate} "
            "ORDER BY path_fold,path LIMIT ? OFFSET ?", (*params, limit, page * page_size)).fetchall()
        rows = fetched[:page_size]
        has_next = ((page + 1) * page_size < total if total is not None
                    else len(fetched) > page_size)
        if total is None and not has_next and (rows or page == 0):
            total = page * page_size + len(rows)
        return {"total": total, "rows": [dict(row) for row in rows], "page": page,
                "page_size": page_size, "query": needle, "match_mode": match_mode,
                "has_next": has_next}


def query_access_gaps(db_path: Path | str, *, volume_id: str, query: str = "", page: int = 0,
                      page_size: int = PAGE_SIZE,
                      cancelled: Callable[[], bool] | None = None) -> dict[str, Any]:
    """Paginated access-gap inspection, independent of expensive path searches."""
    if page < 0 or not 1 <= page_size <= 500:
        raise ValueError("Invalid access-gap page")
    with _query_connection(Path(db_path), cancelled=cancelled) as conn:
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        needle = query.strip()
        if needle:
            like = "%" + _escape_like(needle) + "%"
            predicate = "(path LIKE ? ESCAPE '\\' COLLATE NOCASE OR operation LIKE ? ESCAPE '\\' COLLATE NOCASE OR message LIKE ? ESCAPE '\\' COLLATE NOCASE)"
            params: tuple[str, ...] = (like, like, like)
        else:
            predicate, params = "1=1", ()
        total = int(conn.execute(f"SELECT COUNT(*) FROM errors WHERE {predicate}", params).fetchone()[0])
        rows = conn.execute(
            f"SELECT path,operation,message FROM errors WHERE {predicate} "
            "ORDER BY path COLLATE NOCASE,path,operation LIMIT ? OFFSET ?",
            (*params, page_size, page * page_size),
        ).fetchall()
        return {"total": total, "rows": [dict(row) for row in rows], "page": page,
                "page_size": page_size, "query": needle,
                "has_next": (page + 1) * page_size < total}


def _record_error(conn: sqlite3.Connection, path: str, operation: str, exc: OSError) -> None:
    conn.execute("INSERT INTO errors(path,operation,message) VALUES(?,?,?) ON CONFLICT(path,operation) "
                 "DO UPDATE SET message=excluded.message", (path or ".", operation, f"{type(exc).__name__}: {exc}"))


def _record_entry(conn: sqlite3.Connection, item: tuple[str, str, str, int | None, int | None, str | None],
                  additions: dict[str, int]) -> None:
    path, parent, kind, size, stamp, boundary = item
    cursor = conn.execute("INSERT OR IGNORE INTO entries(path,parent,path_fold,type,size_bytes,modified_ns,boundary) "
                          "VALUES(?,?,?,?,?,?,?)", (path, parent, path.casefold(), kind, size, stamp, boundary))
    if cursor.rowcount:
        additions[kind] += 1
    else:
        former = conn.execute("SELECT type FROM entries WHERE path=?", (path,)).fetchone()
        if former[0] != kind:
            additions[former[0]] -= 1
            additions[kind] += 1
        conn.execute("UPDATE entries SET parent=?,path_fold=?,type=?,size_bytes=?,modified_ns=?,boundary=? "
                     "WHERE path=?", (parent, path.casefold(), kind, size, stamp, boundary, path))
    conn.execute("DELETE FROM errors WHERE path=? AND operation='stat'", (path,))


def _flush(conn: sqlite3.Connection, additions: dict[str, int], current: str, *, state: str = "running") -> None:
    conn.execute("UPDATE scan_state SET state=?,updated_utc=?,indexed_files=indexed_files+?, "
                 "indexed_directories=indexed_directories+?,indexed_symlinks=indexed_symlinks+?, "
                 "indexed_other=indexed_other+?,last_directory=? WHERE id=1",
                 (state, _utc(), additions["file"], additions["directory"], additions["symlink"], additions["other"], current))
    conn.commit()
    additions.update({kind: 0 for kind in additions})


def run_scan(volume_root: Path | str, db_path: Path | str, *, volume_id: str,
             progress: Callable[[dict[str, Any]], None] | None = None,
             cancelled: Callable[[], bool] | None = None, batch_size: int = BATCH_SIZE,
             restart: bool = False) -> dict[str, Any]:
    """Explicit scan/resume; durable pending directories, no fixed entry cap.

    Interrupted directories remain pending. Re-enumeration is idempotent; committed
    batches and successfully scanned sibling directories are never discarded.
    """
    if batch_size < 1 or batch_size > 10000:
        raise ValueError("Invalid inventory batch size")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", volume_id):
        raise ValueError("Invalid volume identity")
    root = Path(volume_root).expanduser().resolve(strict=True)
    if not root.is_dir():
        raise NotADirectoryError(str(root))
    db_path = Path(db_path).expanduser().resolve()
    # Refuse to collide with the source repository's Vault catalog or marker.
    if db_path.name.casefold() != "inventory.sqlite3" or db_path == root / "vault_catalog.db":
        raise ValueError("Inventory database must be a dedicated inventory.sqlite3")
    excluded = None
    try:
        excluded = db_path.parent.relative_to(root).as_posix()
    except ValueError:
        pass
    drive = os.path.normcase(os.path.splitdrive(str(root))[0])
    root_device = root.stat().st_dev
    conn = _connection(db_path, writable=True)
    try:
        _initialize(conn, volume_id)
        if restart:
            conn.execute("DELETE FROM entries")
            conn.execute("DELETE FROM directories")
            conn.execute("DELETE FROM errors")
            conn.execute("DELETE FROM scan_state")
            conn.commit()
        initial = conn.execute("SELECT state FROM scan_state WHERE id=1").fetchone()
        if initial is None:
            stamp = _utc()
            conn.execute("INSERT INTO scan_state(id,state,started_utc,updated_utc) VALUES(1,'running',?,?)", (stamp, stamp))
            conn.execute("INSERT INTO directories(path) VALUES('')")
            conn.commit()
        elif initial[0] == "complete":
            return _status(conn, root)  # rescan requires explicit restart=True
        else:
            conn.execute("UPDATE scan_state SET state='running',updated_utc=? WHERE id=1", (_utc(),))
            conn.commit()
        last_emit = 0.0
        since_commit = 0
        additions = {kind: 0 for kind in ("file", "directory", "symlink", "other")}
        while True:
            if cancelled and cancelled():
                _flush(conn, additions, "", state="paused")
                break
            pending = conn.execute("SELECT id,path FROM directories WHERE finished=0 ORDER BY id LIMIT 1").fetchone()
            if pending is None:
                _flush(conn, additions, "", state="complete")
                conn.execute("UPDATE scan_state SET completed_utc=? WHERE id=1", (_utc(),))
                conn.commit()
                break
            qid, relative = pending["id"], pending["path"]
            folder = root.joinpath(*relative.split("/")) if relative else root
            interrupted = False
            try:
                with os.scandir(folder) as iterator:
                    for child in iterator:
                        if cancelled and cancelled():
                            interrupted = True
                            break
                        rel = f"{relative}/{child.name}" if relative else child.name
                        try:
                            info = child.stat(follow_symlinks=False)
                            kind = ("symlink" if stat.S_ISLNK(info.st_mode) else
                                    "directory" if stat.S_ISDIR(info.st_mode) else
                                    "file" if stat.S_ISREG(info.st_mode) else "other")
                            boundary = None
                            if kind == "directory":
                                attrs = getattr(info, "st_file_attributes", 0)
                                reparse = bool(attrs & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0))
                                child_drive = os.path.normcase(os.path.splitdrive(child.path)[0])
                                if excluded is not None and rel == excluded:
                                    boundary = "inventory_index"
                                elif os.name == "nt":
                                    if reparse:
                                        boundary = "reparse_point"
                                    elif child_drive != drive:
                                        boundary = "different_device"
                                elif info.st_dev != root_device:
                                    boundary = "different_device"
                                elif os.path.ismount(child.path):
                                    boundary = "mount_point"
                                if boundary is None:
                                    conn.execute("INSERT OR IGNORE INTO directories(path) VALUES(?)", (rel,))
                            _record_entry(conn, (rel, relative, kind,
                                                 info.st_size if kind == "file" else None,
                                                 info.st_mtime_ns, boundary), additions)
                        except OSError as exc:
                            _record_error(conn, rel, "stat", exc)
                        since_commit += 1
                        if since_commit >= batch_size:
                            _flush(conn, additions, relative)
                            since_commit = 0
                            if progress and time.monotonic() - last_emit >= .35:
                                progress(_status(conn, root))
                                last_emit = time.monotonic()
            except OSError as exc:
                _record_error(conn, relative, "scandir", exc)
            if interrupted:
                _flush(conn, additions, relative, state="paused")
                break  # keep unfinished directory pending, with durable batch records
            conn.execute("UPDATE directories SET finished=1 WHERE id=?", (qid,))
            _flush(conn, additions, relative)
            if progress and time.monotonic() - last_emit >= .35:
                progress(_status(conn, root))
                last_emit = time.monotonic()
        result = _status(conn, root)
        if progress:
            progress(result)
        return result
    except BaseException:
        conn.rollback()
        raise
    finally:
        conn.close()


def list_errors(db_path: Path | str, *, volume_id: str, limit: int = 20) -> list[dict[str, str]]:
    if not 1 <= limit <= 100:
        raise ValueError("Invalid error sample limit")
    with closing(_connection(Path(db_path), writable=False)) as conn:
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        return [dict(row) for row in conn.execute(
            "SELECT path,operation,message FROM errors ORDER BY path LIMIT ?", (limit,))]
