"""R8H: persistent, bounded-memory metadata inventory for an explicitly selected volume.

Only Cortex's dedicated inventory SQLite index is written.  Source content, project
registrations, the existing Vault catalog and filesystem metadata are never changed.
The durable directory queue permits idempotent resumption after cancellation/crash.
"""
from __future__ import annotations

import os
import errno
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


def _flush(conn: sqlite3.Connection, additions: dict[str, int], current: str, *,
           state: str = "running", scan_token: str | None = None) -> None:
    # The heartbeat is committed in the same transaction as the progress counters.
    # A worker that has lost its lease must never continue publishing scan batches.
    if scan_token is not None:
        held = conn.execute("UPDATE inventory_scan_owner SET updated_utc=? WHERE id=1 AND token=?",
                            (_utc(), scan_token))
        if held.rowcount != 1:
            raise RuntimeError("Inventory scanner no longer owns the durable scan lease")
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
    scan_token: str | None = None
    try:
        _initialize(conn, volume_id)
        # BEGIN IMMEDIATE serializes lease claims across Cortex processes. Merely
        # checking scan_state='running' is insufficient: that state is retained
        # after process death and was previously accepted by *both* live workers.
        conn.execute("BEGIN IMMEDIATE")
        if conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='recursive_refresh'").fetchone():
            job = conn.execute("SELECT state FROM recursive_refresh WHERE id=1").fetchone()
            if job and job[0] in ('running', 'paused'):
                raise RuntimeError("Finish or resume the recursive refresh before starting the inventory scanner")
        initial = conn.execute("SELECT state FROM scan_state WHERE id=1").fetchone()
        if initial is not None and initial[0] == "complete" and not restart:
            result = _status(conn, root)
            conn.rollback()  # a completed index is a read-only no-op; no lease table added
            return result
        # This auxiliary table is additive within the existing dedicated index;
        # no entries, counters, Vault catalog or schema version are rebuilt.
        conn.execute("CREATE TABLE IF NOT EXISTS inventory_scan_owner ("
                     "id INTEGER PRIMARY KEY CHECK(id=1), owner_pid INTEGER NOT NULL, "
                     "token TEXT NOT NULL, updated_utc TEXT NOT NULL)")
        prior = conn.execute("SELECT owner_pid FROM inventory_scan_owner WHERE id=1").fetchone()
        if prior is not None and _owner_alive(int(prior['owner_pid'])):
            raise RuntimeError("Inventory scanner is already active in another Cortex worker")
        scan_token = uuid.uuid4().hex
        conn.execute("INSERT INTO inventory_scan_owner(id,owner_pid,token,updated_utc) "
                     "VALUES(1,?,?,?) ON CONFLICT(id) DO UPDATE SET "
                     "owner_pid=excluded.owner_pid,token=excluded.token,updated_utc=excluded.updated_utc",
                     (os.getpid(), scan_token, _utc()))
        if restart:
            conn.execute("DELETE FROM entries")
            conn.execute("DELETE FROM directories")
            conn.execute("DELETE FROM errors")
            conn.execute("DELETE FROM scan_state")
            initial = None
        if initial is None:
            stamp = _utc()
            conn.execute("INSERT INTO scan_state(id,state,started_utc,updated_utc) VALUES(1,'running',?,?)", (stamp, stamp))
            conn.execute("INSERT INTO directories(path) VALUES('')")
        else:
            conn.execute("UPDATE scan_state SET state='running',updated_utc=? WHERE id=1", (_utc(),))
        conn.commit()
        last_emit = 0.0
        since_commit = 0
        additions = {kind: 0 for kind in ("file", "directory", "symlink", "other")}
        while True:
            if cancelled and cancelled():
                _flush(conn, additions, "", state="paused", scan_token=scan_token)
                break
            pending = conn.execute("SELECT id,path FROM directories WHERE finished=0 ORDER BY id LIMIT 1").fetchone()
            if pending is None:
                _flush(conn, additions, "", state="complete", scan_token=scan_token)
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
                            _flush(conn, additions, relative, scan_token=scan_token)
                            since_commit = 0
                            if progress and time.monotonic() - last_emit >= .35:
                                progress(_status(conn, root))
                                last_emit = time.monotonic()
                # Successful enumeration supersedes an older transient scandir
                # error (including the special volume-root display key '.').
                # Keep stat errors for children which are still inaccessible.
                conn.execute("DELETE FROM errors WHERE path=? AND operation='scandir'", (relative or '.',))
            except OSError as exc:
                _record_error(conn, relative, "scandir", exc)
            if interrupted:
                _flush(conn, additions, relative, state="paused", scan_token=scan_token)
                break  # keep unfinished directory pending, with durable batch records
            conn.execute("UPDATE directories SET finished=1 WHERE id=?", (qid,))
            _flush(conn, additions, relative, scan_token=scan_token)
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
        if scan_token is not None:
            # Release only our own token; a later owner may have recovered a
            # genuinely dead worker. Unexpected worker failure leaves the durable
            # scan queue paused rather than falsely advertising an active scan.
            try:
                conn.rollback()
                conn.execute("BEGIN IMMEDIATE")
                released = conn.execute("DELETE FROM inventory_scan_owner WHERE id=1 AND token=?",
                                        (scan_token,))
                if released.rowcount:
                    conn.execute("UPDATE scan_state SET state='paused',completed_utc=NULL,updated_utc=? "
                                 "WHERE id=1 AND state='running'", (_utc(),))
                conn.commit()
            except sqlite3.Error:
                conn.rollback()
        conn.close()



