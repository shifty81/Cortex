# Cortex / Rust Forge Quality and Certification Contract

**Current source behavior verified:** 2026-09-26 (R8J-B1-W7 audit)

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
9. write the GREEN governed-source marker for the current source fingerprint;
10. emit structured PASS/FAIL evidence and a debug bundle.

**Implemented boundary:** `GateEngine.full()` in `tools/control/CortexPCC.py` performs the Python/Cargo stages and invokes Git `mark-green`. `mark-green` writes the governed-source fingerprint; neither function calls `PCCVaultStorage.mirror_project` or `verify_latest_mirror`. The standalone `vault-mirror` and `vault-verify` commands are separate operations. Therefore `FULL QUALITY GATE GREEN / SOURCE CERTIFIED` means the source/build/test gate passed and its fingerprint was marked; it **does not currently prove a deep-verified Vault recovery point** or a completed restore drill. It also does not claim provider/chat behavior or complete Windows UI interaction.

**Required recovery-gate integration before treating FULL as recoverability certification:** capture a source-stable content-addressed mirror excluding rebuildable/operational/transport/link residue; prove unchanged source fingerprints before and after capture; verify exact mirror contents by SHA-256; bind and certify a recovery receipt to the same GREEN fingerprint; fail FULL and avoid a successful GREEN marker if any mandatory stage fails. Add negative fixtures for corruption, source drift, cancelled mirror and failed recovery verification before activating this stage. Until then, mirror and verify are explicit separate operations, not implicit in GREEN. This is a documented gap, not a claimed implementation.

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
