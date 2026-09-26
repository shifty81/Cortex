# Cortex R6C — Permanent Runtime-State / Worktree Repair

Based on current-main `crates/cortex_observability/src/lib.rs` blob `a49b6d05745918b12cbf2adc5e060d5a1ac88e38`.

- Moves ProjectObservability JSONL stores to `<state_root>/observability/events/`.
- Adds a Rust regression proving observability cannot create `<project_root>/logs/events/`.
- Runtime-state repair v0.2 restores six tracked legacy `.gitkeep` files from HEAD, migrates legacy events, and returns exit 2 if Git remains dirty.
- Does not overwrite CortexPCC.py, ToolchainBroker, or the current adapter.

After overwrite, run:
`python tools\\control\\PCCRuntimeStateRepair.py --root G:\\Cortex`

Then refresh PCC. If clean, run FULL once. If FULL recreates Modified, capture that status: the known observability producer has been removed and the remaining producer can be isolated independently.
