# FR01 — Parallel Rust Forge Vertical Slice

FR01 is deliberately small enough to build and verify quickly while establishing the correct long-term boundary.

## Implemented in source

- isolated Rust Cargo workspace under `products/forge-rust`;
- permissive/open-source `egui` / `eframe` native desktop shell;
- `forge.project.v1` parsing;
- project-native machine-provider resolution before direct command fallback;
- quick action dispatch for Full Gate, Build, Run, Debug Bundle and certified commit/push;
- asynchronous process execution and live stdout/stderr event stream;
- semantic console status coloring;
- ForgePY-like left workspace rail, project-operation area, console, and right health rail;
- explicit Cortex bridge crate with no fake model/chat implementation;
- unit tests for contract/provider resolution.

## Explicitly deferred, not faked

- project fleet registry / D-drive scan;
- patch intake engine and downloads watcher;
- Artifact Central browser;
- Forge Internal Git;
- GitHub workspace;
- tray integration;
- Vault catalog;
- Monaco/Wry IDE;
- Cortex chat/runtime and managed llama.cpp;
- durable job queue;
- native drag/drop;
- settings persistence;
- full ForgePY F61-F80 parity.

ForgePY remains the production authority until these are implemented and the takeover matrix is fully GREEN.
