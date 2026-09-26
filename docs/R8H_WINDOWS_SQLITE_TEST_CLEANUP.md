# R8H Windows SQLite test cleanup correction

## Failure

The R8H mandatory fixture `test_inventory_index_excluded_and_source_links_unfollowed`
used `with sqlite3.connect(index_file) as db:`. The sqlite3 connection context
manager commits/rolls back but **does not close** the connection. On Windows,
`TemporaryDirectory.cleanup()` could therefore fail with WinError 32 while
attempting to delete the test inventory SQLite database.

## Correction

The fixture now uses `contextlib.closing(sqlite3.connect(...))` and explicitly
asserts that the handle cannot be reused after leaving the block. This is a
test-only lifecycle correction; the production scanner already closes its
writable connection in `finally` and its read helpers use `closing`.

## Install and validate

Overlay this ZIP at the Cortex repository root. Run a focused check:

```powershell
python -B -m unittest -v tests.test_persistent_volume_inventory
```

Then run the standard PCC FULL QUALITY GATE. A new GREEN certification is
required before COMMIT + PUSH GREEN. The test does not scan or modify the real
external volume or existing Vault catalog.
