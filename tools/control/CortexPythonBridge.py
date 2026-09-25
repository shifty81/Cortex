#!/usr/bin/env python3
"""Python-first Cortex bridge used by the PCC GUI.

The bridge keeps ordinary chat/inspection independent from the Rust build.  It
prefers an already-built transactional Cortex worker, but never invokes Cargo to
answer a chat message.  Without a worker, chat/inspect/plan use the configured
local model provider; apply/repair fail closed until the transactional worker is
available.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import time
import urllib.error
import urllib.request
import urllib.parse
from pathlib import Path
from typing import Any

from PCCChatStore import load_messages, save_messages

BRIDGE_VERSION = "CORTEX-PY-BRIDGE-0.7"
CHAT_HISTORY_LIMIT = 20
CONTEXT_TEXT_LIMIT = 24000


def _norm(path: Path) -> Path:
    return path.expanduser().resolve()


def _candidate_runtime_paths(cortex_root: Path) -> list[Path]:
    """Find an existing Cortex worker without running Cargo or triggering a build."""
    candidates: list[Path] = []
    explicit = str(os.environ.get("CORTEX_CLI_EXE") or "").strip()
    if explicit:
        candidates.append(Path(explicit))

    target_env = str(os.environ.get("CARGO_TARGET_DIR") or "").strip()
    if target_env:
        target = Path(target_env)
        candidates.extend([target / "debug" / "cortex.exe", target / "release" / "cortex.exe"])

    candidates.extend([
        cortex_root / "target" / "debug" / "cortex.exe",
        cortex_root / "target" / "release" / "cortex.exe",
        cortex_root / "runtime" / "cortex.exe",
        cortex_root / "bin" / "cortex.exe",
        cortex_root / "artifacts" / "bin" / "cortex.exe",
    ])

    found_on_path = shutil.which("cortex.exe") or shutil.which("cortex")
    if found_on_path:
        candidates.append(Path(found_on_path))

    unique: list[Path] = []
    seen: set[str] = set()
    for candidate in candidates:
        try:
            resolved = _norm(candidate)
        except Exception:
            continue
        key = str(resolved).casefold() if os.name == "nt" else str(resolved)
        if key not in seen:
            seen.add(key)
            unique.append(resolved)
    return unique


def _resolve_runtime(cortex_root: Path) -> Path | None:
    for candidate in _candidate_runtime_paths(cortex_root):
        if candidate.is_file():
            return candidate
    return None


def _provider_url() -> str:
    kind = _provider_kind()
    if kind == "lmstudio":
        return str(
            os.environ.get("CORTEX_LMSTUDIO_URL")
            or os.environ.get("OPEN2D_LMSTUDIO_URL")
            or "http://127.0.0.1:1234/v1"
        ).rstrip("/")
    return str(os.environ.get("CORTEX_NATIVE_MODEL_URL") or "http://127.0.0.1:12400/v1").rstrip("/")




def _provider_kind() -> str:
    return str(os.environ.get("CORTEX_PROVIDER") or "native").strip().casefold()


def _provider_health_url(base_url: str) -> str:
    parsed = urllib.parse.urlsplit(base_url)
    path = parsed.path.rstrip("/")
    if path.endswith("/v1"):
        path = path[:-3]
    path = path.rstrip("/") + "/health"
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, path, "", ""))


def _provider_is_ready(base_url: str, timeout: float = 1.5) -> bool:
    try:
        request = urllib.request.Request(_provider_health_url(base_url), headers={"Accept": "application/json"}, method="GET")
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return 200 <= int(getattr(response, "status", 200)) < 300
    except Exception:
        return False


def _candidate_model_host_paths(cortex_root: Path) -> list[Path]:
    candidates: list[Path] = []
    explicit = str(os.environ.get("CORTEX_MODEL_HOST_EXE") or "").strip()
    if explicit:
        candidates.append(Path(explicit))
    target_env = str(os.environ.get("CARGO_TARGET_DIR") or "").strip()
    if target_env:
        target = Path(target_env)
        candidates.extend([target / "debug" / "cortex_model_host.exe", target / "release" / "cortex_model_host.exe"])
    runtime = _resolve_runtime(cortex_root)
    if runtime is not None:
        candidates.append(runtime.parent / "cortex_model_host.exe")
    candidates.extend([
        cortex_root / "target" / "debug" / "cortex_model_host.exe",
        cortex_root / "target" / "release" / "cortex_model_host.exe",
        cortex_root / "runtime" / "cortex_model_host.exe",
        cortex_root / "bin" / "cortex_model_host.exe",
        cortex_root / "artifacts" / "bin" / "cortex_model_host.exe",
    ])
    found = shutil.which("cortex_model_host.exe") or shutil.which("cortex_model_host")
    if found:
        candidates.append(Path(found))
    unique: list[Path] = []
    seen: set[str] = set()
    for candidate in candidates:
        try:
            resolved = _norm(candidate)
        except Exception:
            continue
        key = str(resolved).casefold() if os.name == "nt" else str(resolved)
        if key not in seen:
            seen.add(key)
            unique.append(resolved)
    return unique


def _resolve_model_host(cortex_root: Path) -> Path | None:
    for candidate in _candidate_model_host_paths(cortex_root):
        if candidate.is_file():
            return candidate
    return None


def _portable_library_root(cortex_root: Path) -> Path:
    for key in ("CORTEX_VAULT_ROOT", "CORTEX_LIBRARY_ROOT", "PCC_VAULT_ROOT", "CORTEX_PORTABLE_VOLUME_ROOT"):
        value = str(os.environ.get(key) or "").strip()
        if value:
            path = Path(value)
            if path.exists():
                return _norm(path)
    if os.name == "nt" and cortex_root.drive:
        drive_root = Path(cortex_root.drive + "\\")
        if drive_root.exists():
            return _norm(drive_root)
    return cortex_root.parent


def _native_provider_port(base_url: str) -> int:
    parsed = urllib.parse.urlsplit(base_url)
    return int(parsed.port or 12400)


def _tail_text(path: Path, limit: int = 3000) -> str:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return ""
    return text[-limit:]


def _ensure_native_provider(cortex_root: Path, base_url: str, owner_pid: int) -> bool:
    if _provider_is_ready(base_url):
        return True
    host = _resolve_model_host(cortex_root)
    if host is None:
        print("[BLOCKED] Cortex Native Models is offline and cortex_model_host.exe was not found.", flush=True)
        print("[NEXT] Run FULL or build the cortex_model_host_app target, then retry chat.", flush=True)
        return False
    library_root = _portable_library_root(cortex_root)
    if owner_pid <= 0:
        owner_pid = os.getppid()
    port = _native_provider_port(base_url)
    log_dir = library_root / ".cortex" / "model-host"
    log_dir.mkdir(parents=True, exist_ok=True)
    launch_log = log_dir / "pcc-launcher.log"
    argv = [
        str(host), "run",
        "--library-root", str(library_root),
        "--parent-pid", str(owner_pid),
        "--port", str(port),
        "--models-max", str(max(1, min(8, int(os.environ.get("CORTEX_NATIVE_MODELS_MAX") or "2")))),
        "--auto-bootstrap",
    ]
    print(f"[INFO] Cortex Native Models is offline; starting portable model host on 127.0.0.1:{port}...", flush=True)
    try:
        log_handle = open(launch_log, "a", encoding="utf-8")
        creationflags = 0
        if os.name == "nt":
            creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0) | getattr(subprocess, "DETACHED_PROCESS", 0)
        process = subprocess.Popen(
            argv,
            cwd=str(cortex_root),
            stdin=subprocess.DEVNULL,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            text=True,
            creationflags=creationflags,
            close_fds=True,
        )
        log_handle.close()
    except Exception as exc:
        print(f"[FAIL] Could not start Cortex Native Models from {host}: {exc}", flush=True)
        return False
    deadline = time.monotonic() + 45.0
    while time.monotonic() < deadline:
        if _provider_is_ready(base_url):
            print(f"[PASS] Cortex Native Models ready at {base_url} (model-host PID {process.pid}).", flush=True)
            return True
        rc = process.poll()
        if rc is not None:
            detail = _tail_text(launch_log)
            print(f"[FAIL] Cortex Native Models exited during startup with code {rc}.", flush=True)
            if detail:
                print(detail, flush=True)
            return False
        time.sleep(0.25)
    print(f"[FAIL] Cortex Native Models did not become ready within 45 seconds. Log: {launch_log}", flush=True)
    detail = _tail_text(launch_log)
    if detail:
        print(detail, flush=True)
    return False


def _ensure_provider_ready(cortex_root: Path, base_url: str, owner_pid: int) -> bool:
    if _provider_is_ready(base_url):
        return True
    if _provider_kind() == "native":
        return _ensure_native_provider(cortex_root, base_url, owner_pid)
    print(f"[BLOCKED] LM Studio provider is offline at {base_url}.", flush=True)
    print("[NEXT] Start LM Studio server or switch CORTEX_PROVIDER=native.", flush=True)
    return False


def _http_json(method: str, url: str, payload: dict[str, Any] | None = None, timeout: float = 120.0) -> Any:
    data = None
    headers = {"Accept": "application/json"}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        raw = response.read()
    return json.loads(raw.decode("utf-8", errors="replace"))


def _select_chat_model(base_url: str) -> str:
    for key in (
        "CORTEX_MODEL_CHAT", "CORTEX_MODEL", "CORTEX_LMSTUDIO_MODEL_CHAT",
        "CORTEX_LMSTUDIO_MODEL", "OPEN2D_LMSTUDIO_MODEL_CHAT", "OPEN2D_LMSTUDIO_MODEL",
    ):
        value = str(os.environ.get(key) or "").strip()
        if value:
            return value

    value = _http_json("GET", f"{base_url}/models", timeout=10.0)
    models = [
        str(item.get("id") or "")
        for item in (value.get("data") or [])
        if isinstance(item, dict) and str(item.get("id") or "").strip()
    ]
    text_models = [m for m in models if "embed" not in m.casefold() and "rerank" not in m.casefold()]
    if not text_models:
        raise RuntimeError("No text model is currently visible from the configured Cortex provider.")
    return text_models[0]


def _parse_response_text(raw: Any) -> str:
    if isinstance(raw, dict):
        direct = raw.get("output_text")
        if isinstance(direct, str) and direct.strip():
            return direct.strip()
        parts: list[str] = []
        for item in raw.get("output") or []:
            if not isinstance(item, dict) or item.get("type") != "message":
                continue
            for part in item.get("content") or []:
                if not isinstance(part, dict):
                    continue
                if part.get("type") in {"output_text", "text"}:
                    text = part.get("text")
                    if isinstance(text, str) and text:
                        parts.append(text)
        if parts:
            return "\n".join(parts).strip()
    raise RuntimeError("The local Cortex provider returned no assistant text.")


def _run_bounded(argv: list[str], cwd: Path, timeout: float = 10.0) -> tuple[int, str]:
    try:
        cp = subprocess.run(
            argv,
            cwd=str(cwd),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
            creationflags=(getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0),
        )
        return cp.returncode, cp.stdout.strip()
    except Exception as exc:
        return 124, f"ERROR: {exc}"


def _workspace_context(workspace: Path, *, include_logs: bool = True) -> str:
    rows: list[str] = [f"Workspace: {workspace}"]
    try:
        from PCCBuildDoctor import latest_failure
        doctor = latest_failure(workspace)
        rows.append("BuildDoctor: " + json.dumps(doctor, ensure_ascii=False)[:12000])
    except Exception as exc:
        rows.append(f"BuildDoctor unavailable: {exc}")

    contract = workspace / "project.control.json"
    if contract.is_file():
        try:
            data = json.loads(contract.read_text(encoding="utf-8-sig"))
            project = data.get("project") or {}
            rows.append(
                "Project contract: "
                f"id={project.get('id') or project.get('name') or workspace.name}; "
                f"kind={project.get('kind') or 'unknown'}; "
                f"commands={len(data.get('commands') or [])}; gates={len(data.get('quality_gates') or [])}"
            )
        except Exception as exc:
            rows.append(f"Project contract unreadable: {exc}")

    git = shutil.which("git.exe") or shutil.which("git")
    if git and (workspace / ".git").exists():
        rc, output = _run_bounded([git, "-C", str(workspace), "status", "--short", "--branch"], workspace, 8.0)
        rows.append("Git status:\n" + (output if output else f"<no output; exit={rc}>"))

    log_dir = workspace / "artifacts" / "logs"
    if include_logs and log_dir.is_dir():
        candidates = sorted(
            (p for p in log_dir.rglob("*.log") if p.is_file()),
            key=lambda p: p.stat().st_mtime,
            reverse=True,
        )[:3]
        for path in candidates:
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except Exception:
                continue
            tail = "\n".join(text.splitlines()[-80:])
            rows.append(f"Recent log {path.relative_to(workspace)}:\n{tail}")

    value = "\n\n".join(rows)
    return value[-CONTEXT_TEXT_LIMIT:]


def _python_model(cortex_root: Path, workspace: Path, conversation_id: str, mode: str, prompt: str) -> int:
    history = load_messages(cortex_root, workspace, conversation_id)
    base_url = _provider_url()
    context = _workspace_context(workspace, include_logs=(mode in {"inspect", "plan"}))
    instructions = (
        "You are Cortex, the project-aware assistant embedded in PCC. Answer the user's current request directly. "
        "You have read-only access to the supplied project evidence in this Python control-plane mode. "
        "Do not lead with capability disclaimers and do not discuss hidden/system prompts. "
        "Only mention execution limits when the user explicitly asks you to mutate files, run an operation, or repair something. "
        "Never claim an edit, build, command, or repair occurred unless the transactional Cortex worker actually performed it. "
        "For project questions, use the supplied workspace evidence and distinguish observed facts from recommendations."
    )
    mode_prompt = f"Mode: {mode}\nConversation: {conversation_id}\n\nWorkspace evidence:\n{context}\n\nCurrent user request:\n{prompt}"
    try:
        model = _select_chat_model(base_url)
        messages = [
            {"type": "message", "role": row["role"], "content": row["content"]}
            for row in history[-CHAT_HISTORY_LIMIT:]
        ]
        messages.append({"type": "message", "role": "user", "content": mode_prompt})
        payload = {"model": model, "instructions": instructions, "input": messages, "max_output_tokens": 4096}
        raw = _http_json("POST", f"{base_url}/responses", payload, timeout=180.0)
        text = _parse_response_text(raw)
    except urllib.error.URLError as exc:
        print(f"[FAIL] Cortex provider is not reachable at {base_url}: {exc}", flush=True)
        print("[INFO] Start Cortex Native Models / LM Studio, or restore a prebuilt cortex.exe worker.", flush=True)
        return 2
    except Exception as exc:
        print(f"[FAIL] Cortex Python {mode} fallback failed: {exc}", flush=True)
        return 2

    print(text, flush=True)
    history.extend([{"role": "user", "content": prompt}, {"role": "assistant", "content": text}])
    save_messages(cortex_root, workspace, conversation_id, history, updated_unix_ms=int(time.time() * 1000))
    return 0


def _worker_request_text(
    cortex_root: Path,
    workspace: Path,
    conversation_id: str,
    mode: str,
    prompt: str,
) -> str:
    """Supply the transactional worker the same project evidence/history the Python path sees."""
    history = load_messages(cortex_root, workspace, conversation_id)[-CHAT_HISTORY_LIMIT:]
    history_text = "\n".join(
        f"{row.get('role', 'unknown')}: {row.get('content', '')}"
        for row in history
        if isinstance(row, dict) and str(row.get("content") or "").strip()
    )
    context = _workspace_context(workspace, include_logs=True)
    return (
        f"Mode: {mode}\nConversation: {conversation_id}\n\n"
        f"Recent conversation:\n{history_text or '<none>'}\n\n"
        f"Current workspace evidence supplied by PCC:\n{context}\n\n"
        f"Current user request:\n{prompt}"
    )


def _run_worker(runtime: Path, cortex_root: Path, workspace: Path, conversation_id: str, mode: str, prompt: str) -> int:
    worker_prompt = _worker_request_text(cortex_root, workspace, conversation_id, mode, prompt)

    if mode in {"chat", "inspect", "plan"}:
        argv = [str(runtime), "--workspace", str(workspace), mode, worker_prompt]
        if mode == "chat":
            argv.append("--json")
        result = subprocess.run(
            argv,
            cwd=str(workspace),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if result.stderr.strip():
            print(result.stderr.rstrip(), flush=True)

        text = ""
        if result.returncode == 0:
            if mode == "chat":
                try:
                    payload = json.loads(result.stdout)
                    text = str(payload.get("text") or "").strip()
                except Exception:
                    text = result.stdout.strip()
            else:
                text = result.stdout.strip()

        if result.returncode == 0 and text:
            print(text, flush=True)
            history = load_messages(cortex_root, workspace, conversation_id)
            history.extend([{"role": "user", "content": prompt}, {"role": "assistant", "content": text}])
            save_messages(
                cortex_root, workspace, conversation_id, history,
                updated_unix_ms=int(time.time() * 1000),
            )
            return 0

        detail = result.stdout.strip() or result.stderr.strip() or f"exit {result.returncode} with no text"
        print(f"[WARN] Transactional Cortex {mode} worker did not produce a usable response: {detail[-1200:]}", flush=True)
        print("[INFO] Falling back to the non-mutating Python provider path for this request.", flush=True)
        return _python_model(cortex_root, workspace, conversation_id, mode, prompt)

    # Mutating modes remain fail-closed on the transactional worker.  They receive
    # the same build/log evidence automatically, but never fall back to chat-only mode.
    argv = [str(runtime), "--workspace", str(workspace), mode, worker_prompt]
    process = subprocess.Popen(
        argv,
        cwd=str(workspace),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
    )
    assert process.stdout is not None
    for line in process.stdout:
        print(line, end="", flush=True)
    return int(process.wait())

def main() -> int:
    parser = argparse.ArgumentParser(description="Python bootstrap/control bridge for Cortex PCC.")
    parser.add_argument("--cortex-root", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--conversation-id", default="session")
    parser.add_argument("--owner-pid", type=int, default=0, help="Stable PCC/GUI owner PID for portable native-model lifetime")
    parser.add_argument("mode", choices=["chat", "inspect", "plan", "apply", "repair"])
    parser.add_argument("prompt")
    args = parser.parse_args()

    cortex_root = _norm(Path(args.cortex_root))
    workspace = _norm(Path(args.workspace))
    if not workspace.is_dir():
        print(f"[FAIL] Cortex workspace does not exist: {workspace}", flush=True)
        return 2

    base_url = _provider_url()
    if not _ensure_provider_ready(cortex_root, base_url, int(args.owner_pid or os.getppid())):
        return 2

    runtime = _resolve_runtime(cortex_root)
    if runtime is not None:
        print(f"[Cortex] {BRIDGE_VERSION} | worker={runtime}", flush=True)
        return _run_worker(runtime, cortex_root, workspace, args.conversation_id, args.mode, args.prompt)

    if args.mode in {"chat", "inspect", "plan"}:
        print(f"[Cortex] {BRIDGE_VERSION} | worker=unavailable | using non-mutating Python provider fallback", flush=True)
        return _python_model(cortex_root, workspace, args.conversation_id, args.mode, args.prompt)

    print(f"[BLOCKED] Cortex {args.mode} requires the transactional Cortex worker.", flush=True)
    print("[INFO] No prebuilt cortex.exe was found. Chat, inspect, and plan remain available through Python.", flush=True)
    print("[NEXT] Run PCC command: cortex-worker-build", flush=True)
    print("[INFO] Worker provisioning is explicit; chat will never compile Cortex merely to answer a prompt.", flush=True)
    return 3


if __name__ == "__main__":
    raise SystemExit(main())
