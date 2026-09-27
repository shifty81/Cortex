# Cortex W12 — Candidate Survival and Safe Failure Evidence

**Testing candidate; NOT Windows/Rust/full-gate certified.** No changes to Havenwild. This is a strict, small **incremental source updater** for the user-published, post-rustfmt W11 GREEN Cortex source. It is not a full source rollup and is not a governed root-drop archive: do not drop this ZIP into Cortex `updates/`. A later cumulative replacement should be assembled from a newly certified W12 source checkpoint.

## Provenance and scope

- Live W11 GREEN GitHub checkpoint: `shifty81/Cortex` at `71c63c7c8a5ae037b3c8294acb4b751580ea10fa` (27 Sep 2026 00:59 checkpoint). The updater verifies **exact Git blob hashes** for `crates/cortex_cli/src/lib.rs` and `crates/cortex_core/src/lib.rs` before any writes, rather than assuming a drive letter, branch name, or stale preformatted W11 archive.
- Writes only these two existing Rust source files when `--apply` is specified. Keeps byte-exact backups and a manifest in `G:\Cortex\artifacts\updates\w12-green-source-<UTC>/` (substituting the actual selected project root). No automatic Git operations, builds, registry edits, Drive inventory reads, Vault changes, source moves, or Havenwild writes.
- The updater preserves the currently installed green source and applies anchored source modifications in memory. If any anchor or preimage differs it refuses before writing anything. It does not overwrite the previously formatted Rust files with older W11 ZIP contents.

## Correct PowerShell usage

Extract this ZIP to a separate directory, such as your Desktop, not within Cortex. From that extracted directory:

```powershell
$py = "G:\shared\toolchains\python\3.14.7\python.exe"
& $py .\W12_GREEN_APPLY.py --root "G:\Cortex"          # PREVIEW: no changes
& $py .\W12_GREEN_APPLY.py --root "G:\Cortex" --apply  # EXPLICIT source update

Set-Location "G:\Cortex"
cargo fmt --all
cargo fmt --all -- --check
git diff --check
& .\PROJECT_CONTROL_CENTER.cmd
```

Choose **FULL**. Do **not** commit or push until the complete Windows quality gate is GREEN. If preimage validation fails, stop and provide a current source rollup; do not force the updater or apply an old W11 archive. This script will not itself run FULL.

## What W12 changes

1. **Candidate survival barrier:** after the agent turn, check actual transaction status, original transaction ID and touched/created ledger **before** invoking a project-owned checkpoint. An already rolled-back mutation immediately yields `candidate_not_active_after_model`, without rerunning native FULL against restored source.
2. **Unexecuted tool-call barrier:** if the final repair narrative still contains a line beginning `<tool_call>` or `<function_call>`, record `unexecuted_tool_call_in_model_report`, leave the retained candidate for review and do not invoke FULL.
3. **Evidence schema v3:** receipt gains pre-gate status/ledger, `checkpoint_executed`, `checkpoint_skip_reason`, and per-call bounded source-relative `target_path`, fixed-vocabulary `failure_class` and `candidate_decision`. It does **not** include raw mutation payloads, full error bodies, or credentials in those fields. The receipt still includes the user's original prompt and verification outputs, so keep it private.
4. **Regression coverage:** adds in-source Rust tests for rolled-back candidate, retained candidate, pseudo-tool-call rejection, safe error classification, sensitive text exclusion and controller disposition. The standalone Python tests verify fail-closed preimage/anchor checks, backups and reapplication refusal.

## Known limitations and acceptance

- Python fixture suite here: 4 tests passed. This environment has no Rust/Cargo compiler, so `cargo fmt`, Rust compilation, Clippy and Windows FULL are **not yet certified**; only the real Windows gate can promote W12 to GREEN.
- The updater targets current exact source hashes. The fixture contains **historical pre-format W11 source only for offline transformation tests**; those fixture files are not applied to your project.
- W12 prevents wasted FULL after candidate loss; it does **not** guarantee that the local model invents a valid dependency fix or eliminate the first 280 seconds of model inference. A future pass can optimize resident provider/worker latency and add a bounded agent retry policy.
- If the failure is `candidate_not_active_after_model`, do not immediately re-run repair; attach the v3 evidence receipt. The candidate may already be safely rolled back, and the tool's recorded disposition/error classes should explain the next correction.
