# Cortex / Forge Roadmap — Current Authority

Cortex separation is complete. Current work is Forge convergence and takeover certification.

## C0 — Cortex product health

Committed RS03 baseline is GREEN through the current project control flow.

## C1 — Cortex native authority

Implemented: native CLI, Desktop/controller/native host, service/RPC, providers/model host/tools, conversations/context/jobs/activity/tasks, permissions/plugins/skills, transactions/recovery/review, Vault/library/observability/project intelligence.

Remaining Cortex work is parity/hardening/release work, not a native bootstrap.

## F0 — Forge contract/capability authority

Implemented: `forge.project.v1`, capability discovery and project-owned provider execution. ForgePY remains production authority.

## F1 — Rust Forge deterministic operation spine

GREEN at RS03: native shell, capability-driven commands, durable operation IDs, receipts/logs, cancellation and interrupted recovery.

## F2 — Forge-hosted Cortex

RS04–RS63 candidate: real Cortex donor/controller reuse, conversations/streaming agent modes, files/workbench/reviews/Vault/tasks/settings, provider/context/repository actions and host boundaries.

## F3 — Forge persistent machine state

RS64–RS93 candidate: fleet/nested-project state, active project, Artifact Central lineage state, service/queue/source-control state and takeover matrix.

## F4 — Native universal operations

RS94–RS193 candidate: native modern patch engine, Artifact Central, Git/Internal Git, scheduler, services, intake, certification, IDE backend, platform state and release/recovery foundations.

## F5 — Native workstation hardening

RS194–RS293 candidate adds:

- native IDE presentation inside Forge;
- multi-tab editor state, undo/redo and conflict-safe saves;
- native LSP/DAP stdio protocol transport;
- project file/search/tool discovery;
- polling filesystem/intake watch snapshots;
- project toolchain doctor;
- native diagnostics aggregation;
- extended Git branch/pull/commit/history operations;
- scheduler resource planning/fairness constraints;
- release update planning and explicit recovery restore;
- tray/menu/notification state contracts;
- Ember host adapter discovery;
- updated takeover matrix and acceptance evidence.

WebView/Monaco is not part of the required IDE architecture.

## F6 — Platform completion

Still required:

- OS-native file notifications and resumable large-drive crawler;
- asynchronous LSP/DAP integration and richer code-editor rendering;
- system tray + Windows toast implementation;
- secure credential storage;
- installer/updater/signing/self-recovery;
- scheduler true parallel workers with locking;
- accessibility/keyboard polish;
- complete Forge-host Cortex parity.

## F7 — Takeover certification

Rust Forge must run side-by-side fixtures against installed ForgePY and prove semantic parity for discovery, build/test/run, patch safety, source control, Artifact Central, recovery, logs, services, Cortex hosting and scheduling. Every required takeover check must be VERIFIED.

ForgePY stays production authority until F7 is GREEN and takeover is explicitly approved.

## E0 — Ember game-authoring migration

After the Forge host is stable enough, migrate real Ember authoring systems: World/Scene, content browser, sprite/pixel/animation, terrain/tile/level, node/gameplay logic, UI/audio/dialogue, runtime/PIE, asset pipelines and Cortex-assisted editor actions.
