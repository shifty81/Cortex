# Cortex R8J-A — Targeted, non-destructive inventory refresh

Baseline: R8I-W3, Windows FULL GREEN, commit `7041070`, GUI `PCC-GUI-0.15.3`.
This patch advances to `PCC-GUI-0.15.4`. It does **not** run a scan on installation,
rebuild the existing 2,238,746-entry inventory, migrate SQLite, touch
`vault_catalog.db`, register projects, move source, or operate on Git repositories.

## What is delivered

- Vault / Forge > Inventory: **Refresh Top Level** and **Refresh Selected Folder**.
- Both operations are explicit background jobs, permitted only after the initial
  full inventory is complete and while no other Vault operation is running.
- Refresh Selected Folder enumerates **only that selected folder's immediate
  children**, staging names/metadata in a connection-local SQLite TEMP table.
  It identifies added, changed and removed entries and reconciles deleted directory
  descendants, pending work and access errors transactionally.
- Refresh Top Level rereads the current volume root, not the entire drive tree.
  Existing child folders are not traversed recursively in this pass.
- Newly discovered directories are added to the existing **durable queue**, and
  the index becomes `paused/resumable`; click **Start / Resume Index** to enumerate
  those new branches. This does **not** rebuild the whole volume.
- If a folder cannot be opened, previous indexed entries are retained and an
  `refresh_scandir` access gap is recorded. Cancellation or unexpected exceptions
  roll back the targeted update rather than deleting partial source metadata.
- The standard GREEN commit message now defaults to a neutral current checkpoint
  rather than reusing the title of an unrelated historical R17D patch receipt.

## Important boundaries

This is **R8J-A**, not a watcher or a finished drive-wide delta crawler.
It detects changes only in the directory explicitly refreshed. To reconcile
changes *inside an existing child directory*, select and refresh that child too.
Removal of a whole folder is discovered by refreshing its **parent**. A moved
folder may appear as a removal and an addition; no rename identity is asserted.
Nothing runs automatically on startup. The stored volume ID, not `G:` or `D:`,
remains the index authority. Changes inside inaccessible locations remain unknown.

## Validation in the isolated source fixture

- New seven-case incremental-refresh tests cover changes, removed subtrees, newly
  queued folders and resumption, cancellation, unreadable folders, unsafe paths,
  unchanged Vault catalog, schema v1, link-replacement safety and refusal to collide with a running scan.
- Mandatory Python PCC group: 177 tests, 0 failures, 1 skipped (host symlinks).
- PCC provider group: 5 passed.
- Native Tk GUI smoke: selected folder and top-level refresh completed without
  blocking GUI event pumping; a newly added file appeared in the database.
- Python syntax and packaged ZIP integrity verified.
- The user must perform the Windows FULL QUALITY GATE and actual volume refresh
  before committing and pushing.

## Windows acceptance

1. Close Cortex, extract this ZIP at the existing Cortex root and reopen.
   Verify GUI `PCC-GUI-0.15.4` and the **saved** inventory counters appear.
2. **Do not click Rebuild Index.** Use Refresh Top Level to check the 22 root
   entries, including `projects`, `Git` and `Cortex`.
3. Select a normal directory and choose Refresh Selected Folder. New/deleted
   immediate children should reconcile with a summary; the UI must remain usable.
4. If a newly created directory is discovered, it becomes pending. Start / Resume
   only to enumerate queued new directories, without clearing old records.
5. Verify Access Gaps remains available and Health / Storage Health stays
   responsive. Run the full PCC quality gate, then commit/push only if GREEN.
