# R8I-W2 — Inventory completion + access-gap usability

Source baseline: R8I-W1 `PCC-GUI-0.15.1`, installed on top of the Windows-certified R8H persistent inventory. Incremental repository-root overwrite patch; the application is `PCC-GUI-0.15.2` after installation.

## Changes

- Keep the existing persistent volume index and its SQLite schema **unchanged**. No scanner restart, source mutation, Vault catalog migration, elevation, or automatic rescan.
- Reuse the existing `errors(path, operation, message)` table for `query_access_gaps(...)`: paginated (default 200; maximum 500 per page), literal substring filtering across path, operation, and diagnostic text, volume-ID validation, read-only SQLite connection, and bounded result materialization. Keep existing `list_errors(...)` for compatibility.
- Add a dedicated Access Gaps mode in the Inventory results pane. Search/filter, page controls, per-row operation and diagnostic, and a full selected-error inspector now work for the entire set, rather than showing only the first 30 text records. `All paths` returns to the normal inventory. The existing fast path filters select the regular file view.
- Preserve the selected row across refresh when it remains on the displayed page. The left results pane starts at approximately 61% width and the inventory-specific Treeview uses larger row/heading typography without changing other PCC tables.
- The activity indicator is packed only while the scan is active; completed/paused inventory shows a stable status label and counts. Completion remains explicitly `COMPLETE WITH ACCESS GAPS` when unreadable locations are recorded.
- Guard status readbacks by the scan's authoritative `updated_utc` value and terminal-state ordering so a slower page-query worker cannot overwrite newer progress figures. The inspector's overview is derived from that same accepted status snapshot. The active result selection is not forcibly overwritten by every refresh.
- Preserve R8I-W1 quick, non-recursive Storage Health; do not restore synchronous health walks on the GUI thread.

## Existing database and operation safety

The index remains at `<volume>/.cortex/inventory/<volumeId>/inventory.sqlite3`. No migration is required and the existing finished inventory is immediately readable. `Rebuild Index` is *not* required and should not be used to test this patch.

Source data and `vault_catalog.db` remain untouched by these read-only query/UI changes. The dedicated index itself is only written by an **explicit** Start/Resume/Rebuild scan, as in R8H.

## Windows verification procedure

1. Confirm R8I-W1 is installed, stop/close PCC before overwriting its GUI Python source. Do not delete the inventory database.
2. Extract the patch at the Cortex repository root. Relaunch and check `PCC-GUI-0.15.2`.
3. Inventory should reopen in `COMPLETE WITH ACCESS GAPS` with the previous entry count, zero pending directories, and no moving activity bar.
4. Click `Access Gaps (N)`. Search for `PermissionError` or a known inaccessible path; change pages, select a record, and confirm the inspector shows the full diagnostic. Return via `All paths`.
5. Test `Health > Storage Health`; it must remain nonblocking. Do not run Deep Health simultaneously with a live scan.
6. Run the authoritative PCC FULL QUALITY GATE. Commit/push only on a fresh GREEN/MATCH.

Local Linux regression coverage includes 166 mandatory Python tests (one symlink skip), five provider tests, 27 Vault storage tests, regular/minimum-window Inventory GUI smoke, and concurrent-inventory quick/deep Health GUI smoke. Windows FULL certification and live UX checks remain to be performed locally.
