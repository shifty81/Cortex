# Cortex R8J-B1 — Scoped recursive inventory refresh

Prerequisite: published R8J-A / `PCC-GUI-0.15.4` / existing complete persistent volume index. This patch advances to `PCC-GUI-0.15.5f`. It is repository-root overwrite-only and does not import a full source tree.

## Behavior

- Vault / Forge > Inventory: **Refresh Selected Tree**, **Resume Tree Refresh**, **Pause Tree Refresh**, and explicitly confirmed **Discard Paused Queue**.
- The selected indexed directory is the only tree refreshed. Its immediate and nested child directories are reconciled one at a time using R8J-A's SQLite-staged metadata comparison. The GUI worker remains asynchronous and throttles progress events.
- Durable `recursive_refresh` and `recursive_refresh_queue` tables are added lazily **inside the existing dedicated inventory.sqlite3**. No table or content is removed from the existing v1 index; `PRAGMA user_version` remains 1. No rebuild, rescan, imported files, project registration, Vault catalog change, or arbitrary source moving is performed.
- The queue retains remaining directories after a pause or worker death. Each directory's inventory transaction is committed before the corresponding work item is acknowledged. Repeating a directory after an interrupted acknowledgment is safe and idempotent.
- A newly seen directory is traversed by this same targeted job; it does not dispatch the unrelated full-drive scan. Deleted folder descendants and any old queued rows are removed. Reparse points, links and foreign mount boundaries are not followed.
- An inaccessible directory retains its prior indexed children and contributes an access gap. If the selected root disappears, the job pauses rather than silently expanding the operation outside the approved scope. `Discard Paused Queue` requires explicit confirmation, retains all already indexed records, and permits subsequent parent-directory repair.
- Running the initial volume scanner or another single-directory refresh during an unfinished recursive job is rejected to avoid conflicting traversal authorities.

## Scope limitations and review authority

This is the **recursive reconciliation foundation**, not unattended filesystem monitoring or a completed Organization Review. There is no polling/watcher on startup; the user starts this job explicitly. Folder relocation is represented by observed deletion and addition, **not** assumed to be the same project. Source contents are not hashed or read. The cumulative added/changed/deleted counters are operational summaries; if the process dies in the tiny interval between a directory transaction and queue acknowledgment, the directory is safely replayed and totals may undercount that already committed operation. Database entry counts remain authoritative. Review and registration of project/duplicate candidates belong to R8J-B2.

## B1 audit hardening

- Use a non-terminating Windows `OpenProcess` (QUERY_LIMITED_INFORMATION + SYNCHRONIZE) + zero-time `WaitForSingleObject` probe for stale worker ownership. Windows `os.kill(pid, 0)` must never be used: it can send a control event.
- Clear transient `scandir`/`refresh_scandir` error records once that exact directory is enumerated successfully; keep unresolved child stat errors.
- Remove invalidated recursive work-queue descendants transactionally alongside deleted/replaced indexed subtrees, avoiding retries against already-removed paths.
- Dedicated fixtures exercise Windows API outcomes, non-signalling ownership, transient access recovery, and deleted-queue pruning.

## Second audit hardening

- Root-scoped recursive refresh now treats the display label `.` as the canonical empty root path when enqueueing its descendants. Previously a backend invocation against the root could incorrectly claim completion after only one folder.
- The volume-root's transient `scandir`/`refresh_scandir` errors clear when it becomes readable; the root's error key is `.` rather than the empty path used for entry parent identities.
- Recursive job start/resume claims the work queue under an SQLite `BEGIN IMMEDIATE` reservation, preventing two separate processes from claiming a paused queue simultaneously.
- Independent fixtures cover both root cases, while W1 safety coverage remains retained.

## Local verification

Mandatory PCC Python regression suite, dedicated recursive and targeted refresh tests, PCC provider tests, GUI smoke at an isolated volume fixture, syntax checks and ZIP integrity checks performed in this handoff. The user's Windows FULL quality gate and real-volume run are required before publication. Tests operate on temporary directories only.

## Windows acceptance