def _refresh_relative(value: str, excluded: str | None) -> str:
    """Normalize a volume-relative directory without resolving links or changing source."""
    if not isinstance(value, str) or "\x00" in value:
        raise ValueError("Invalid relative inventory directory")
    raw = value.replace("\\", "/").strip("/")
    if (value.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:", value)
            or any(part in ("", ".", "..") for part in raw.split("/") if raw)):
        raise ValueError("Directory must be a safe volume-relative path")
    if excluded and (raw == excluded or raw.startswith(excluded + "/")):
        raise ValueError("Cannot inventory the dedicated inventory index")
    return raw


def _delete_indexed_tree(conn: sqlite3.Connection, relative: str, changes: dict[str, int]) -> None:
    """Delete one removed file or an entire missing directory via the path B-tree."""
    lower, upper = relative + "/", relative + "/" + chr(0x10ffff)
    predicate = "(path=? OR (path>=? AND path<?))"
    args = (relative, lower, upper)
    for row in conn.execute(f"SELECT type,COUNT(*) AS n FROM entries WHERE {predicate} GROUP BY type", args):
        changes[row["type"]] -= int(row["n"])
    conn.execute(f"DELETE FROM entries WHERE {predicate}", args)
    conn.execute(f"DELETE FROM directories WHERE {predicate}", args)
    conn.execute(f"DELETE FROM errors WHERE {predicate}", args)
    # A recursive job may already have queued descendants. Prune those work items
    # in the same transaction as the removed metadata, rather than repeatedly
    # retrying paths which the parent has now proved no longer exist.
    if conn.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='recursive_refresh_queue'").fetchone():
        conn.execute(f"DELETE FROM recursive_refresh_queue WHERE {predicate}", args)


