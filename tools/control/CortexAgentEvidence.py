#!/usr/bin/env python3
"""Read-only PCC/CLI presentation and worker-trace inspection helpers.

The Python chat cache is *not* the authoritative Rust ConversationStore. These
helpers never create a conversation, resume a Rust operation, or infer success
from text. Actual coding authority remains in the Rust common controller.
"""
from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from PCCChatStore import list_conversations, load_conversation, conversation_key, conversation_path

OPERATION_ID = re.compile(r"[0-9a-f]{32}\Z")
MAX_TRACE_BYTES = 2 * 1024 * 1024


def conversations(cortex_root: Path, workspace: Path, *, limit: int = 10) -> list[dict[str, Any]]:
    """Return bounded titles and identifiers from the existing PCC presentation cache."""
    return list_conversations(cortex_root, workspace)[:max(0, min(limit, 50))]


def history(cortex_root: Path, workspace: Path, conversation_id: str) -> dict[str, Any] | None:
    """Read only an existing cache record; do not synthesize a new conversation."""
    if conversation_key(conversation_id) != conversation_id:
        return None
    candidate = conversation_path(cortex_root, workspace, conversation_id)
    if candidate.is_symlink() or not candidate.is_file():
        return None
    record = load_conversation(cortex_root, workspace, conversation_id)
    return record if record.get("conversationId") == conversation_id else None


def operation_directory(cortex_root: Path) -> Path:
    return cortex_root / "artifacts" / "logs" / "cortex-agent"


def _trace_record(path: Path) -> dict[str, Any] | None:
    """Inspect only small metadata-only JSONL traces, without arbitrary file paths."""
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_TRACE_BYTES:
        return None
    first: dict[str, Any] | None = None
    last: dict[str, Any] | None = None
    count = 0
    with path.open("r", encoding="utf-8", errors="replace") as source:
        for line in source:
            if len(line) > 32768:
                return None
            try:
                row = json.loads(line)
            except (json.JSONDecodeError, ValueError):
                continue
            if not isinstance(row, dict) or row.get("schema") != "cortex.agent.operation.v1":
                continue
            if first is None:
                first = row
            elif row.get("operationId") != first.get("operationId"):
                return None
            last = row
            count += 1
    if not first or not last:
        return None
    operation_id = str(first.get("operationId") or "")
    if not OPERATION_ID.fullmatch(operation_id):
        return None
    if not path.name.endswith(f"-{operation_id}.jsonl"):
        return None
    return {
        "operationId": operation_id,
        "startedUtc": first.get("utc"),
        "workspace": first.get("workspace"),
        "worker": first.get("worker"),
        "command": first.get("command"),
        "lastEvent": last.get("kind"),
        "lastEventUtc": last.get("utc"),
        "exitCode": last.get("exitCode") if last.get("kind") == "finished" else None,
        "elapsedSeconds": last.get("elapsedSeconds"),
        "eventCount": count,
        "file": str(path),
    }


def recent_operations(cortex_root: Path, *, limit: int = 10) -> list[dict[str, Any]]:
    if limit <= 0:
        return []
    directory = operation_directory(cortex_root)
    if not directory.is_dir():
        return []
    paths = sorted(directory.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True)
    results: list[dict[str, Any]] = []
    for path in paths[:250]:
        record = _trace_record(path)
        if record:
            results.append(record)
        if len(results) >= max(0, min(limit, 50)):
            break
    return results


def find_operation(cortex_root: Path, operation_id: str) -> dict[str, Any] | None:
    """Look up an exact operation ID (never accept a path from the terminal)."""
    if not OPERATION_ID.fullmatch(operation_id):
        return None
    directory = operation_directory(cortex_root)
    if not directory.is_dir():
        return None
    for path in directory.glob(f"*-{operation_id}.jsonl"):
        record = _trace_record(path)
        if record and record["operationId"] == operation_id:
            return record
    return None


def summarize_operation(record: dict[str, Any]) -> str:
    """Report only recorded operational state; an unfinished trace is never GREEN."""
    state = str(record.get("lastEvent") or "unknown")
    code = record.get("exitCode")
    if state == "finished":
        state = f"finished exit={code}"
    else:
        state += " (completion not recorded)"
    return (f"{record.get('operationId')} | {state} | "
            f"{record.get('command') or '?'} | {record.get('startedUtc') or '?'}")