1. Close Cortex completely; install this ZIP into its existing project root, relaunch, and verify `PCC-GUI-0.15.5f`. Confirm the saved index loads without a new scan. **Do not choose Rebuild Index.**
2. Go to Vault / Forge > Inventory > All paths, select a previously indexed project folder. Choose **Refresh Selected Tree**. It should reconcile descendants and leave other root directories untouched. Start with a reasonably sized project, not the entire `G:\` root.
3. Pause during the refresh and use **Resume Tree Refresh**. Confirm progress continues from its saved queue and no prior results disappear.
4. Open Access Gaps and Storage Health; neither should block the GUI. No changes are made to `vault_catalog.db`, project registration or source files.
5. Run authoritative FULL quality gate and use the user's normal GREEN checkpoint/push workflow after Windows validation. Report a new debug bundle if any gate fails.

## Third audit: PCC and project-wide contract convergence (W3)

- W3 first consolidated the project-wide contract corrections. The **W6** cumulative replacement supersedes B1, W1, W2, W3, W4 and W5 and installs directly over published R8J-A. The current visible GUI version is `PCC-GUI-0.15.5f`.
- `Discard Paused Queue` obtains `BEGIN IMMEDIATE` before examining job ownership/state, preventing a stale decision from deleting a newly resumed worker's queue.
- The FULL gate keeps its fixed mandatory list and now additionally discovers every `test_*.py` in `tools/control/tests` and `tests`, failing closed on an absent/empty suite. Successful negative fixtures are buffered; genuine failures retain their diagnostics. Universal PCC provider tests remain mandatory.
- `Trust Current Checkout` is wired to the existing scoped Git trust action; Git authority preflight verifies the `gitReady` result, not merely a successful JSON response.
- The portable registry keys project identity to the stable volume ID and volume-relative path rather than a mutable `G:`/`D:` mount letter. The historical machine-local registry is read-only compatibility input; reading it does not move, rewrite or auto-register projects.
- Repair actions and other commands that write Git metadata, build artifacts or diagnostics are identified as local mutations in shared PCC command metadata. Gate-failure evidence uses a bounded, non-recursive Vault/storage summary.
- W3 adds seven independent authority regressions to the previously skipped broad control tests. See `docs/R8J_B1_W3_PROJECT_WIDE_AUDIT.md` for scope, test counts, source lineage and remaining Windows-only certification.


## Fourth cumulative audit / W4

W4 carries all W3 corrections, reserves the SQLite writer before scan/refresh authority checks, clears recovered initial-scan and replaced-folder access gaps, and labels recursive completion with access gaps accurately. It adds `CortexInventoryIndexAudit.py` for explicit read-only snapshot/count/quick-check acceptance. See `docs/R8J_B1_W4_CUMULATIVE_AUDIT_AND_GAP_MATRIX.md` for the verified cross-project matrix, limitations and next-stage gaps.

## Fifth audit / W5: FULL gate and completion truth

W5 keeps W4 recursive-refresh code intact. It adds independent root-level staging tests and the otherwise undiscovered nested project-control resolver suite to mandatory FULL. The read-only index auditor must no longer return an overall PASS for a paused/incomplete scan, a nonempty initial-scan queue or an unfinished recursive work queue; its `completed` field distinguishes lifecycle state from independently verified SQLite integrity/counter parity. See `docs/R8J_B1_W5_FINAL_ACCEPTANCE_AND_GAPS.md`.

## Sixth audit / W6: exclusive initial scanner lease

W5 exposed a missing cross-process ownership contract for the original initial scanner. W6 adds a one-row auxiliary `inventory_scan_owner` SQLite table and claims it under `BEGIN IMMEDIATE`, before any explicit restart can delete records. Only a worker holding its unguessable scan token can publish progress/heartbeat batches; the lease is released on completion, cancellation, or ordinary exceptions. A dead owner can be reclaimed without clearing the scan queue. The completed-index no-op does not claim the scanner lease. The existing `entries`, counts, queues and schema version are preserved. See `docs/R8J_B1_W6_SCANNER_OWNERSHIP_AUDIT.md`.

The GUI remains explicit: none of these changes schedules scans, triggers Rebuild Index, modifies source files, or migrates `vault_catalog.db`.
