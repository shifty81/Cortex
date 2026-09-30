#!/usr/bin/env python3
"""Interactive terminal interface to the existing Cortex PCC/common controller.

Run from any registered project: python <Cortex>/tools/control/CortexAgentCLI.py
or pass --workspace. This frontend deliberately delegates *every* natural-language
request through CortexPythonBridge; it has no independent AI/coding authority.
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path
from typing import Callable

import CortexPythonBridge as bridge
import CortexAgentEvidence as evidence


def dispatch(bridge_script: Path, *, cortex_root: Path, workspace: Path,
             conversation_id: str, prompt: str,
             reporter: Callable[[str], None] = print) -> tuple[int, Path]:
    """Stream bridge operational output and return the reported active workspace."""
    argv = [sys.executable, "-u", str(bridge_script),
            "--cortex-root", str(cortex_root), "--workspace", str(workspace),
            "--conversation-id", conversation_id, "--owner-pid", str(os.getpid()),
            "chat", prompt]
    proc = subprocess.Popen(argv, cwd=str(cortex_root), stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, encoding="utf-8",
                            errors="replace", bufsize=1)
    target = workspace
    assert proc.stdout is not None
    for line in proc.stdout:
        content = line.rstrip("\r\n")
        if content.startswith("[CortexWorkspace] "):
            proposed = Path(content[len("[CortexWorkspace] "):].strip()).expanduser()
            if proposed.is_dir():
                target = proposed.resolve()
            else:
                reporter("[WARN] Cortex returned an unavailable workspace; retaining the previous project.")
        elif not content.startswith("[CortexConversation] "):
            reporter(content)
    proc.stdout.close()
    rc = int(proc.wait())
    if rc:
        reporter(f"[Cortex Agent] Request FAILED (exit {rc}); no execution success is certified.")
        return rc, workspace
    if target != workspace:
        reporter(f"[Cortex Agent] Active workspace: {target}")
    return 0, target


def main(argv: list[str] | None = None, *, input_fn: Callable[[str], str] = input,
         reporter: Callable[[str], None] = print) -> int:
    parser = argparse.ArgumentParser(description="Interactive Cortex coding agent — PCC common-controller frontend")
    parser.add_argument("--cortex-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--conversation-id", default="cortex-terminal")
    parser.add_argument("--prompt", help="Submit one noninteractive prompt and exit")
    options = parser.parse_args(argv)
    cortex_root = options.cortex_root.expanduser().resolve()
    workspace = options.workspace.expanduser().resolve()
    bridge_script = cortex_root / "tools" / "control" / "CortexPythonBridge.py"
    if not bridge_script.is_file() or not workspace.is_dir():
        reporter(f"[FAIL] Valid Cortex bridge and existing workspace required. bridge={bridge_script} workspace={workspace}")
        return 2
    conversation_id = options.conversation_id

    def status() -> None:
        worker = bridge._resolve_runtime(cortex_root)
        ready = bridge._provider_is_ready(bridge._provider_url())
        reporter(f"Workspace : {workspace}")
        reporter(f"Worker    : {worker or 'NOT FOUND (coding blocked)'}")
        reporter(f"Provider  : {'HTTP healthy' if ready else 'offline / not confirmed'} ({bridge._provider_url()})")
        reporter(f"Trace     : {cortex_root / 'artifacts' / 'logs' / 'cortex-agent'}")
        reporter(f"PCC cache conversation: {conversation_id}")
        reporter("Note: provider HTTP health does not prove model loading or tool execution.")
        reporter("Note: PCC presentation IDs are not yet certified as Rust ConversationStore IDs.")

    def submit(prompt: str) -> int:
        nonlocal workspace
        rc, new_workspace = dispatch(bridge_script, cortex_root=cortex_root, workspace=workspace,
                                     conversation_id=conversation_id, prompt=prompt, reporter=reporter)
        if rc == 0:
            workspace = new_workspace
        return rc

    if options.prompt is not None:
        if not options.prompt.strip():
            reporter("[FAIL] --prompt cannot be blank")
            return 2
        return submit(options.prompt.strip())

    reporter("Cortex Terminal Agent — PCC/common controller (not general text-only 'cortex chat')")
    reporter("Type /help for commands. Approval answers such as Yes or No go directly to Cortex.")
    status()
    while True:
        try:
            prompt = input_fn("cortex> ").strip()
        except (EOFError, KeyboardInterrupt):
            reporter("\n[Cortex Agent] Session ended; durable conversation and operation logs retained.")
            return 0
        if not prompt:
            continue
        command = prompt.casefold()
        if command in {"/exit", "/quit", "/q"}:
            return 0
        if command in {"/help", "/?"}:
            reporter("/status   Show active workspace, worker and endpoint health")
            reporter("/logs     Show recent per-request operation trace filenames")
            reporter("/operations       Show recorded worker-operation metadata")
            reporter("/operation ID     Inspect one exact operation ID (metadata only)")
            reporter("/conversations    List PCC presentation-cache conversation titles and IDs")
            reporter("/history [ID]     Read an existing PCC presentation-cache conversation")
            reporter("/use PATH Switch to another existing workspace (read-only selection)")
            reporter("/exit     Close this terminal frontend")
            reporter("All other text, including Yes/No, goes to the Rust common controller.")
            continue
        if command == "/status":
            status()
            continue
        if command == "/logs":
            directory = cortex_root / "artifacts" / "logs" / "cortex-agent"
            for log in sorted(directory.glob("*.jsonl"), key=lambda x: x.stat().st_mtime, reverse=True)[:5] if directory.is_dir() else []:
                reporter(str(log))
            continue
        if command == "/operations":
            rows = evidence.recent_operations(cortex_root)
            if not rows:
                reporter("[INFO] No valid bounded Cortex worker traces found.")
            for row in rows:
                reporter(evidence.summarize_operation(row))
            continue
        if command.startswith("/operation "):
            operation_id = prompt[len("/operation "):].strip()
            row = evidence.find_operation(cortex_root, operation_id)
            if not row:
                reporter("[BLOCKED] Operation ID not found or invalid; expected exact 32-character hex ID.")
            else:
                for key, value in row.items():
                    reporter(f"{key}: {value}")
            continue
        if command == "/conversations":
            rows = evidence.conversations(cortex_root, workspace)
            if not rows:
                reporter("[INFO] No PCC presentation-cache conversations found for this workspace.")
            for row in rows:
                reporter(f"{row['conversationId']} | {row['title']} | messages={row['messageCount']}")
            reporter("[INFO] Read-only cache listing; backend conversation identity convergence remains pending.")
            continue
        if command == "/history" or command.startswith("/history "):
            requested = prompt[len("/history"):].strip() or conversation_id
            record = evidence.history(cortex_root, workspace, requested)
            if record is None:
                reporter("[BLOCKED] No existing PCC presentation conversation with that exact ID.")
            else:
                reporter(f"PCC presentation history: {record['conversationId']} (read-only)")
                for message in record.get("messages", []):
                    reporter(f"[{str(message['role']).upper()}] {message['content']}")
                reporter("[INFO] This does not claim a Rust operation or approval was resumed.")
            continue
        if command.startswith("/use "):
            proposed = Path(prompt[5:].strip().strip('"')).expanduser().resolve()
            if not proposed.is_dir():
                reporter(f"[BLOCKED] Workspace does not exist: {proposed}")
            else:
                workspace = proposed
                reporter(f"[Cortex Agent] Selected existing workspace: {workspace}")
            continue
        submit(prompt)


if __name__ == "__main__":
    raise SystemExit(main())
