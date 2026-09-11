# Current Cortex / Forge Audit

Committed baseline: RS03 GREEN at `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5`.

## Implemented Cortex authority

Cortex already has a native CLI, Desktop/controller/native host, service/RPC, provider router, local/native/LM Studio/ComfyUI provider paths, model host, tool broker, permissions, conversations, context, project/workspace registry and discovery, jobs/activity/tasks, transactions/recovery, observability, Vault/library, artifacts, review, plugins/skills/protocol manifests and project operations.

The old “CLI not built yet / GUI not built yet” diagnosis is obsolete.

## Forge production/candidate split

ForgePY remains the production universal workstation. Rust Forge is a parallel candidate built through the Cortex project provider.

RS03 is committed GREEN and established durable Rust Forge operation IDs, receipts, logs, cancellation and interrupted recovery.

RS04–RS293 is cumulative candidate source and must be built before any of it is called GREEN.

## Candidate implementation through RS293

The candidate now includes:

- Forge-hosted Cortex normalization;
- persistent machine project/fleet state;
- nested project graph and active-project switching;
- native `forge.patch.v1` transactional update engine;
- Artifact Central promotion/index/retention;
- GitHub/origin + Forge Repository/Internal Git backend;
- durable scheduler execution plus conservative resource planning;
- service lifecycle/logging/health;
- manifest-aware patch intake and polling watch snapshots;
- native toolchain doctor and diagnostic aggregation;
- native release/update/recovery evidence;
- fail-closed takeover comparison;
- **native Rust IDE with no WebView requirement**;
- native LSP/DAP-style stdio JSON-RPC framing;
- Ember host adapter contract;
- platform notification/tray state model.

## Remaining high-value gaps

1. First GREEN compile/test/Clippy/runtime certification of the cumulative candidate.
2. Full asynchronous language-server/debug-adapter event loop and rich IDE diagnostics/completion UI.
3. Rope/tree-sitter-class editor internals and syntax rendering after the native editor spine is certified.
4. OS-native filesystem notification backend; current watcher is bounded polling/snapshot based.
5. Truly concurrent scheduler workers with shared-state locking; current planner exposes budgets but execution remains conservative/sequential.
6. Native Windows system tray/toasts.
7. Installer/updater/signing/self-recovery and OS-backed secure credential storage.
8. Actual installed ForgePY side-by-side takeover fixture run.
9. Remaining rich Cortex review/tool-card/attachment/accessibility parity.
10. Real Ember editor/component migration.

## Build order

Apply only the newest RS04–RS293 cumulative patch, run `forge-rust-gate`, repair the first real compiler/test/Clippy blocker, smoke the native IDE/Cortex/project operations, then run the Cortex Full Quality Gate. Commit/snapshot only when both lanes are GREEN.
