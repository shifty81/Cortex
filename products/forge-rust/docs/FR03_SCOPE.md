# FR03 — Durable Operations, Receipts, Cancellation + Logs

FR03 makes Rust Forge operations durable and inspectable without changing the authority boundary: ForgePY remains the production universal controller while the Rust implementation proves parity.

## Implemented

- every Rust Forge project operation receives a filesystem-safe durable operation ID before process launch;
- each operation writes `artifacts/forge-rust/operations/<operation-id>/receipt.json`;
- stdout/stderr are streamed to the native console and persisted to `operation.log`;
- receipts move through explicit `queued -> running -> terminal` states;
- terminal states distinguish `succeeded`, `failed`, `cancelled`, `start_failed`, and `interrupted`;
- operation receipts persist PID, argv, cwd, start/end time, elapsed time, exit code, cancellation state, log path, receipt path, and host error detail;
- the GUI exposes STOP controls in the quick-action bar, project console, and health rail;
- Windows cancellation requests terminate the process tree through `taskkill /T /F` with a direct child-kill fallback;
- non-Windows cancellation uses the child-process kill path;
- startup recovery converts orphaned nonterminal receipts to `interrupted` without falsely claiming that an external process stopped;
- the Operations workspace shows durable recent history and authoritative receipt/log locations;
- `forge-operation-inspect --root <project> [--limit N]` emits machine-readable operation-host capabilities and recent receipt history;
- `forge.operation_host.v1` declares durable IDs, receipts, logs, cancellation, interrupted recovery, and history capabilities.

## Storage contract

Runtime evidence is operational data, not governed source:

```text
<project>/artifacts/forge-rust/operations/
  forgeop-.../
    receipt.json
    operation.log
```

This keeps project roots clean while making operation history available to Forge, Cortex, diagnostics, Artifact Central, and later takeover certification.

## Authority boundary

FR03 does not make Rust Forge production authority. ForgePY remains the active universal build/control front end, and project-owned PCC/CLI providers remain the execution authority for project-specific operations.

## Verification

The Rust Forge candidate gate should run format, check, tests, clippy with warnings denied, and build against `products/forge-rust/Cargo.toml`. The normal Cortex Full Gate should then remain GREEN.

## Next pass

FR04 should turn durable operation evidence into typed operation/result bindings for Artifact Central and multi-project scheduling, then add provider health/version handshake data so Forge can reject incompatible project providers before execution.