def refresh_directory(volume_root: Path | str, db_path: Path | str, *, volume_id: str,
                      relative: str = "", cancelled: Callable[[], bool] | None = None,
                      _recursive: bool = False) -> dict[str, Any]:
    """R8J-A: atomically reconcile one explicit directory, without rebuilding a volume.

    Stage immediate children in a connection-local SQLite TEMP table; no multi-million-row
    Python lists are created. Deleted subtrees are removed from the index and counts.
    Newly discovered folders are queued for the existing resumable scanner. Existing
    child folders are NOT recursively rescanned by this single-directory operation.
    If enumeration fails or is cancelled, prior index entries remain intact.
    """
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", volume_id):
        raise ValueError("Invalid volume identity")
    root = Path(volume_root).expanduser().resolve(strict=True)
    db = Path(db_path).expanduser().resolve()
    if not root.is_dir() or not db.is_file() or db.name.casefold() != "inventory.sqlite3":
        raise ValueError("A completed, dedicated inventory and valid volume root are required")
    try:
        excluded = db.parent.relative_to(root).as_posix()
    except ValueError:
        excluded = None
    rel = _refresh_relative(relative, excluded)
    folder = root.joinpath(*rel.split("/")) if rel else root
    drive = os.path.normcase(os.path.splitdrive(str(root))[0])
    root_device = root.stat().st_dev
    # A previously indexed directory might have been replaced with a symlink or
    # Windows junction since the last scan. Re-check every path component before
    # opening it; the historical entry's boundary flag alone is not sufficient.
    current_path = root
    for component in rel.split("/") if rel else ():
        current_path = current_path / component
        item_info = current_path.lstat()
        attrs = getattr(item_info, "st_file_attributes", 0)
        if (not stat.S_ISDIR(item_info.st_mode) or stat.S_ISLNK(item_info.st_mode)
                or attrs & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
                or (os.name == "nt" and os.path.normcase(os.path.splitdrive(str(current_path))[0]) != drive)
                or (os.name != "nt" and (item_info.st_dev != root_device or os.path.ismount(current_path)))):
            raise ValueError("Refresh cannot traverse a changed link, junction, mount or non-directory; refresh its parent")
    with closing(_connection(db, writable=True)) as conn:
        _initialize(conn, volume_id)
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        # Validate active scan/tree authority under the SAME writer reservation as
        # the eventual directory reconciliation, not in a stale pre-lock read.
        conn.execute("BEGIN IMMEDIATE")
        current = conn.execute("SELECT state FROM scan_state WHERE id=1").fetchone()
        active = None
        if conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='recursive_refresh'").fetchone():
            active = conn.execute("SELECT state,owner_pid FROM recursive_refresh WHERE id=1").fetchone()
        if _recursive:
            if not active or active['state'] != 'running' or active['owner_pid'] != os.getpid():
                raise RuntimeError("Recursive directory refresh has no active owned session")
            if current is None or current[0] not in ('complete', 'paused'):
                raise RuntimeError("Inventory scanner must be stopped before recursive refresh")
        else:
            if active and active['state'] in ('running', 'paused'):
                raise RuntimeError("Finish the recursive refresh before using single-directory refresh")
            if current is None or current[0] != "complete":
                raise RuntimeError("Refresh requires a completed inventory; pause/resume the initial scan first")
        if rel:
            host = conn.execute("SELECT type,boundary FROM entries WHERE path=?", (rel,)).fetchone()
            if host is None or host["type"] != "directory" or host["boundary"]:
                raise ValueError("Refresh requires a previously indexed, traversable directory")
        conn.execute("CREATE TEMP TABLE refresh_seen (path TEXT PRIMARY KEY, valid INTEGER NOT NULL, "
                     "parent TEXT, kind TEXT, size_bytes INTEGER, modified_ns INTEGER, boundary TEXT) WITHOUT ROWID")
        try:
            # Never treat an inaccessible directory as an empty directory: that could
            # otherwise remove valid metadata for thousands of existing source files.
            with os.scandir(folder) as scanner:
                for child in scanner:
                    if cancelled is not None and cancelled():
                        raise InterruptedError("Directory refresh cancelled; previous index retained")
                    child_rel = f"{rel}/{child.name}" if rel else child.name
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
                            if excluded and child_rel == excluded:
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
                        conn.execute("INSERT INTO refresh_seen VALUES(?,?,?,?,?,?,?)",
                                     (child_rel, 1, rel, kind,
                                      info.st_size if kind == "file" else None,
                                      info.st_mtime_ns, boundary))
                    except OSError as exc:
                        # A stat failure must not turn an existing file into a deletion.
                        conn.execute("INSERT INTO refresh_seen(path,valid) VALUES(?,0)", (child_rel,))
                        _record_error(conn, child_rel, "stat", exc)
            if cancelled is not None and cancelled():
                raise InterruptedError("Directory refresh cancelled; previous index retained")
            delta = {kind: 0 for kind in ("file", "directory", "symlink", "other")}
            outcome = {"added": 0, "changed": 0, "removed": 0, "unchanged": 0, "stat_errors": 0,
                       "new_directories": 0, "directory": rel or "."}
            outcome["stat_errors"] = int(conn.execute("SELECT COUNT(*) FROM refresh_seen WHERE valid=0").fetchone()[0])
            conn.execute("CREATE TEMP TABLE refresh_missing(path TEXT PRIMARY KEY,type TEXT NOT NULL) WITHOUT ROWID")
            conn.execute("INSERT INTO refresh_missing SELECT path,type FROM entries WHERE parent=? AND NOT EXISTS "
                         "(SELECT 1 FROM refresh_seen WHERE refresh_seen.path=entries.path)", (rel,))
            for row in conn.execute("SELECT path,type FROM refresh_missing ORDER BY path"):
                outcome["removed"] += 1
                _delete_indexed_tree(conn, row["path"], delta)
            for row in conn.execute("SELECT * FROM refresh_seen WHERE valid=1 ORDER BY path"):
                path, kind = row["path"], row["kind"]
                previous = conn.execute("SELECT type,size_bytes,modified_ns,boundary FROM entries WHERE path=?",
                                        (path,)).fetchone()
                new_value = (kind, row["size_bytes"], row["modified_ns"], row["boundary"])
                if previous is None:
                    outcome["added"] += 1
                elif tuple(previous) != new_value:
                    outcome["changed"] += 1
                else:
                    outcome["unchanged"] += 1
                if previous is not None and previous["type"] == "directory" and (kind != "directory" or row["boundary"] is not None):
                    # A former folder turned into a file/link. Its descendants no longer exist.
                    lower, upper = path + "/", path + "/" + chr(0x10ffff)
                    for old in conn.execute("SELECT type,COUNT(*) AS n FROM entries WHERE path>=? AND path<? GROUP BY type",
                                            (lower, upper)):
                        delta[old["type"]] -= int(old["n"])
                    conn.execute("DELETE FROM entries WHERE path>=? AND path<?", (lower, upper))
                    conn.execute("DELETE FROM directories WHERE path=? OR (path>=? AND path<?)",
                                 (path, lower, upper))
                    conn.execute("DELETE FROM errors WHERE path>=? AND path<?", (lower, upper))
                    # The old host is no longer a traversable folder: its prior
                    # directory-level access gaps no longer describe this path.
                    conn.execute("DELETE FROM errors WHERE path=? AND operation IN ('scandir','refresh_scandir')", (path,))
                    if conn.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='recursive_refresh_queue'").fetchone():
                        conn.execute("DELETE FROM recursive_refresh_queue WHERE path=? OR (path>=? AND path<?)",
                                     (path, lower, upper))
                if previous is None or tuple(previous) != new_value:
                    _record_entry(conn, (path, rel, kind, row["size_bytes"], row["modified_ns"], row["boundary"]), delta)
                else:
                    conn.execute("DELETE FROM errors WHERE path=? AND operation='stat'", (path,))
                if kind == "directory" and row["boundary"] is None and (previous is None or previous["type"] != "directory" or previous["boundary"] is not None):
                    conn.execute("INSERT INTO directories(path,finished) VALUES(?,0) "
                                 "ON CONFLICT(path) DO UPDATE SET finished=0", (path,))
                    outcome["new_directories"] += 1
            # A previous transient access denial must clear when this same
            # directory is successfully enumerated. Preserve unrelated stat gaps.
            # The volume-root display sentinel is '.', while its stored entry
            # parent is ''. A successful root refresh must clear its old gap too.
            conn.execute("DELETE FROM errors WHERE path=? AND operation IN ('scandir','refresh_scandir')", (rel or '.',))
            pending = int(conn.execute("SELECT COUNT(*) FROM directories WHERE finished=0").fetchone()[0])
            conn.execute("UPDATE scan_state SET updated_utc=?,state=?,completed_utc=?,last_directory=?, "
                         "indexed_files=indexed_files+?,indexed_directories=indexed_directories+?, "
                         "indexed_symlinks=indexed_symlinks+?,indexed_other=indexed_other+? WHERE id=1",
                         (_utc(), "paused" if pending else "complete", None if pending else _utc(), rel,
                          delta["file"], delta["directory"], delta["symlink"], delta["other"]))
            conn.commit()
            return {"refresh": outcome, "status": _status(conn, root)}
        except InterruptedError:
            conn.rollback()
            raise
        except OSError as exc:
            conn.rollback()
            # Do not delete any old record if the folder itself is inaccessible.
            _record_error(conn, rel or ".", "refresh_scandir", exc)
            conn.commit()
            return {"refresh": {"directory": rel or ".", "unreadable": str(exc),
                                "added": 0, "changed": 0, "removed": 0},
                    "status": _status(conn, root)}
        except BaseException:
            conn.rollback()
            raise


