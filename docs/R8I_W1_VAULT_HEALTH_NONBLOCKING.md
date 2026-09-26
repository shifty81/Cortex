# R8I-W1 — Nonblocking Vault Health recovery

Target: certified R8H + R8H-W1 (`PCC-GUI-0.15.0`), repository-root overwrite.

## Cause and change

The old Health tab invoked `storage_health(root)` directly in the Tk GUI callback. That operation recursively walks shared dependency/build directories and the object store, computes reference/GC and reclaim plans, and can take considerable time on a development volume. The application event loop blocked during that time. `storage_health()` also previously called `ensure_layout()` even though a health query should not initiate layout provisioning/migration.

`Storage Health` now starts a background **quick metadata-only** inspection. It reports disk usage, shared-store presence and whether the current project mirror manifest is present; it deliberately does not claim CAS/GC/reclaim verification. `Deep Health` is an explicit background operation. Retention and GC plans also use the background event queue, with busy guards and Health-tab results/status area. Deep Health/GC plan require the persistent volume index to be paused first. All Tk widget changes occur in the main thread via event dispatch. `storage_health()` no longer calls `ensure_layout()`.

No persistent inventory schema changes. No migration, source file movement, Vault catalog write, or automatic drive rescan. Normal deep checks retain their existing CLI contract.

## Install and verify on Windows

1. Allow inventory Pause to finish, then close **every** Cortex GUI process. If the existing GUI is frozen, close it using Task Manager; do not delete `.cortex/inventory/` or select Rebuild Index.
2. Extract the ZIP into the Cortex repository root (`G:\Cortex` or the current portable drive letter).
3. Relaunch and verify `PCC-GUI-0.15.1`.
4. Start/Resume inventory. Health > Storage Health should remain interactive and display a clearly labeled quick metadata summary. Deep Health and GC Plan should request a paused inventory while scanning.
5. Pause and wait for acknowledgment; Deep Health should run asynchronously without freezing GUI.
6. Run FULL QUALITY GATE and commit/push only after a new source certification.

Known limitation: Deep Health's existing exhaustive implementation may take substantial I/O and memory on very large Vault trees; this hotfix moves it off the UI and makes it explicit, but does not replace its full traversal algorithm. A later pass should implement incremental aggregate metrics and cancellation.

## Local evidence

- Mandatory Python regression group: 164 tests, 0 failures, 1 symlink skip (Linux).
- Additional Vault storage suite: 27 tests, 0 failures.
- Five PCC provider tests: 0 failures.
- Isolated GUI smoke: quick health during active scan, deep guard during scanning, deep worker nonblocking, main-thread event/result display.
- Windows FULL gate and actual-drive GUI behavior remain unverified until user runs them.
