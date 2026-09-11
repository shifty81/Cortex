# Cortex — Intelligence Runtime Hosted by Forge

Date: 2026-09-11  
Committed baseline: `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5` (RS03 GREEN)  
Current development lane: Rust Forge cumulative candidate RS04–RS293.

## Product hierarchy

- **Forge** is the user-facing universal workstation and project/build/control/tooling shell.
- **Cortex** is the intelligence/runtime subsystem under Forge and remains independently testable/service-capable.
- **Ember** is the game-authoring/editor system hosted by Forge and powered by Cortex.

The intended normal user experience is one Forge workstation with first-class Cortex and Ember workspaces, not three competing top-level control applications.

## Cortex authority

Cortex owns AI/agent orchestration, conversations, context/memory, providers/models, tools/permissions, jobs/tasks/activity, project intelligence, review intent/evidence, plugins/skills/protocol contracts and Cortex CLI/API/service behavior.

Cortex does not own competing universal implementations of Forge project operations, Artifact Central, universal patch intake, Forge Repository/Internal Git, GitHub source hosting, universal build orchestration or Ember editing.

## Forge authority

Forge owns project/fleet state, universal operations, build/test/run routing, update transactions, Artifact Central, GitHub/Internal Git, services, IDE, platform state, release/recovery and takeover certification.

ForgePY remains production authority until Rust Forge passes explicit takeover certification. The committed source is GREEN through RS03; RS04–RS293 remains candidate until the local nested Forge gate and Cortex Full Gate pass.

## Native IDE direction

The Forge IDE is **native Rust**. WebView/Monaco is not required and is no longer the active architecture.

The current candidate includes:

- native egui IDE workspace;
- multi-tab text editing;
- bounded undo/redo;
- dirty/conflict state;
- SHA-preimage guarded transactional saves;
- project file listing/search;
- language-tool discovery;
- native stdio `Content-Length` JSON-RPC transport for LSP/DAP-style processes;
- native terminal profile contracts.

Rope/tree-sitter-class editing, richer syntax presentation and full asynchronous LSP/DAP UI integration remain later hardening work after the first candidate build.

## Current truth

The Cortex native CLI and Desktop/controller stack are already implemented. Older documents stating that they are “not built yet” are stale historical material.

See:

- `docs/CURRENT_IMPLEMENTATION_AUDIT_RS293.md`;
- `docs/QUALITY_GATE_CONTRACT.md`;
- `docs/TARGET_ARCHITECTURE.md`;
- `docs/FORGE_RUST_PARALLEL_LANE.md`;
- `products/forge-rust/docs/RS04_RS293_CUMULATIVE_ROLLUP.md`.