def list_errors(db_path: Path | str, *, volume_id: str, limit: int = 20) -> list[dict[str, str]]:
    if not 1 <= limit <= 100:
        raise ValueError("Invalid error sample limit")
    with closing(_connection(Path(db_path), writable=False)) as conn:
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        return [dict(row) for row in conn.execute(
            "SELECT path,operation,message FROM errors ORDER BY path LIMIT ?", (limit,))]


# R8J-B: additive work tables within the existing dedicated inventory database.
# The v1 entries/schema remain readable; no full rebuild or Vault catalog mutation.
def _recursive_tables(conn: sqlite3.Connection) -> None:
    conn.executescript("""
        CREATE TABLE IF NOT EXISTS recursive_refresh (
            id INTEGER PRIMARY KEY CHECK (id=1), root TEXT NOT NULL,
            state TEXT NOT NULL, owner_pid INTEGER,
            started_utc TEXT NOT NULL, updated_utc TEXT NOT NULL,
            refreshed INTEGER NOT NULL DEFAULT 0,
            added INTEGER NOT NULL DEFAULT 0,
            changed INTEGER NOT NULL DEFAULT 0,
            removed INTEGER NOT NULL DEFAULT 0,
            unreadable INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS recursive_refresh_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE, finished INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_recursive_pending ON recursive_refresh_queue(finished,id);
    """)


