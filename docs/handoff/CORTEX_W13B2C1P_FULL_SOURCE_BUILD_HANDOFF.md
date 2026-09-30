# Cortex W13B2C1P Full Source Build Handoff

## Authority

This cumulative source rollup is reconstructed from the last published GREEN Cortex
checkpoint `034bdaf189bdb167af60918603a57325842072f7` plus the W13B2C1P
Interface Preservation + Evidence source overlay.

It is a **source rollup**, not a root patch and not a Vault/state backup.

## Included

- PCC-only application direction retained.
- Cortex CLI + PCC common-controller path retained.
- W13B2B fail-closed coding/chat integrity retained.
- W13B2C1 terminal-agent + request observability retained.
- W13B2C1W Windows fixture correction retained.
- W13B2C1P interface preservation/evidence commands integrated.
- Existing Rust CLI, PCC, Vault, project, Git, build/test, recovery and provider code retained.

## Intentionally excluded

- `.git`
- `.cortex`
- `artifacts`
- build output / `target`
- live Vault contents
- local patch receipts
- user PCC conversation data under `data/conversations`
- models, credentials, tokens and machine-local state

Those remain owned by the existing Cortex installation/Vault and must not be replaced
from a source rollup.

## First build / certification

The W13 patch payloads used for reconstruction contain the pre-rustfmt Rust source
for a few files. The corresponding logical source is already proven by the later
GREEN Windows checkpoint, and W13B2C1P itself is Python-only. On a fresh extracted
copy, normalize Rust formatting once before certification:

```powershell
cargo fmt --all
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
```

When installed into the governed Cortex repository, run PCC FULL afterward.

Do not copy or manufacture `.cortex` / Vault authority from this ZIP. Preserve the
existing marked physical-volume authority and project registry.

## Authoring-environment verification

The cumulative Python/control source in this rollup passed:
- 339 `tools/control/tests` tests
- 81 `tests` inventory tests (1 skipped)
- capability-preservation guard PASS (10 source files inspected)

Rust compilation cannot be executed in the ChatGPT packaging container because that
container has no Rust toolchain. The user's Windows checkpoint immediately beneath
this source lineage passed fmt/check/test/clippy/build before the W13B2C1P Python-only
overlay.
