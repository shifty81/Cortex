#!/usr/bin/env python3
"""Portable PCC/Cortex conversation storage shared by GUI and Python bridge."""
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


def conversation_dir(cortex_root: Path, workspace: Path) -> Path:
    return _norm(cortex_root) / "data" / "conversations" / "pcc" / workspace_key(workspace)


def conversation_path(cortex_root: Path, workspace: Path, conversation_id: str) -> Path:
    return conversation_dir(cortex_root, workspace) / f"{conversation_key(conversation_id)}.json"


def load_conversation(cortex_root: Path, workspace: Path, conversation_id: str) -> dict[str, Any]:
    path = conversation_path(cortex_root, workspace, conversation_id)
    if not path.is_file():
        return {
            "schema": "cortex.python_bridge.conversation.v2",
            "workspace": str(_norm(workspace)),
            "conversationId": conversation_id,
            "updatedUnixMs": 0,
            "messages": [],
        }
    try:
        value = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception:
        value = {}
    rows = value.get("messages", []) if isinstance(value, dict) else []
    messages: list[dict[str, str]] = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        role = str(row.get("role") or "").strip()
        content = str(row.get("content") or "").strip()
        if role in {"user", "assistant"} and content:
            messages.append({"role": role, "content": content})
    return {
        "schema": str(value.get("schema") or "cortex.python_bridge.conversation.v2") if isinstance(value, dict) else "cortex.python_bridge.conversation.v2",
        "workspace": str(value.get("workspace") or _norm(workspace)) if isinstance(value, dict) else str(_norm(workspace)),
        "conversationId": str(value.get("conversationId") or conversation_id) if isinstance(value, dict) else conversation_id,
        "updatedUnixMs": int(value.get("updatedUnixMs") or 0) if isinstance(value, dict) else 0,
        "messages": messages[-CHAT_HISTORY_LIMIT:],
    }


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
    return path


def list_conversations(cortex_root: Path, workspace: Path) -> list[dict[str, Any]]:
    directory = conversation_dir(cortex_root, workspace)
    if not directory.is_dir():
        return []
    result: list[dict[str, Any]] = []
    for path in directory.glob("*.json"):
        try:
            value = json.loads(path.read_text(encoding="utf-8-sig"))
        except Exception:
            continue
        if not isinstance(value, dict):
            continue
        conversation_id = str(value.get("conversationId") or path.stem)
        messages = value.get("messages") or []
        clean: list[dict[str, str]] = []
        for row in messages:
            if not isinstance(row, dict):
                continue
            role = str(row.get("role") or "").strip()
            content = str(row.get("content") or "").strip()
            if role in {"user", "assistant"} and content:
                clean.append({"role": role, "content": content})
        first_user = next((row["content"] for row in clean if row["role"] == "user"), "")
        title = "New conversation"
        if first_user:
            title = " ".join(first_user.split())
            if len(title) > 58:
                title = title[:55] + "..."
        result.append({
            "conversationId": conversation_id,
            "title": title,
            "updatedUnixMs": int(value.get("updatedUnixMs") or 0),
            "messageCount": len(clean),
            "path": str(path),
        })
    result.sort(key=lambda item: int(item.get("updatedUnixMs") or 0), reverse=True)
    return result
