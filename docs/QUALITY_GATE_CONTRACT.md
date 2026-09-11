# Cortex / Rust Forge Quality Gate Contract

## Cortex Full Quality

Canonical order:

1. patch/update intake through production authority;
2. root cleanliness/contract sanity;
3. toolchain/workspace/dependency health;
4. format verification;
5. Cargo check/all-target compilation;
6. unit/integration tests;
7. Clippy with warnings denied;
8. full Cortex workspace build;
9. CLI/API/schema fixtures;
10. provider/plugin/tool contracts;
11. transaction/recovery tests;
12. Desktop/runtime smoke;
13. Forge integration fixtures;
14. debug/provenance evidence;
15. structured GREEN/FAIL result.

## Rust Forge candidate gate

Run independently through `forge-rust-gate`:

1. format check;
2. workspace/all-target Cargo check;
3. workspace/all-target tests;
4. Clippy `-D warnings`;
5. `forge-rs` build.

A GREEN candidate gate proves source/build health only. It does not grant takeover.

## RS293 native runtime smoke

After candidate gate, verify read-only behavior first:

- project contract/provider load;
- durable operation start/stop/history;
- machine-state open/save/reopen;
- project/fleet registry;
- Cortex host connection and real conversation/chat;
- active-project switch/rebind;
- patch inspector without mutation;
- Artifact Central inspector without promotion;
- VCS state/history inspection;
- intake classification with auto-apply disabled;
- filesystem watch snapshot/diff;
- toolchain doctor;
- native diagnostics;
- native IDE open/search/edit/undo/redo/conflict check using a disposable test file;
- LSP/DAP frame round-trip tests;
- Ember host inspector;
- release-manifest/update-plan verification;
- takeover matrix remains fail-closed.

Then test mutation paths individually with recovery evidence before running Cortex Full Quality.

## Takeover certification

ForgePY remains production authority until every required takeover check is VERIFIED against side-by-side semantic fixtures for project/fleet discovery, operation cancellation, patch apply/rollback, Artifact Central/lineage, GitHub/Internal Git, diagnostics/recovery, services, Cortex hosting, scheduling, native IDE/platform/distribution and other required workstation behavior.

## Explicit exclusions

Do not reintroduce obsolete migration/separation marker gates. Do not require WebView/Monaco for IDE certification.
