#!/usr/bin/env python3
"""Portable PCC/Cortex conversation storage shared by GUI and Python bridge.

The canonical PCC presentation cache lives under ``.cortex/conversations`` so
source-only rollups never capture live conversation/runtime state. Historical
``data/conversations`` state is migrated in-place, preserving the newest valid
copy for each conversation and removing empty legacy directories.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any

CHAT_HISTORY_LIMIT = 20


def _norm(path: Path) -> Path:
    return path.expanduser().resolve()


def workspace_key(workspace: Path) -> str:
    value = str(_norm(workspace))
    if os.name == "nt":
        value = value.casefold()
    return hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()[:20]


def conversation_key(value: str) -> str:
    safe = "".join(ch for ch in value.strip() if ch.isalnum() or ch in "-_")
    return (safe or "session")[:96]


def _conversation_root(cortex_root: Path) -> Path:
    return _norm(cortex_root) / ".cortex" / "conversations" / "pcc"


def _legacy_conversation_root(cortex_root: Path) -> Path:
    return _norm(cortex_root) / "data" / "conversations" / "pcc"


def conversation_dir(cortex_root: Path, workspace: Path) -> Path:
    return _conversation_root(cortex_root) / workspace_key(workspace)


def _legacy_conversation_dir(cortex_root: Path, workspace: Path) -> Path:
    return _legacy_conversation_root(cortex_root) / workspace_key(workspace)


def conversation_path(cortex_root: Path, workspace: Path, conversation_id: str) -> Path:
    return conversation_dir(cortex_root, workspace) / f"{conversation_key(conversation_id)}.json"


def _legacy_conversation_path(cortex_root: Path, workspace: Path, conversation_id: str) -> Path:
    return _legacy_conversation_dir(cortex_root, workspace) / f"{conversation_key(conversation_id)}.json"


def _read_payload(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    try:
        value = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception:
        return None
    return value if isinstance(value, dict) else None


def _updated_ms(path: Path) -> int:
    value = _read_payload(path)
    if value is None:
        return -1
    try:
        return int(value.get("updatedUnixMs") or 0)
    except (TypeError, ValueError):
        return 0


def _remove_empty_legacy_parents(cortex_root: Path, start: Path) -> None:
    stop = _norm(cortex_root) / "data"
    current = start
    while current != stop and stop in current.parents:
        try:
            current.rmdir()
        except OSError:
            break
        current = current.parent
    try:
        stop.rmdir()
    except OSError:
        pass


def _migrate_file(cortex_root: Path, legacy: Path, canonical: Path) -> bool:
    legacy_payload = _read_payload(legacy)
    if legacy_payload is None:
        return False
    canonical_payload = _read_payload(canonical)
    if canonical_payload is not None and _updated_ms(canonical) >= _updated_ms(legacy):
        try:
            legacy.unlink()
            _remove_empty_legacy_parents(cortex_root, legacy.parent)
        except OSError:
            pass
        return True
    canonical.parent.mkdir(parents=True, exist_ok=True)
    tmp = canonical.with_suffix(canonical.suffix + ".migrate.tmp")
    try:
        tmp.write_text(json.dumps(legacy_payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        tmp.replace(canonical)
        legacy.unlink()
        _remove_empty_legacy_parents(cortex_root, legacy.parent)
        return True
    finally:
        try:
            tmp.unlink()
        except OSError:
            pass


def migrate_legacy_conversations(cortex_root: Path) -> dict[str, int]:
    """Move valid legacy PCC presentation-cache files out of the source tree.

    Malformed legacy JSON is intentionally left in place so repository hygiene
    remains RED rather than silently deleting evidence that could not be read.
    """
    root = _legacy_conversation_root(cortex_root)
    result = {"migrated": 0, "invalid": 0}
    if not root.is_dir():
        return result
    for legacy in sorted(root.glob("*/*.json"), key=lambda p: p.as_posix().casefold()):
        if _read_payload(legacy) is None:
            result["invalid"] += 1
            continue
        rel = legacy.relative_to(root)
        canonical = _conversation_root(cortex_root) / rel
        if _migrate_file(cortex_root, legacy, canonical):
            result["migrated"] += 1
    _remove_empty_legacy_parents(cortex_root, root)
    return result


def _clean_conversation(value: dict[str, Any], workspace: Path, conversation_id: str) -> dict[str, Any]:
    rows = value.get("messages", [])
    messages: list[dict[str, str]] = []
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict):
            continue
        role = str(row.get("role") or "").strip()
        content = str(row.get("content") or "").strip()
        if role in {"user", "assistant"} and content:
            messages.append({"role": role, "content": content})
    try:
        updated = int(value.get("updatedUnixMs") or 0)
    except (TypeError, ValueError):
        updated = 0
    return {
        "schema": str(value.get("schema") or "cortex.python_bridge.conversation.v2"),
        "workspace": str(value.get("workspace") or _norm(workspace)),
        "conversationId": str(value.get("conversationId") or conversation_id),
        "updatedUnixMs": updated,
        "messages": messages[-CHAT_HISTORY_LIMIT:],
    }


def load_conversation(cortex_root: Path, workspace: Path, conversation_id: str) -> dict[str, Any]:
    migrate_legacy_conversations(cortex_root)
    path = conversation_path(cortex_root, workspace, conversation_id)
    value = _read_payload(path)
    if value is None:
        return {
            "schema": "cortex.python_bridge.conversation.v2",
            "workspace": str(_norm(workspace)),
            "conversationId": conversation_id,
            "updatedUnixMs": 0,
            "messages": [],
        }
    return _clean_conversation(value, workspace, conversation_id)


def load_messages(cortex_root: Path, workspace: Path, conversation_id: str) -> list[dict[str, str]]:
    return list(load_conversation(cortex_root, workspace, conversation_id).get("messages") or [])


def save_messages(
    cortex_root: Path,
    workspace: Path,
    conversation_id: str,
    rows: list[dict[str, str]],
    *,
    updated_unix_ms: int,
) -> Path:
    migrate_legacy_conversations(cortex_root)
    directory = conversation_dir(cortex_root, workspace)
    directory.mkdir(parents=True, exist_ok=True)
    path = conversation_path(cortex_root, workspace, conversation_id)
    payload = {
        "schema": "cortex.python_bridge.conversation.v2",
        "workspace": str(_norm(workspace)),
        "conversationId": conversation_id,
        "updatedUnixMs": int(updated_unix_ms),
        "messages": rows[-CHAT_HISTORY_LIMIT:],
    }
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    tmp.replace(path)
    legacy = _legacy_conversation_path(cortex_root, workspace, conversation_id)
    if legacy.is_file():
        try:
            legacy.unlink()
            _remove_empty_legacy_parents(cortex_root, legacy.parent)
        except OSError:
            pass
    return path


def list_conversations(cortex_root: Path, workspace: Path) -> list[dict[str, Any]]:
    migrate_legacy_conversations(cortex_root)
    directory = conversation_dir(cortex_root, workspace)
    if not directory.is_dir():
        return []
    result: list[dict[str, Any]] = []
    for path in directory.glob("*.json"):
        value = _read_payload(path)
        if value is None:
            continue
        conversation_id = str(value.get("conversationId") or path.stem)
        clean = _clean_conversation(value, workspace, conversation_id)
        messages = clean["messages"]
        first_user = next((row["content"] for row in messages if row["role"] == "user"), "")
        title = "New conversation"
        if first_user:
            title = " ".join(first_user.split())
            if len(title) > 58:
                title = title[:55] + "..."
        result.append({
            "conversationId": conversation_id,
            "title": title,
            "updatedUnixMs": int(clean.get("updatedUnixMs") or 0),
            "messageCount": len(messages),
            "path": str(path),
        })
    result.sort(key=lambda item: int(item.get("updatedUnixMs") or 0), reverse=True)
    return result
