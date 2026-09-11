# Forge Rust — Parallel Candidate

Status: **RS03 committed GREEN; RS04–RS293 cumulative candidate pending build**.

Forge Rust is the native implementation of Forge, the universal workstation that hosts Cortex intelligence and Ember game authoring. It remains isolated under `products/forge-rust/` while takeover parity is developed.

ForgePY remains production authority.

## Candidate crates

- `forge-contracts` — `forge.project.v1` and capability authority;
- `forge-core` — project session/provider routing;
- `forge-process` — durable operation IDs/receipts/logs/cancellation/recovery;
- `forge-state` — machine fleet/settings/artifact/lineage/service/queue/source-control/takeover state;
- `forge-update` — modern patch validation/transaction/rollback evidence;
- `forge-artifacts` — Artifact Central verified promotion/index/retention;
- `forge-vcs` — Git state/worktrees/branches/Internal Git/GitHub-origin backend;
- `forge-scheduler` — persisted project queue plus conservative resource planning;
- `forge-services` — service lifecycle/logging/health;
- `forge-intake` — patch intake classification, watch integration and explicit promotion;
- `forge-certify` — semantic takeover certification foundation;
- `forge-ide` — native multi-tab editor/session/search/guarded-save authority;
- `forge-protocol` — native stdio `Content-Length` JSON-RPC transport for LSP/DAP-style processes;
- `forge-watch` — bounded filesystem snapshot/diff watcher;
- `forge-toolchain` — project requirement/toolchain doctor;
- `forge-diagnostics` — native Forge diagnostic aggregation;
- `forge-ember` — project-provided Ember host adapter contract;
- `forge-platform` — platform/tray/notification state contracts;
- `forge-release` — release manifests, update plans and recovery checkpoints;
- `cortex-bridge` — adapter over real Cortex Desktop core/client/protocol;
- `forge-rs` — native Forge shell.

## Native IDE

The IDE is native Rust and is rendered directly in Forge. It does **not** require Monaco/WebView/Electron/Chromium.

Current candidate behavior includes multi-tab editing, bounded undo/redo, dirty/conflict state, SHA-preimage guarded saves, project file/search surfaces, language-tool discovery and native LSP/DAP stdio protocol framing.

## Build through ForgePY/Cortex

```text
ForgePY -> Cortex project provider -> forge-rust-gate / forge-rust-build / forge-rust-run
```

Direct developer commands remain available through the nested Cargo workspace.

## Truth rule

Every unbuilt surface remains `Candidate`. GUI labels, contracts or source presence do not make a subsystem production authority until its required gate/runtime/takeover evidence passes.