def recursive_refresh_status(db_path: Path | str, *, volume_id: str) -> dict[str, Any]:
    """Inspect an optional refresh job without creating a database or altering source."""
    with closing(_connection(Path(db_path), writable=False)) as conn:
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to a different volume")
        if conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='recursive_refresh'").fetchone() is None:
            return {"state": "not_started", "pending": 0}
        row = conn.execute("SELECT * FROM recursive_refresh WHERE id=1").fetchone()
        if row is None:
            return {"state": "not_started", "pending": 0}
        pending = conn.execute("SELECT COUNT(*) FROM recursive_refresh_queue WHERE finished=0").fetchone()[0]
        return {**dict(row), "pending": int(pending)}


def _windows_owner_alive(pid: int, *, kernel32: Any = None,
                         last_error: Callable[[], int] | None = None) -> bool:
    """Non-signalling Windows process check. Never use os.kill(pid, 0) here.

    On Windows, signal zero can be interpreted as CTRL_C_EVENT. Request only
    PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE and poll the handle without
    signalling, terminating, or otherwise modifying the target process.
    Unknown/access-denied conditions fail closed (assume the owner is alive).
    """
    import ctypes
    from ctypes import wintypes
    api = kernel32 if kernel32 is not None else ctypes.WinDLL('kernel32', use_last_error=True)
    api.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    api.OpenProcess.restype = wintypes.HANDLE
    api.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    api.WaitForSingleObject.restype = wintypes.DWORD
    api.CloseHandle.argtypes = (wintypes.HANDLE,)
    api.CloseHandle.restype = wintypes.BOOL
    handle = api.OpenProcess(0x101000, False, pid)  # QUERY_LIMITED_INFORMATION | SYNCHRONIZE
    if not handle:
        code = int((last_error or ctypes.get_last_error)())
        return code not in (87, 1168)  # INVALID_PARAMETER, NOT_FOUND
    try:
        status = int(api.WaitForSingleObject(handle, 0))
        return status != 0  # WAIT_OBJECT_0 is a terminated owner; unknown fails closed
    finally:
        api.CloseHandle(handle)


