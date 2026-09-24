# Cortex / Rust Forge Quality and Certification Contract

**Current truth:** 2026-09-23

## Cortex `full` source gate — implemented authority

Canonical implemented order:

1. quick/root/toolchain/typed-contract sanity;
2. mandatory Python/PCC regression suite;
3. `cargo fmt --all -- --check`;
4. `cargo check --workspace --all-targets`;
5. `cargo test --workspace --all-targets`;
6. `cargo clippy --workspace --all-targets -- -D warnings`;
7. `cargo build --workspace`;
8. capture governed-source fingerprint;
9. content-addressed project Vault mirror excluding rebuildable/operational/transport/link residue;
10. prove governed source did not change during mirror capture;
11. deep SHA-256 verification of the exact latest mirror;
12. write GREEN governed-source marker;
13. certify the exact Vault recovery snapshot against the GREEN source fingerprint;
14. emit structured PASS/FAIL evidence and debug bundle on failure.

A `FULL QUALITY GATE GREEN / SOURCE CERTIFIED` result means source/build/test quality plus a verified recovery point. It does **not by itself** claim provider/chat behavior or complete Windows UI interaction.

## Windows/runtime acceptance

Before a product/runtime milestone is called fully certified on the development machine, also verify:

- `launch-gui` observes a visible native top-level window owned by the exact launched PID; process survival alone is failure/inconclusive;
- a real configured provider can send/stream/complete a prompt;
- the same conversation reloads after restart;
- cancel and error paths are visible and recoverable;
- structured file/diff/chat cards render without leaking raw transport payloads into normal conversation presentation;
- runtime logs/debug evidence are retained under the operational artifact tree, not mirrored as governed project source.

These are intentionally separate from the source gate until safe automated runtime teardown and provider-independent fixtures are proven.

## Rust Forge candidate gate

Run independently through `forge-rust-gate`:

1. format check;
2. workspace/all-target Cargo check;
3. workspace/all-target tests;
4. Clippy `-D warnings`;
5. `forge-rs` build.

A GREEN candidate gate proves source/build health only. It does not grant takeover.

## Rust Forge runtime/parity acceptance

After candidate source GREEN, verify project/provider load, durable operations, registry/fleet state, Cortex host connection, active-project rebinding, patch/artifact/VCS/intake inspection, watcher/toolchain/diagnostics, native IDE workflows, protocol transport, Ember host inspection and release/recovery behavior. Mutation paths require explicit recovery evidence.

## Takeover certification

ForgePY/Python PCC remains production authority until the required takeover checks are VERIFIED side-by-side for project/fleet discovery, operation cancellation, patch apply/rollback, Vault/artifact lineage, GitHub/Internal Git, diagnostics/recovery, services, Cortex hosting, scheduling, IDE/platform/distribution and other workstation behavior.

## Explicit exclusions

Do not reintroduce obsolete migration/separation marker gates. Do not require WebView/Monaco for native IDE certification. Do not treat candidate/declared parity as verified runtime parity.

## Command risk vocabulary and compatibility

The typed project contract uses `read_only`, `local_mutation`, `external_mutation`, and `destructive`.
Legacy donor/project adapters may expose `read`, `write`, or `confirm`; universal discovery may interpret those aliases in memory (`read` -> `read_only`, `write`/`confirm` -> `local_mutation`) but must not silently rewrite the project source. Unknown risk values remain fail-closed. Cortex's root `project.control.json` stays canonical and must contain explicit quality-gate stage arrays.
