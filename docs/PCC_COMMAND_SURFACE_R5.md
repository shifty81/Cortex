# PCC Command Surface R5

## Purpose

Restore the interactive project-console behavior that existed in ForgePY and eliminate the split between hard-coded GUI buttons, project-contract commands, and the project-native PCC provider.

## Production authority

`PCC-GUI-0.11.0` is the production-side command surface while Rust Forge remains a takeover candidate.

The GUI now builds one discovered command catalog from:

1. `project.control.json` command descriptors; and
2. the active Cortex PCC provider command parser, inspected through Python AST without importing or executing project code.

For the current Cortex contract this exposes 83 operations across build, diagnostics, gate, project, run, source-control, storage, test, tooling, and updates categories.

## Project Console composer

The persistent Project Console now has a bottom command composer with Send, Enter-to-send, Up/Down history, Stop, Clear, and a compact/verbose output toggle.

Routing:

- registered command key or exact label -> project command authority;
- `/cortex <prompt>` or natural language -> Cortex chat CLI;
- `/inspect`, `/plan`, `/apply`, `/repair` -> matching Cortex agent lane;
- `!<command>` or `shell <command>` -> user-directed shell command in the active project root;
- `/history`, `/stop`, `/clear`, `/help` -> local console controls.

Project-contract commands run directly from their declared program/arguments through the same hidden process host/shared dependency environment. Provider commands remain routed through the project-native PCC provider.

## GUI command coverage

The Project Workspace now includes Build & Run, Updates, Source Control, Diagnostics, Storage & Vault, Tooling, and Command Registry surfaces. Category surfaces are generated from the command catalog rather than maintaining independent hard-coded command lists.

The Command Registry is searchable/filterable, shows category/key/label/risk/source/program, supports Run Selected and double-click execution, and refreshes when the active project changes.

## Console log normalization

Durable provider/command logs remain complete. The Project Console is compact by default:

- ANSI color escape sequences are removed from GUI text;
- individual successful unittest case lines are collapsed;
- machine-only `PCC_RESULT_JSON` payloads are hidden from the compact console;
- rustfmt diffs collapse to one explanatory line while the full diff remains available in the expanded log and command-output artifact;
- Verbose restores the full live stream.

## Rust Forge parity

RS293 Rust Forge already has live project-console output but still lacks the equivalent command composer. Its parity matrix now tracks this explicitly. The Rust implementation should mirror this contract only after Windows rustfmt/check/test/clippy/build verification, preventing an unverified Rust UI edit from re-blocking the current full gate.
