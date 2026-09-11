# Cortex / Forge Current Implementation Audit — RS293 Candidate

Committed baseline: `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5` (RS03 GREEN).  
Cumulative candidate: RS04–RS293, pending local compile/test/runtime certification.

## Current stage

Cortex itself is a mature native intelligence/runtime system. The main implementation risk has shifted to Rust Forge takeover parity and workstation completion.

The cumulative Rust Forge candidate now has four major layers:

1. **RS04–RS63:** Forge-hosted Cortex normalization and host boundaries.
2. **RS64–RS93:** persistent Forge machine/fleet state and takeover matrix.
3. **RS94–RS193:** universal takeover backends for updates, artifacts, VCS, scheduling, services, intake, certification, IDE backend, platform and release/recovery.
4. **RS194–RS293:** native IDE/no-WebView hardening, protocol transport, watcher/toolchain/diagnostics, VCS/scheduler/release hardening and Ember host discovery.

## Native IDE truth

WebView/Monaco is no longer required.

Candidate implementation includes:

- native egui workspace rendered directly in Forge;
- project file list/filter;
- project-relative safe file open;
- multi-tab editing;
- dirty/conflict markers;
- bounded undo/redo;
- SHA-preimage save guard;
- backup/restore-style atomic save publication;
- active-buffer find results;
- language identification;
- language tool discovery;
- terminal profile discovery;
- native `Content-Length` JSON-RPC stdio transport suitable for LSP/DAP processes.

Still missing for full IDE parity:

- rope/piece-table-class large-file buffer performance;
- tree-sitter-class incremental syntax parsing/highlighting;
- asynchronous LSP request/event lifecycle and diagnostics/completion UI;
- DAP session/state/breakpoint UI;
- integrated terminal viewport;
- split editor groups, minimap, breadcrumbs, outline, command palette and richer keybinding system;
- Cortex inline edit/diff/review cards within the editor.

These are native-editor hardening items, not reasons to reintroduce a browser host.

## Universal Forge backend state

| Area | Candidate implementation | Remaining before takeover |
|---|---|---|
| Project/fleet | persistent registry + nested graph | large-drive/resumable discovery runtime proof |
| Operations | RS03 durable IDs/receipts/logs/cancel | runtime regression smoke |
| Updates | native `forge.patch.v1` transaction/rollback | parity fixtures + runtime mutation test |
| Artifact Central | verified promotion/index/retention | parity/runtime and richer retention policy |
| Patch lineage/intake | classification + explicit promotion + watch snapshot | OS-native file events + installed ForgePY parity |
| GitHub/Internal Git | inspection, worktrees, snapshots, branches, pull/commit/tag/history | rich GUI + parity fixtures |
| Scheduler | durable execution + budget planner | certified concurrent worker pool/locking |
| Services | process start/stop/status/logs | richer dependency graph + recovery policy |
| Toolchains | project requirement/PATH/version doctor | install/remediation workflows |
| Diagnostics | native aggregated report | full debug-bundle promotion/parity |
| IDE | native editor + protocol contracts | richer editor/LSP/DAP/terminal features |
| Ember | host manifest/readiness contract | actual editor migration |
| Platform | notification/tray command state | native Windows tray/toast implementation |
| Release | manifest/update plan/recovery checkpoint | installer, signing, updater, self-recovery |
| Credentials | no fake persistence | OS-backed secure store still required |
| Takeover | fail-closed matrix/comparator | side-by-side installed ForgePY evidence |

## Safety invariants retained

- ForgePY remains production authority.
- Project-local PCC/CLI remains independently usable.
- Downloads do not auto-apply patches.
- Mutation-capable candidate APIs require explicit invocation/approval.
- Takeover `Candidate` never counts as `Verified`.
- Cortex and Ember do not become competing project-control backends.
- The native IDE does not bypass Git/transaction/preimage safety.

## First build risk

No Rust compiler is available in the artifact environment used to prepare this candidate. The first local `forge-rust-gate` is therefore authoritative for syntax/type/API/Clippy truth.

Repair policy: fix the first concrete compiler/test/Clippy failure in place; do not use failures as a reason to duplicate Cortex or revert to WebView architecture.
