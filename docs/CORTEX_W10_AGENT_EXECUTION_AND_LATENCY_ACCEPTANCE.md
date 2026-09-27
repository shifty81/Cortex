# Cortex W10 — Cumulative agent-execution and chat-latency repair

**Prerequisite:** Published R8J-A or already installed W9. One repository-root overwrite archive contains all W9 payloads and W10 replacements. Do not install older B1 or W8/W9 patches before this if starting from published R8J-A. Close PCC first. W10 Windows FULL is **not yet certified**.

## Observed user evidence (2026-09-26)

- Windows FULL twice GREEN. Last source fingerprint `ba35d46c0fcb9a779e36dfb6ac2fd86b451d69702e841c289207b570dee50aa9`; current user log shows commit/push of `bf271d1`.
- Chat requests logged `CORTEX-PY-BRIDGE-0.7 | worker=<prebuilt cortex.exe>` and returned generic assistant prose; **not** a build or transactional repair.
- The GUI initially selects Chat, a non-mutating mode. `Repair Plan`/`Repair + FULL` are for classified active gate failures; after a newer GREEN gate, coordinator returns `NO_REPAIR_NEEDED`.
- The bridge launched the prebuilt worker anew every turn and rebuilt workspace context; the historical BuildDoctor path recursively scanned artifact logs each time, and even ordinary chat additionally rglobbed/sorted `.log` files.

## W10 changes

- In Chat composer, `/repair`, `/apply`, `/inspect`, `/plan`, `/chat` choose their corresponding modes. Imperative code-change requests such as `Fix the failing Rust test` offer governed Repair rather than silently becoming generic read-only chat. Apply/Repair require explicit GUI confirmation; cancellation retains the prompt. Console `/apply` and `/repair` also prompt for confirmation.
- Distinguish prebuilt conversational worker from transactional operation in worker status/response; do not represent chat as a source edit.
- Ordinary Chat bypasses historical BuildDoctor scans, per-turn Git status subprocess, and log enumeration. Diagnostic modes retain project evidence and select at most three logs from a shallow 256-entry budget.
- Optional `CORTEX_BRIDGE_TRACE=1` emits `provider_ready_ms`, `context_ms` and `worker_response_ms` to Project Console. This separates model/provider startup from local context preparation and inference. Chat need not compile Cortex; only explicit Build Worker / FULL runs Cargo.
- Keeps the W9 cumulative payload and fixes historical assertions impacted by this version change.

## Acceptance once on Windows

1. Confirm GUI `PCC-GUI-0.15.6a`. Your existing inventory and Vault catalog must remain untouched; no index rebuild.
2. Send `hello` in Chat twice. No `cargo build` should appear. Optional bridge trace above should show provider readiness, context and worker time; a response may still wait on inference/model loading or fresh-process startup.
3. Enter `/repair fix a deliberately broken test in a disposable project`. Verify the confirmation dialog, a real transactional worker/tool attempt in the execution transcript, source diff, targeted validation and rollback/commit evidence. If no mutation occurs, treat that as **FAIL**, not success based on prose.
4. Cancel a Repair confirmation once; composer text must remain and project bytes must not change.
5. Verify `/apply` and `/repair` console commands also request confirmation. Run `Repair Plan` against a failing project and `Repair + FULL` on that failure. Do not expect the latter to change anything when there is no active failure after GREEN.
6. Run authoritative Windows FULL, then publish if GREEN. W10 is not a resident worker daemon: persistent worker reuse and true streaming JSON chat output remain follow-on performance work.

## Existing limits / scope

No automatic source moving, registration, Vault catalog mutation, SQLite schema change or inventory rescan. This patch improves routing and eliminates unbounded log work on ordinary chat; it does not claim that a local model can reliably call edit tools until a real Windows repair acceptance passes. The worker still starts fresh for each message, and the model host may take up to 45s to bootstrap when initially offline. `Chat` stays intentionally non-mutating unless the user explicitly confirms a transactional request.
