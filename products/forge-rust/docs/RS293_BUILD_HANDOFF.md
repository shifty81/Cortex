# RS293 Home Build / Acceptance Handoff

## Apply

Apply only the RS04–RS293 cumulative `.patch` through ForgePY while the Cortex repository still matches RS03 GREEN `6d35ceea75dfa00bce392239fbce7a30eb2d9cc5`.

## Gate order

1. `forge-rust-gate`
2. repair first concrete format/compile/test/Clippy blocker only
3. rerun `forge-rust-gate` until GREEN
4. `forge-rust-build`
5. `forge-rust-run`
6. read-only smoke
7. targeted mutation/recovery smoke
8. Cortex Full Quality Gate
9. commit/push/snapshot GREEN only after both lanes pass

## Read-only smoke

Verify:

- Projects/fleet and active project;
- durable operation history;
- Cortex tab conversations/chat/Inspect/Plan;
- native IDE opens without a WebView dependency;
- IDE file browser/filter;
- multi-tab open/switch;
- edit + undo + redo on disposable source fixture;
- find results;
- language-tool availability display;
- external disk change produces conflict truth;
- protocol framing tests pass;
- toolchain doctor;
- native diagnostics;
- patch inspector;
- Artifact Central inspector;
- VCS state/history;
- intake classification/watch snapshot;
- Ember host inspector;
- takeover status remains fail-closed.

## Mutation smoke after read-only GREEN

Use disposable/test data first for:

- native IDE guarded save;
- native patch apply/rollback;
- Artifact Central promotion;
- Internal Git snapshot;
- branch create/switch and fast-forward pull as appropriate;
- scheduler execution;
- service start/stop;
- release recovery checkpoint and explicit restore.

## Expected first-risk files

If the candidate does not compile, repair in this order without changing architecture:

1. `products/forge-rust/apps/forge-rs/src/main_forge_host.rs`
2. `products/forge-rust/crates/forge-ide`
3. `products/forge-rust/crates/forge-protocol`
4. newly added watcher/toolchain/diagnostic/Ember crates
5. hardened VCS/scheduler/release code

Do not work around a compiler error by restoring Monaco/WebView or copying Cortex functionality into Forge.
