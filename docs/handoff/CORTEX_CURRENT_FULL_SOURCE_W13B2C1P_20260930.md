# Cortex Current Full Source — W13B2C1P — 2026-09-30

## Integrated authority

This is the cumulative Cortex source baseline assembled from the last published
GREEN checkpoint:

`034bdaf189bdb167af60918603a57325842072f7`

plus the W13B2C1P Interface Preservation + Evidence source overlay.

No W13B2C1P source file remains external to this rollup.

## Included

- PCC-only application-shell direction
- Rust CLI-first Cortex engine
- `pcc-chat` common developer-controller path
- W13B2B fail-closed coding / approval behavior
- W13B2C1 terminal agent + request observability
- W13B2C1W Windows path-fixture correction
- W13B2C1P interface-preservation capability guard
- read-only conversation/history inspection commands
- read-only operation/evidence inspection commands
- current Vault/project/Git/build/recovery/provider source
- current PCC GUI and CLI source

## Static / Python verification performed during packaging

- `tools/control/tests`: 339 passed
- `tests`: 81 passed, 1 Windows-only junction test skipped
- `CortexCapabilityGuard.py`: PASS, 10 protected source files inspected
- source-rollup archive verification: required before delivery

## Rust build note

The cumulative overlay was reconstructed from W13 patch payloads. Three Rust files
retain pre-rustfmt layout even though their logic corresponds to the previously
GREEN Windows lineage. The ChatGPT packaging environment does not contain Rust,
so rustfmt cannot be executed here.

On the extracted Windows checkout, run once before FULL:

```powershell
cargo fmt --all
cargo fmt --all -- --check
git diff --check
```

Then run PCC FULL. The preceding Windows checkpoint passed Rust fmt/check/test,
Clippy with warnings denied, and workspace build before the Python-only C1P overlay.

## Deliberately not embedded

This is source, not a machine/Vault clone. It intentionally does not replace:

- `.git`
- `.cortex`
- Vault managed state
- models
- credentials or tokens
- `target`
- operational `artifacts`
- local patch receipts
- user conversation data

Preserve those from the existing Cortex installation.

## Patch-lineage note

W13B2C1P is integrated in source here. The existing machine may still lack a
transactional W13B2C1P patch receipt because this cumulative rollup integrated the
same source directly. That receipt should be reconciled separately after FULL GREEN;
it is not application source and is intentionally not fabricated inside this ZIP.
