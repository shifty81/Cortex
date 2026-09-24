
## Portable external-drive Vault setup

For a laptop/external-drive setup, keep this source in a project folder such as `E:\\Cortex\\` and run `SETUP_EXTERNAL_DRIVE_VAULT_ROOT.cmd`. The setup binds the **drive root itself** (for example `E:\\`) as the machine Vault authority. See `docs/EXTERNAL_DRIVE_ROOT_VAULT.md`.

# Forge + Cortex — Current Project Authority

**Date:** 2026-09-23

## Product hierarchy

- **Forge** is the primary user-facing universal workstation, project manager and project-control shell.
- **Cortex** is Forge's integrated intelligence/automation backend and remains independently testable/service-capable.
- **Vault** is the shared storage, catalog, provenance and recovery subsystem used by the control plane.
- **PCC capabilities** are universal project-control capabilities inside this same system, not a competing end-user product.
- **Ember and other games/tools** remain independent managed projects that consume the universal contracts.

The target is one cohesive application/repository direction for Forge + Cortex + Vault + PCC behavior while keeping hosted projects independently buildable and recoverable.

## Current authority boundary

The Python Forge/PCC lane remains production authority today. Rust Forge RS04–RS293 is a candidate lane and must pass local build/runtime/parity/takeover certification before replacing the production control plane.

Cortex owns AI/agent orchestration, conversations, context/memory, providers/models, tools/permissions, jobs/activity, project intelligence, review evidence, plugins/skills/protocol contracts and Cortex CLI/API/service behavior.

Forge/PCC owns universal project discovery/control, build/test/run/gate orchestration, update transactions, Vault/project mirrors, artifacts/provenance, source control, recovery and takeover certification.

## Storage rule

Use one governed Vault. Shared dependency/download caches are machine-wide; project build outputs remain safely namespaced; each registered project has a deduplicated content-addressed mirror. FULL GREEN requires a source-stable, deep-verified project mirror and a recovery certification tied to the GREEN source fingerprint.

Do not silently delete legacy caches or recovery material. Cleanup remains explicit, planned, quarantined and reversible before purge.

## Current testing truth

Python/PCC behavior can be certified in this source environment. Rust compilation, Windows GUI startup and real provider/chat behavior remain authoritative only when run on the Windows development machine.

See `README.md`, `docs/QUALITY_GATE_CONTRACT.md`, `docs/CURRENT_IMPLEMENTATION_AUDIT_RS293.md`, and `products/forge-rust/docs/FORGEPY_PARITY_MATRIX.md`.

## Portable first-run environment hydration

On a fresh Windows machine, Cortex no longer requires Python to be installed manually before the PCC can start. `PROJECT_CONTROL_CENTER.cmd` now runs a PowerShell bootstrap first and can hydrate shared Python, Git, Rust, rustfmt and clippy into the configured Vault.

For the external-drive-root layout, run `SETUP_EXTERNAL_DRIVE_VAULT_ROOT.cmd` once, then launch `PROJECT_CONTROL_CENTER.cmd`. You can also force the first hydration with `BOOTSTRAP_CORTEX_ENVIRONMENT.cmd`. If Windows C++/MSVC linker tooling is missing, run `HYDRATE_CORTEX_BUILD_TOOLS.cmd` once before Full Gate certification.

Environment evidence is written to `artifacts/bootstrap/environment-health.json`; hydrated toolchains live under the Vault `shared/` namespace rather than inside every project. See `docs/CORTEX_ENVIRONMENT_HYDRATION.md`.

## Portable bootstrap recovery (R8A)

Portable-drive builds carry `config/cortex/portable_drive_root_vault.v1.json`, so a repository at `E:\Cortex` automatically selects `E:\` as Vault authority unless `CORTEX_VAULT_ROOT` or `PCC_VAULT_ROOT` explicitly overrides it.

If dependency hydration fails after Python has already installed, PCC should still launch and report the remaining build dependency as unhealthy. Rustup downloads are SHA-256 verified from Rust's official `.sha256` endpoint; stale or mismatched cached installers are deleted and retried up to three times.

## Portable bootstrap recovery (R8B)

R8B hardens the portable Windows bootstrap after real external-drive testing. Rust's official `.sha256` endpoint may be returned by Windows PowerShell as binary `application/octet-stream`; the bootstrap now decodes byte content before parsing the digest. Portable Python remains the full Windows distribution because the PCC GUI requires Tkinter. If Python 3.14.7 was already installed by an earlier Cortex bootstrap under the legacy AppData Vault or another registered per-user location, the installer may no-op instead of creating a second target directory. R8B discovers that complete runtime, migrates it into the active drive-root Vault, and validates both Python 3.11+ and Tkinter before marking launch readiness.
