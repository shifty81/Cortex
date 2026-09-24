# Root Project Control bootstrap

`PROJECT_CONTROL_CENTER.cmd` is the standard repository-root doorway into the current Forge/PCC control plane.

## Current behavior

The bootstrap launches the project-owned Python PCC GUI/console authority. That authority provides project status, source gates, transactional patch intake, Git/recovery operations, Vault/shared-dependency controls, debug bundles, Rust Forge candidate routing and Cortex build/runtime commands.

The native `cortex_desktop` executable is a Cortex product/runtime surface launched from the control plane; it is not the universal project-control authority.

## Storage behavior

PCC project commands receive the shared Vault dependency environment. Rust crate/git downloads are shared, Rust target output is project-namespaced, and registered project source can be mirrored into the global content-addressed Vault store. FULL GREEN requires a source-stable, deep-verified mirror before recovery certification.

No external `PCC_HOME` or separate Universal PCC install is required for this repository. Historical standalone PCC implementations are donor/provenance material only.