def _owner_alive(pid: int | None) -> bool:
    if not pid or pid <= 0:
        return False
    if os.name == 'nt':
        return _windows_owner_alive(pid)
    try:
        os.kill(pid, 0)  # POSIX only: a non-signalling process existence probe
        return True
    except ProcessLookupError:
        return False
    except OSError as exc:
        if exc.errno == errno.ESRCH:
            return False
        # Permission or unknown errors must not permit a conflicting writer.
        return True


def refresh_tree(volume_root: Path | str, db_path: Path | str, *, volume_id: str,
                 relative: str | None = None, cancelled: Callable[[], bool] | None = None,
                 progress: Callable[[dict[str, Any]], None] | None = None) -> dict[str, Any]:
    """Recursively reconcile a selected, indexed tree with a durable SQLite work queue.

    Every directory is reconciled atomically through refresh_directory. A pause or
    process death leaves prior directory commits and the remaining work queue intact.
    No guessed rename identity, content hashes, automatic registration, or source edits.
    A missing selected root must be refreshed through its parent (fail closed).
    """
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", volume_id):
        raise ValueError("Invalid volume identity")
    root = Path(volume_root).expanduser().resolve(strict=True)
    db = Path(db_path).expanduser().resolve()
    if not root.is_dir() or not db.is_file() or db.name.casefold() != 'inventory.sqlite3':
        raise ValueError("A dedicated, existing inventory database and volume root are required")
    excluded = None
    try:
        excluded = db.parent.relative_to(root).as_posix()
    except ValueError:
        pass
    if relative is not None:
        relative = _refresh_relative(relative, excluded)
    with closing(_connection(db, writable=True)) as conn:
        _initialize(conn, volume_id)
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to another volume")
        _recursive_tables(conn)
        # Claim/resume the durable job under a SQLite write reservation. A
        # second Cortex process must not observe a paused/unclaimed queue and
        # simultaneously take ownership of the same recursive reconciliation.
        conn.execute("BEGIN IMMEDIATE")
        job = conn.execute("SELECT * FROM recursive_refresh WHERE id=1").fetchone()
        if job and job['state'] in ('running', 'paused'):
            if relative is not None and relative != job['root']:
                raise RuntimeError("An unfinished recursive refresh has a different root; resume that job first")
            if job['state'] == 'running' and _owner_alive(job['owner_pid']):
                raise RuntimeError("Recursive refresh is already active in another worker")
            relative = job['root']
        else:
            if relative is None:
                raise ValueError("Select an indexed directory before starting a recursive refresh")
            state = conn.execute("SELECT state FROM scan_state WHERE id=1").fetchone()
            if state is None or state[0] != 'complete':
                raise RuntimeError("Finish the initial scanner before starting a recursive refresh")
            if relative:
                host = conn.execute("SELECT type,boundary FROM entries WHERE path=?", (relative,)).fetchone()
                if host is None or host['type'] != 'directory' or host['boundary']:
                    raise ValueError("Choose a previously indexed, traversable directory")
            # Preflight all path components before recording a job. This detects
            # replaced Windows junctions and directory symlinks at the outset.
            _preflight_refresh_host(root, relative)
            conn.execute("DELETE FROM recursive_refresh_queue")
            stamp = _utc()
            conn.execute("INSERT INTO recursive_refresh(id,root,state,owner_pid,started_utc,updated_utc) "
                         "VALUES(1,?,'running',?,?,?) ON CONFLICT(id) DO UPDATE SET "
                         "root=excluded.root,state='running',owner_pid=excluded.owner_pid,"
                         "started_utc=excluded.started_utc,updated_utc=excluded.updated_utc,"
                         "refreshed=0,added=0,changed=0,removed=0,unreadable=0",
                         (relative, os.getpid(), stamp, stamp))
            conn.execute("INSERT INTO recursive_refresh_queue(path) VALUES(?)", (relative,))
            conn.commit()
        conn.execute("UPDATE recursive_refresh SET state='running',owner_pid=?,updated_utc=? WHERE id=1",
                     (os.getpid(), _utc()))
        conn.commit()

    def pause() -> dict[str, Any]:
        with closing(_connection(db, writable=True)) as conn:
            conn.execute("UPDATE recursive_refresh SET state='paused',owner_pid=NULL,updated_utc=? WHERE id=1",
                         (_utc(),))
            conn.commit()
        return recursive_refresh_status(db, volume_id=volume_id)

    last_emit = 0.0
    while True:
        if cancelled and cancelled():
            return pause()
        with closing(_connection(db, writable=True)) as conn:
            row = conn.execute("SELECT path FROM recursive_refresh_queue WHERE finished=0 ORDER BY id LIMIT 1").fetchone()
            if row is None:
                # All new directories discovered by the targeted refresh were also
                # traversed here. Do not dispatch the unrelated whole-volume scanner.
                pending = conn.execute("SELECT COUNT(*) FROM directories WHERE finished=0").fetchone()[0]
                conn.execute("UPDATE scan_state SET state=?,completed_utc=?,updated_utc=? WHERE id=1",
                             ('complete' if not pending else 'paused', _utc() if not pending else None, _utc()))
                conn.execute("UPDATE recursive_refresh SET state='complete',owner_pid=NULL,updated_utc=? WHERE id=1",
                             (_utc(),))
                conn.commit()
                break
            selected = str(row['path'])
        try:
            result = refresh_directory(root, db, volume_id=volume_id, relative=selected,
                                       cancelled=cancelled, _recursive=True)
        except InterruptedError:
            return pause()
        except (FileNotFoundError, NotADirectoryError, ValueError) as exc:
            # A queued child may disappear or become a link after its parent was already committed.
            # Reconcile the nearest existing ancestor inside the selected tree,
            # rather than claiming stale children or traversing a new link.
            with closing(_connection(db, writable=False)) as check:
                job_root = check.execute("SELECT root FROM recursive_refresh WHERE id=1").fetchone()[0]
            parent = selected.rpartition('/')[0]
            if selected == job_root or not (parent == job_root or parent.startswith(job_root + '/') or not job_root):
                pause()
                raise RuntimeError(f"Selected refresh root is gone: {selected!r}; refresh its parent after clearing the job") from exc
            ancestor = parent
            while True:
                try:
                    _preflight_refresh_host(root, ancestor)
                    break
                except (FileNotFoundError, NotADirectoryError):
                    if ancestor == job_root:
                        pause()
                        raise RuntimeError(f"Selected refresh root disappeared: {job_root!r}") from exc
                    ancestor = ancestor.rpartition('/')[0]
            try:
                result = refresh_directory(root, db, volume_id=volume_id, relative=ancestor,
                                           cancelled=cancelled, _recursive=True)
            except Exception:
                pause()
                raise
            # The missing work item was superseded by its parent reconciliation.
            # Other queued records are idempotent and revalidated before opening.
        except Exception:
            pause()
            raise
        change = result['refresh']
        with closing(_connection(db, writable=True)) as conn:
            conn.execute("BEGIN IMMEDIATE")
            # Reprocessing after a crash between directory commit and queue commit
            # remains idempotent; the current metadata is source-of-truth.
            # refresh_directory uses '.' solely to *display* the volume root.
            # The index and work queue use '' for that root. Do not query
            # parent='.' here, or a root-scoped recursive job would silently
            # finish without enqueuing any of the root's child directories.
            enumerated = str(change.get('directory', selected))
            if enumerated == '.':
                enumerated = ''
            for child in conn.execute("SELECT path FROM entries WHERE parent=? AND type='directory' "
                                      "AND boundary IS NULL ORDER BY path", (enumerated,)):
                conn.execute("INSERT OR IGNORE INTO recursive_refresh_queue(path) VALUES(?)", (child['path'],))
            conn.execute("UPDATE recursive_refresh_queue SET finished=1 WHERE path=?", (selected,))
            conn.execute("UPDATE directories SET finished=1 WHERE path=?", (selected,))
            conn.execute("UPDATE recursive_refresh SET refreshed=refreshed+1,added=added+?,"
                         "changed=changed+?,removed=removed+?,unreadable=unreadable+?,updated_utc=? WHERE id=1",
                         (int(change.get('added',0)),int(change.get('changed',0)),int(change.get('removed',0)),
                          int(bool(change.get('unreadable'))),_utc()))
            conn.commit()
        if progress and time.monotonic() - last_emit >= .35:
            progress(recursive_refresh_status(db, volume_id=volume_id))
            last_emit = time.monotonic()
    return recursive_refresh_status(db, volume_id=volume_id)


