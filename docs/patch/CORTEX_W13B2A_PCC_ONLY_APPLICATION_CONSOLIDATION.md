# Cortex W13B2A — PCC-only application consolidation

This pass makes PCC the only active Cortex application shell while preserving the Rust CLI-first engine.

## Changes

- Normal PCC Chat now reaches the common Rust developer controller (`cortex pcc-chat`) instead of generic no-tool chat.
- Natural-language create/edit requests are no longer pre-routed into Repair, so standalone-project creation and approval can execute.
- The common controller has an embedded open mode that does not claim legacy Desktop runtime ownership.
- When Cortex creates/switches projects, the bridge reports the authoritative workspace and PCC rebinds to it automatically.
- `launch-gui` is retained only as a compatibility alias and now starts the PCC GUI, never `cortex_desktop.exe`.
- The PCC console no longer advertises or launches Cortex Desktop.
- Native Desktop crates remain in source temporarily because the CLI still consumes reusable controller code; they are compatibility internals, not an active application target.
- PCC keeps its current Python conversation files only as a presentation cache in this pass; the common Rust ConversationStore is authoritative for the developer workflow. Full history/registry file-format convergence is deferred until the Hello World coding path is live-certified.

## Acceptance target

After FULL is GREEN, use PCC Chat with:

> Create a new standalone Rust Hello World project named CortexHelloWorld, build it, run it, and verify the actual output.

The common developer workflow may ask for explicit approval before creating the managed project. Reply `yes` in the same PCC chat. Success requires real project files, build evidence, runtime output evidence, registration, and PCC automatically rebinding to the new project.

## Safety

This patch does not delete legacy Desktop source, does not migrate Vault data, and does not alter the W13B1C storage-authority repair.
