# Cortex W13B2C1 — Terminal-agent bootstrap and request observability

## Purpose

Make the already-built Rust common development controller usable from a persistent terminal frontend without making another AI authority. Add meaningful live request status to PCC Chat/Project Console, retain the user's draft on a busy submission, and collect bounded per-request evidence in Cortex's normal debug bundle.

## Files / behavior

- `tools/control/CortexAgentCLI.py`: Interactive `cortex> ` terminal frontend and `--prompt` single-shot mode. Both forward all natural-language/approval turns to `CortexPythonBridge.py`, which invokes the Rust `pcc-chat` command. `/status`, `/logs`, `/use PATH`, `/help`, and `/exit` are local UI/navigation commands; no bypass of the common controller for coding. The terminal respects returned workspace switches.
- `tools/control/CortexAgentRuntime.py`: Emits worker PID, heartbeat explicitly labelled **waiting for controller output**, exit status and elapsed time; streams real Rust stderr into the existing Project Console. Reserves Rust stdout for its final JSON response. Writes hashed/bounded metadata (not prompts or assistant response bodies) into `artifacts/logs/cortex-agent/*.jsonl`. Default timeout 30 minutes (configurable via `CORTEX_AGENT_TIMEOUT_SECONDS`); output limited to 8 MiB/channel. An explicit failure never yields a fabricated success.
- `tools/control/CortexPythonBridge.py`: Uses the shared observable runner for chat/inspect/plan, while preserving W13B2B fail-closed semantics for actual coding requests. Locates the project-keyed portable Cargo worker target in a standalone terminal even without `CARGO_TARGET_DIR` inherited from PCC.
- `tools/control/CortexPCCGui.py`: Prevents keyboard/button submission while another job is active **before** deleting the draft or creating a YOU bubble; displays safe live worker state and excludes runtime telemetry lines from the assistant bubble.
- `tools/control/CortexPCC.py`: Includes up to eight recent bounded non-symlink agent JSONL traces in normal debug bundles.
- Regression tests updated for bridge version and runtime-adapter boundary; new runtime, error, timeout, CLI workspace-switch, worker-selection and GUI-busy tests.

## On Windows, after applying and certifying FULL

From PowerShell in the Cortex checkout:

```powershell
Set-Location G:\Cortex
python .\tools\control\CortexAgentCLI.py --workspace .
```

The CLI should print its selected worker path and provider health. Use `/status`, ask a read-only project question, then request `CortexHelloWorld` creation. Reply `Yes` only after reviewing the proposal. Runtime certification requires actual project files and captured executable output. Quit with `/exit`.

Noninteractive example:

```powershell
python .\tools\control\CortexAgentCLI.py --workspace . --prompt "Report the active project name and root. Do not modify files."
```

If your Python command is not on PATH, use the already configured portable Python executable. The CLI does not run Cargo to bootstrap itself.

## Limits and next pass

- This is a Python CLI **frontend to the Rust common controller**, not yet a Rust-native `cortex` REPL and not a separate coding engine.
- The present Rust `pcc-chat` response remains one-shot. The Python runner can surface actual stderr and honest heartbeats, but cannot invent fine-grained tool/model events that the Rust controller does not yet publish. Native event streaming and one authoritative conversation identity remain follow-up work.
- `Runtime proof required: no` in the original Hello World proposal is not fixed by this pass; intent extraction and live execution acceptance still require targeted source investigation and Windows evidence. Never mark Hello World certified from build success alone.
- No changes to Vault identity, registry, port allocation, Desktop GUI, Rust source, external project trees or source-moving behavior.

## Validation performed before packaging

- Staged baseline: 2026-09-28 source rollup + W13B2A + W13B2B overlays; exact W13B2B preimage hashes required on every replaced file.
- Python compilation and 319 control tests passed on Linux in staging. Rust compilation and actual Windows PCC FULL, binary launch, interactive console and Hello World execution are **not** certified here.
- New ZIP is root-relative and transactional with `cortex.root_patch.v1` manifest and SHA-256 sidecar.
