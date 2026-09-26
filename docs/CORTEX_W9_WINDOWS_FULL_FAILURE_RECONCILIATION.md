# W9 — Windows FULL regression repair and cumulative acceptance

**Evidence:** `Cortex_DebugBundle_20260926-184249_FULL_FAIL.zip`, `G:\Cortex`, Windows 10, Python 3.14.7. The mandatory 196-test PCC set passed. Expanded control discovery ran 287 tests with two failures and one error; Cargo was not reached. No GREEN issued. This is a W8 cumulative replacement, not an incremental on top of W8. Install only W9 over published R8J-A or already-installed W8.

## Isolated causes and corrections

1. `test_r8j_project_wide_audit.test_live_recursive_job_cannot_be_discarded`: a `sqlite3.connect` context manager does not close a connection when its block exits; Windows refuses to delete its open database at temporary-directory cleanup (`WinError 32`). Use `contextlib.closing` around the fixture connection. Production SQLite code already has explicit close authority; do not change live index state.
2. `test_cortex_source_compare.test_02_added_removed_changed_and_sha_evidence`: `Path.write_text()` translates `\n` to CRLF on Windows. The test expected a hardcoded LF byte digest, so the mismatch is a fixture inconsistency, not source-comparison corruption. Write exact LF bytes with `Path.write_bytes()` for the Rust fixture; retain the report's byte-level SHA-256 assertion.
3. `test_pcc_portable_volume.test_legacy_old_drive_registration_is_deduped_against_portable_project`: the test labelled `G:/Cortex` a historical *missing* registration, but `G:\Cortex` is the actual present checkout. The registry deliberately avoids hiding a separate existing root on the strength of identical project names. The fixture now models only the historical path as missing; added separate test proves a real same-name independent root stays visible. No automatic registry file migration or project merge.

## Safety boundaries

- Does not modify `CortexPersistentVolumeInventory.py`, the SQLite live index or its schema, or `vault_catalog.db`.
- Does not change portable project registry runtime logic or create new registrations.
- The patch adds deterministic regression coverage without muting failures or removing control discovery from FULL.
- W8's candidate preview, inventory backup/acceptance utilities, W7 recursive scan ownership and broader audit work all remain in the cumulative archive.
- GUI version remains `PCC-GUI-0.15.6`; successful startup alone does not certify FULL.
- FULL's current GREEN marker does not prove a source-stable, deeply verified Vault recovery mirror. Rust Forge remains separately gated and ForgePY/Python PCC operationally authoritative.

## Verification before Windows acceptance

Local reconstructed-W8 source: control discovery 288 passed; inventory discovery 81 run with one skip; focused 31 tests passed. Package contents, SHA-256 manifest and clean cumulative overlay must be verified before publishing. Windows FULL, Cargo and actual-volume/index acceptance require the user's PC.

## Home test

Close Cortex. Install this **W9 only** at the repository root, overwriting declared files; do not rebuild or reset inventory. Verify `0.15.6`. Run FULL and retain its log. Only after FULL GREEN, exercise project candidate preview, index check, optionally explicit index backup/restore verification, and UI navigation. Commit/push if the source fingerprint matches GREEN. If FULL fails, upload the new debug bundle.
