# Cortex / Forge Unified Control Project

**Current authority date:** 2026-09-23  
**Operational control plane:** Python Forge/PCC + Cortex integration  
**Rust Forge:** cumulative candidate through RS293; not takeover-certified  
**Cortex:** intelligence, conversation, agent, provider/model, tool, project-intelligence and automation runtime

## Current product direction

This repository is converging on one operator experience rather than competing top-level tools:

- **Forge** is the primary workstation/application shell and universal project-control surface.
- **Cortex** is integrated under Forge as the intelligence and automation backend while remaining independently testable/service-capable.
- **PCC functionality** is the universal project-control spine exposed through Forge and the repository root bootstrap.
- **Vault** is the governed machine storage/catalog/provenance authority, with shared dependency/cache stores and per-project deduplicated mirrors.
- **Ember and other projects** remain independent managed projects/integrations, not Cortex-owned source trees.

ForgePY/Python PCC remains the production control authority until the Rust Forge lane passes explicit semantic/runtime takeover certification. Do not treat candidate Rust Forge code as production merely because it compiles.

## Repository entry point

Run `PROJECT_CONTROL_CENTER.cmd` from the repository root. It launches the current project-owned PCC GUI/console authority. The native Cortex desktop remains a product/runtime surface; it is not a replacement for the universal Forge/PCC control plane.

## Current certification truth

The implemented `full` source gate performs:

1. quick/root/toolchain/contract checks;
2. mandatory Python/PCC regressions;
3. `cargo fmt --all -- --check`;
4. Cargo workspace check;
5. Cargo workspace tests;
6. Clippy with warnings denied;
7. Cargo workspace build;
8. source-stable, content-addressed Vault mirror;
9. deep CAS verification;
10. GREEN governed-source marker;
11. Vault recovery-point certification tied to the GREEN source fingerprint.

Windows native desktop startup and real provider/chat interaction are separate runtime acceptance evidence until they are safely automated end-to-end. `launch-gui` now requires a visible native window owned by the launched PID instead of treating process survival as UI readiness.

## Vault/storage authority

The PCC uses one machine-level Vault. On the intended Windows workstation the configured primary location is `D:\\CortexLibrary` unless explicitly overridden. Shared package/download caches live once under Vault; Rust build targets are project-namespaced to prevent binary collisions; project mirrors use global SHA-256 CAS deduplication.

Project mirrors intentionally exclude VCS internals, build outputs, dependency trees, caches, PCC operational state, debug/log artifacts, update/handoff residue, root transport ZIPs and link/reparse traversal. Lifecycle cleanup is plan-first and quarantine-first; destructive purge requires explicit approval.

## Read next

- `README_FIRST.md` — concise authority/product hierarchy.
- `docs/QUALITY_GATE_CONTRACT.md` — exact implemented certification boundaries.
- `docs/CURRENT_IMPLEMENTATION_AUDIT_RS293.md` — Rust Forge candidate state.
- `docs/TARGET_ARCHITECTURE.md` — target composition.
- `products/forge-rust/docs/FORGEPY_PARITY_MATRIX.md` — current takeover gap matrix.

Historical standalone/N1/R051 manifests remain provenance evidence only. They do not override this file or `README_FIRST.md` as current project authority.
