# Forge Rust — Parallel Application Lane

Status: **FR01 scaffold**.

This is the parallel Rust implementation of Forge. It lives inside the Cortex repository during the migration lane but is deliberately isolated in its own Cargo workspace so it can build without destabilizing the current Cortex workspace.

ForgePY remains the production universal front end until Rust Forge passes takeover certification. The Rust application consumes the same `forge.project.v1` project contract and project-owned machine provider used by ForgePY.

## Build through Cortex / ForgePY

ForgePY -> Cortex project provider -> `forge-rust-gate` / `forge-rust-build` / `forge-rust-run` -> this workspace.

Direct developer commands are also available:

```bat
cargo check --manifest-path products\forge-rust\Cargo.toml --workspace --all-targets
cargo test --manifest-path products\forge-rust\Cargo.toml --workspace --all-targets
cargo clippy --manifest-path products\forge-rust\Cargo.toml --workspace --all-targets -- -D warnings
cargo build --manifest-path products\forge-rust\Cargo.toml -p forge-rs
cargo run --manifest-path products\forge-rust\Cargo.toml -p forge-rs -- --root C:\path\to\project
```

FR01 intentionally implements only the first trustworthy vertical slice: project-contract loading, project-native provider dispatch, native Rust shell, quick actions, structured live console, and project-health presentation. No GUI control claims functionality that is not wired.
