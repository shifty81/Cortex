#!/usr/bin/env python3
"""Observable one-shot Cortex worker host shared by PCC and the terminal agent.

The Rust common controller remains the only development authority.  This Python
module does not interpret prompts, synthesize successful results, or modify a
project.  It emits *operational* events, not private model reasoning.  Assistant
responses are returned in-memory and kept out of telemetry records.
"""
from __future__ import annotations

import hashlib
import json
import os
import queue
import subprocess
import threading
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable


MAX_RESULT_BYTES = 8 * 1024 * 1024


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _safe_duration(raw: str, *, default: float, minimum: float, maximum: float) -> float:
    try:
        return max(minimum, min(maximum, float(raw)))
    except (ValueError, OverflowError):
        return default


_PORTABLE_AUTHORITY_KEYS = (
    "CORTEX_VAULT_ROOT",
    "PCC_VAULT_ROOT",
    "CORTEX_LIBRARY_ROOT",
    "CORTEX_PORTABLE_VOLUME_ROOT",
    "CORTEX_HOME",
    "CORTEX_RUNTIME_ROOT",
    "CORTEX_PROJECTS_ROOT",
    "CORTEX_MODELS_ROOT",
    "CORTEX_LOCAL_GIT_ROOT",
    "CORTEX_STATE_MODE",
)


def _portable_authority_environment(cortex_root: Path) -> dict[str, str]:
    # Return only portable authority variables. Do not inject CARGO_TARGET_DIR:
    # the worker may switch to another managed project during this request.
    from PCCVaultStorage import dependency_environment

    resolved = dependency_environment(cortex_root)
    merged = os.environ.copy()
    missing: list[str] = []
    for key in _PORTABLE_AUTHORITY_KEYS:
        value = str(resolved.get(key) or "").strip()
        if not value:
            missing.append(key)
            continue
        merged[key] = value
    if missing:
        raise RuntimeError(
            "portable Cortex authority environment is incomplete: " + ", ".join(missing)
        )
    return merged


def _parse_execution_state(path: Path) -> dict[str, str] | None:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    state: dict[str, str] = {}
    for raw in text.splitlines():
        if "=" not in raw:
            continue
        key, value = raw.split("=", 1)
        state[key.strip()] = value.strip()
    return state if state.get("owner_pid") else None


def _execution_roots(environment: dict[str, str]) -> list[Path]:
    roots: list[Path] = []
    explicit = str(environment.get("CORTEX_EXECUTION_ROOT") or "").strip()
    if explicit:
        roots.append(Path(explicit).expanduser())
    local = str(environment.get("LOCALAPPDATA") or "").strip()
    if local:
        base = Path(local).expanduser()
    else:
        base = Path(os.environ.get("LOCALAPPDATA") or Path.home())
    # Current Rust authority first, then the normalized future location. Keeping
    # both here makes the PCC projection migration-safe without changing the
    # Rust state format or inventing a second execution authority.
    roots.extend([base / "Open2D" / "Cortex" / "executions", base / "Cortex" / "executions"])
    unique: list[Path] = []
    seen: set[str] = set()
    for root in roots:
        key = str(root).casefold() if os.name == "nt" else str(root)
        if key not in seen:
            seen.add(key)
            unique.append(root)
    return unique


def _execution_live_status_for_owner(owner_pid: int, environment: dict[str, str]) -> tuple[str, str] | None:
    selected: dict[str, str] | None = None
    for base in _execution_roots(environment):
        if not base.is_dir():
            continue
        for path in base.glob("*/active.state"):
            state = _parse_execution_state(path)
            if not state or state.get("owner_pid") != str(owner_pid):
                continue
            try:
                updated = int(state.get("updated_unix_ms") or 0)
                selected_updated = int((selected or {}).get("updated_unix_ms") or -1)
            except ValueError:
                updated, selected_updated = 0, -1
            if selected is None or updated >= selected_updated:
                selected = state
    if selected is None:
        return None

    try:
        started = int(selected.get("started_unix_ms") or 0)
    except ValueError:
        started = 0
    elapsed = max(0.0, (time.time() * 1000.0 - started) / 1000.0) if started else 0.0
    mode = selected.get("mode") or "cortex"
    phase = selected.get("phase") or "working"
    parts = [f"{mode} · {phase} · {elapsed:.1f}s"]
    current = selected.get("current") or ""
    if current:
        parts.append(current)
    tool = selected.get("current_tool") or ""
    target = selected.get("current_target") or ""
    if tool:
        parts.append(f"tool: {tool}" + (f" → {target}" if target else ""))
    model = selected.get("model") or ""
    role = selected.get("model_role") or ""
    if model:
        parts.append("model: " + model + (f" ({role})" if role else ""))
    iteration = selected.get("agent_iteration") or "0"
    budget = selected.get("agent_iteration_budget") or "0"
    if iteration != "0" or budget != "0":
        parts.append(f"iteration: {iteration}/{budget}")
    missing = selected.get("missing_dependencies") or ""
    if missing:
        parts.append(f"grounding: {missing}")
    next_expected = selected.get("next_expected") or ""
    if next_expected and phase not in {"completed", "failed", "cancelled"}:
        parts.append(f"next: {next_expected}")
    error = selected.get("terminal_error") or ""
    if error:
        parts.append("error: " + (error[:217] + "..." if len(error) > 220 else error))
    token = "|".join((
        selected.get("execution_id") or "",
        selected.get("updated_unix_ms") or "",
        phase, current, tool, target, model, role, iteration, budget, missing, next_expected, error,
    ))
    return "  |  ".join(parts), token