def _preflight_refresh_host(root: Path, relative: str) -> None:
    """Refuse links, reparse points, and mounts at every component without following them."""
    drive = os.path.normcase(os.path.splitdrive(str(root))[0])
    root_device = root.stat().st_dev
    current = root
    for part in relative.split('/') if relative else ():
        current = current / part
        info = current.lstat()
        attrs = getattr(info, 'st_file_attributes', 0)
        if (not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode)
                or attrs & getattr(stat, 'FILE_ATTRIBUTE_REPARSE_POINT', 0)
                or (os.name == 'nt' and os.path.normcase(os.path.splitdrive(str(current))[0]) != drive)
                or (os.name != 'nt' and (info.st_dev != root_device or os.path.ismount(current)))):
            raise ValueError('Recursive refresh cannot traverse a link, junction, mount or non-directory')


def abandon_recursive_refresh(db_path: Path | str, *, volume_id: str) -> dict[str, Any]:
    """Explicitly discard *only* a paused/stale work queue, never indexed records.

    This recovery action is needed when the selected root itself has been removed or
    replaced. It does not imply that the index is complete or reconcile that root;
    the user must refresh its still-existing parent explicitly afterward.
    """
    db = Path(db_path)
    with closing(_connection(db, writable=True)) as conn:
        _initialize(conn, volume_id)
        if _stored_volume_id(conn) != volume_id:
            raise ValueError("Inventory belongs to another volume")
        if conn.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='recursive_refresh'").fetchone() is None:
            raise RuntimeError("No recursive refresh queue exists")
        # Claim SQLite's writer reservation BEFORE inspecting the owner/state.
        # Otherwise a paused queue can be resumed by another process between
        # the read and BEGIN, and an outdated discard would erase live work.
        conn.execute("BEGIN IMMEDIATE")
        row = conn.execute("SELECT state,owner_pid FROM recursive_refresh WHERE id=1").fetchone()
        if row is None or row['state'] not in ('running','paused'):
            raise RuntimeError("No unfinished recursive refresh queue exists")
        if row['state'] == 'running' and _owner_alive(row['owner_pid']):
            raise RuntimeError("Cannot discard a live recursive refresh")
        conn.execute("DELETE FROM recursive_refresh_queue")
        conn.execute("UPDATE recursive_refresh SET state='abandoned',owner_pid=NULL,updated_utc=? WHERE id=1", (_utc(),))
        pending=conn.execute("SELECT COUNT(*) FROM directories WHERE finished=0").fetchone()[0]
        conn.execute("UPDATE scan_state SET state=?,completed_utc=?,updated_utc=? WHERE id=1",
                     ('paused' if pending else 'complete', None if pending else _utc(), _utc()))
        conn.commit()
        return {"state":'abandoned',"pending_scan_directories":int(pending)}
