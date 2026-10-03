# Cortex W14A — Functional Alpha Live Execution Convergence

## Purpose

W14A converts several previously separate Cortex/PCC infrastructure pieces into a coherent user-facing execution foundation. The pass is intentionally limited to Python/configuration/documentation surfaces that can be certified without a Rust toolchain in the handoff environment.

## User-visible changes

- PCC Chat no longer describes the entire Chat surface as read-only. Inspect/Plan remain read-only; normal natural-language Chat may route governed developer work through the existing controller; Apply/Repair remain explicit transactional modes.
- PCC Chat now projects live Cortex execution snapshots when available instead of repeating the five-second `no completed response yet` heartbeat.
- Project Console retains detailed Cortex stage/runtime/live telemetry while final assistant text filters those operational markers.
- PCC runtime status identifies the Cortex CLI/PCC GUI path instead of presenting the retired standalone Cortex Desktop as the current application surface.

## Repository/source authority convergence

- `CortexRepositoryLayout.py` is the canonical repository-layout scanner used by PCC maintenance.
- PCC Fast/FULL hygiene normalization fails closed when canonical layout violations remain after repair.
- Known historical root update artifacts are archived under `artifacts/maintenance/legacy-root-source/` rather than silently treated as healthy source.
- Legacy PCC conversation cache under `data/conversations` is migrated loss-aversely to `.cortex/conversations`.
- Source recovery rollups exclude legacy runtime conversation/registry state and known historical root update transports.
- Active storage policy no longer assumes `D:\CortexLibrary`; registry runtime state resolves under `.cortex/registry`.

## Safety behavior

- Malformed legacy conversation files are not deleted during migration; they remain visible to the canonical layout scanner so the repository stays red until they are resolved.
- Patch/source authority remains fail-closed.
- Live execution projection reads only the operational `active.state` key/value snapshot and does not expose private reasoning.
- This pass does not change Rust agent/runtime implementation because Cargo/Rust certification is unavailable in the handoff environment.

## Validation completed in handoff environment

- `tools/control` Python suite: 361 passed, 1 skipped.
- Root Python suite: 81 passed, 1 skipped.
- W14 focused convergence suite: 6 passed.
- Disposable full-source hygiene repair fixture: canonical violations reduced from 21 to 0; 19 known historical root files archived and 7 legacy conversations migrated.
- Source recovery rollup fixture excluded legacy runtime state and historical root-update debris.

## Windows acceptance after apply

Run the normal Cortex FULL gate on the target Windows checkout. W14A should be considered source-integrated only after the Rust workspace, PCC GUI, CLI controller, and Windows-native runtime checks pass there.

The next compile-certified milestone is true end-to-end Rust developer event streaming through `CortexEngine`/service/controller into PCC and Forge, followed by the autonomous Hello World create → build → run → repair certification scenario.

## W14A2 confirmation correction

The replacement W14A2 payload also closes the embedded patch-apply EOF path: generic Command Registry patch application now confirms in the GUI and forwards `--yes`, while the provider fails cleanly instead of calling `input()` when stdin is detached.