def _execution_live_line_for_owner(owner_pid: int, environment: dict[str, str]) -> str | None:
    status = _execution_live_status_for_owner(owner_pid, environment)
    return status[0] if status else None


@dataclass(frozen=True)
class WorkerResult:
    returncode: int
    stdout: str
    stderr: str
    operation_id: str
    log_path: Path
    elapsed_seconds: float
    timed_out: bool = False
    output_limit_exceeded: bool = False


def run_controller(
    argv: list[str], *, cortex_root: Path, workspace: Path, prompt: str,
    heartbeat_seconds: float | None = None, timeout_seconds: float | None = None,
    reporter: Callable[[str], None] = print,
) -> WorkerResult:
    """Run the existing Rust controller while emitting a bounded, durable trace.

    STDOUT is reserved for the controller's authoritative JSON payload. Stderr
    is forwarded as it arrives. A heartbeat indicates only that a process is
    still alive, never that a tool is progressing or that inference succeeded.
    """
    heartbeat = heartbeat_seconds if heartbeat_seconds is not None else _safe_duration(
        os.environ.get("CORTEX_AGENT_HEARTBEAT_SECONDS", "1"), default=1.0, minimum=0.1, maximum=60.0,
    )
    deadline_seconds = timeout_seconds if timeout_seconds is not None else _safe_duration(
        os.environ.get("CORTEX_AGENT_TIMEOUT_SECONDS", "1800"), default=1800.0, minimum=10.0, maximum=7200.0,
    )
    operation_id = uuid.uuid4().hex
    log_dir = cortex_root / "artifacts" / "logs" / "cortex-agent"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / f"{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')}-{operation_id}.jsonl"
    started = time.monotonic()
    prompt_hash = hashlib.sha256(prompt.encode("utf-8")).hexdigest()

    with log_path.open("x", encoding="utf-8") as log:
        def event(kind: str, **fields: object) -> None:
            record = {"schema": "cortex.agent.operation.v1", "utc": _utc_now(),
                      "operationId": operation_id, "kind": kind, **fields}
            log.write(json.dumps(record, ensure_ascii=False) + "\n")
            log.flush()

        event("started", workspace=str(workspace), worker=str(argv[0]),
              command=next((arg for arg in argv if arg in {"pcc-chat", "inspect", "plan", "apply", "repair"}), "unknown"),
              promptSha256=prompt_hash,
              promptBytes=len(prompt.encode("utf-8")), timeoutSeconds=deadline_seconds)
        reporter(f"[CortexRuntime] operation={operation_id} START log={log_path}")
        try:
            worker_env = _portable_authority_environment(cortex_root)
        except Exception as exc:
            elapsed = time.monotonic() - started
            event(
                "environment_failed",
                errorType=type(exc).__name__,
                exitCode=126,
                elapsedSeconds=round(elapsed, 3),
            )
            reporter(
                f"[CortexRuntime] FAIL portable worker environment could not be resolved: "
                f"{type(exc).__name__}: {exc}"
            )
            return WorkerResult(126, "", str(exc), operation_id, log_path, elapsed)

        try:
            proc = subprocess.Popen(
                argv, cwd=str(workspace), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, encoding="utf-8", errors="replace", bufsize=1,
                env=worker_env,
                creationflags=(getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0),
            )
        except OSError as exc:
            elapsed = time.monotonic() - started
            event("spawn_failed", errorType=type(exc).__name__, exitCode=127,
                  elapsedSeconds=round(elapsed, 3))
            reporter(f"[CortexRuntime] FAIL worker could not start: {type(exc).__name__}: {exc}")
            return WorkerResult(127, "", str(exc), operation_id, log_path, elapsed)
        event("worker_spawned", pid=proc.pid)
        reporter(f"[CortexRuntime] worker PID={proc.pid}; waiting for execution/provider activity")

        output_queue: queue.Queue[tuple[str, str | None]] = queue.Queue()
        chunks: dict[str, list[str]] = {"stdout": [], "stderr": []}
        totals = {"stdout": 0, "stderr": 0}

        def pump(name: str, stream: object) -> None:
            try:
                for line in stream:  # type: ignore[attr-defined]
                    output_queue.put((name, line))
            finally:
                output_queue.put((name, None))
                stream.close()  # type: ignore[attr-defined]

        assert proc.stdout is not None and proc.stderr is not None
        for channel, stream in (("stdout", proc.stdout), ("stderr", proc.stderr)):
            threading.Thread(target=pump, args=(channel, stream), daemon=True).start()

        open_channels = {"stdout", "stderr"}
        timed_out = False
        exceeded = False
        last_live_token = ""
        last_fallback_notice = 0.0
        while open_channels:
            elapsed = time.monotonic() - started
            if elapsed >= deadline_seconds:
                timed_out = True
                event("timeout", elapsedSeconds=round(elapsed, 3))
                reporter(f"[CortexRuntime] FAIL controller timed out after {elapsed:.1f}s")
                proc.kill()
                break
            try:
                channel, line = output_queue.get(timeout=min(heartbeat, max(0.01, deadline_seconds - elapsed)))
            except queue.Empty:
                elapsed = time.monotonic() - started
                live_status = _execution_live_status_for_owner(proc.pid, worker_env)
                if live_status and live_status[1] != last_live_token:
                    live_line, last_live_token = live_status
                    encoded = live_line.encode("utf-8", errors="replace")
                    event("execution_status", pid=proc.pid, elapsedSeconds=round(elapsed, 3),
                          bytes=len(encoded), sha256=hashlib.sha256(encoded).hexdigest())
                    reporter(f"[CortexLive] {live_line}")
                elif not live_status and (elapsed - last_fallback_notice >= 15.0 or last_fallback_notice == 0.0 and elapsed >= 5.0):
                    last_fallback_notice = elapsed
                    event("heartbeat", pid=proc.pid, elapsedSeconds=round(elapsed, 3),
                          state="waiting_for_execution_or_provider_activity")
                    reporter(f"[CortexRuntime] worker PID={proc.pid} active; elapsed={elapsed:.1f}s; waiting for execution/provider activity")
                continue
            if line is None:
                open_channels.discard(channel)
                continue
            size = len(line.encode("utf-8", errors="replace"))
            totals[channel] += size
            if totals[channel] > MAX_RESULT_BYTES:
                exceeded = True
                event("output_limit", channel=channel, observedBytes=totals[channel])
                reporter(f"[CortexRuntime] FAIL {channel} exceeded bounded result limit")
                proc.kill()
                break
            chunks[channel].append(line)
            if channel == "stderr":
                # The on-screen console gets the actual diagnostics; the durable
                # telemetry only retains lengths/hashes to avoid writing secrets.
                reporter(line.rstrip("\r\n"))
                event("worker_stderr", bytes=size, sha256=hashlib.sha256(line.encode("utf-8")).hexdigest())

        try:
            actual_rc = proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            proc.kill()
            actual_rc = proc.wait()
        rc = 124 if timed_out else (125 if exceeded else int(actual_rc))
        elapsed = time.monotonic() - started
        stdout = "".join(chunks["stdout"])
        stderr = "".join(chunks["stderr"])
        event("finished", exitCode=rc, nativeExitCode=actual_rc,
              elapsedSeconds=round(elapsed, 3), timedOut=timed_out,
              outputLimitExceeded=exceeded, stdoutBytes=totals["stdout"],
              stderrBytes=totals["stderr"],
              stdoutSha256=hashlib.sha256(stdout.encode("utf-8")).hexdigest())
        reporter(f"[CortexRuntime] operation={operation_id} END exit={rc} elapsed={elapsed:.1f}s log={log_path}")
        return WorkerResult(rc, stdout, stderr, operation_id, log_path, elapsed,
                            timed_out=timed_out, output_limit_exceeded=exceeded)
