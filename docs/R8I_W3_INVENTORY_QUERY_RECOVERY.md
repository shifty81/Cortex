# Cortex R8I-W3 — Inventory query responsiveness

**Prerequisite:** R8I-W2 (PCC-GUI-0.15.2) installed and its existing persistent volume index.
Close all Cortex PCC windows, then extract this incremental ZIP at the Cortex repository root.
Overwrite only the declared source/test paths. Relaunch and verify `PCC-GUI-0.15.3`.

## Defect and repair

- R8I-W2 quick-filter buttons used leading-wildcard substring matching over the 2.24M-entry index. An exact match-count query had to finish before a page could appear.
- The GUI serialized database-page reads; subsequent button presses could queue behind an obsolete expensive query, including Access Gaps.
- Projects, Git and Cortex shortcuts now use a case-folded prefix range on the existing `idx_inventory_fold` B-tree. The Cortex shortcut is directory-scoped (`Cortex/`) rather than matching unrelated names beginning with Cortex.
- Explicit **Filter** retains literal substring matching. Its interactive page fetch can omit an exact full-index count, use one extra row to determine whether another page exists, and show an exact total upon reaching the last page.
- The SQLite progress handler cancels a superseded request; broad searches are bounded to **8 seconds** of SQL execution and yield an actionable error instead of leaving the UI loading indefinitely. Read-lock waiting is bounded to 5 seconds.
- Existing R8I-W2 protections against stale status snapshots remain intact. Access Gaps continues to use its dedicated paged error query.
- No schema migration, database rebuild, source move, Git mutation, project registration, or existing Vault catalog write.

## Verification in the isolated source fixture

- Mandatory PCC/Python group: **169 tests, zero failures, one skip**.
- Five PCC provider tests: passed.
- Focused inventory/backend and GUI-contract tests: **20 passed**. Includes indexed prefix plan, obsolete-query cancellation, count-free substring pagination, and Access Gaps availability.
- Tk GUI smoke under a virtual display: all five Vault sections opened with GUI 0.15.3.
- Python syntax and incremental ZIP integrity: passed.
- The user's **Windows FULL QUALITY GATE** and actual 2.24M-entry query latency remain to be verified; no production-drive scan was run here.

## Windows acceptance

1. Close Cortex PCC completely; extract this ZIP into the existing Cortex root; reopen and confirm `PCC-GUI-0.15.3`.
2. Go to Vault / Forge > Inventory. The saved 2,238,746-entry index should load without a rescan. **Do not click Rebuild Index**.
3. Click Projects, Git, Cortex and Access Gaps (278) in quick succession; the latest request should replace earlier work instead of waiting through all obsolete searches.
4. Use Filter with a narrow substring and try normal paging. A very broad/absent query must produce an actionable timeout rather than an indefinitely spinning status.
5. Run FULL QUALITY GATE. Commit/push only after GREEN.

The base SHA-256 and replacement SHA-256 of each overwritten file are in `R8I_W3_PATCH_MANIFEST.json`.
