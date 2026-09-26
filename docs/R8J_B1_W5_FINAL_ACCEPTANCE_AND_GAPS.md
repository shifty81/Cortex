# Cortex R8J-B1-W5 — Cumulative safety and acceptance audit

## Installation authority

One repository-root overwrite ZIP directly over the published R8J-A checkpoint at GitHub `82d41230673ef2a179dc6fb2f1918493ad68837e`. Supersedes B1, W1, W2, W3 and W4 as **installation packages**, carrying all W4 files and all W5 changes. Visible version: `PCC-GUI-0.15.5e`. Do not apply previous B1 packages first. Manifest hashes verify every declared payload.

This ZIP includes no runtime SQLite index, WAL/SHM, `vault_catalog.db`, registry/state database, project source relocation, cache cleanup, or model/asset payload. The completed 2,238,746-entry baseline belongs to the live external volume and was not supplied to this isolated test container; no claims of live-volume verification are made.

## Fifth independent audit findings and corrections

1. **FULL coverage:** the principal `tests/` and `tools/control/tests/` discovery does not transitively include the separate `tests/control/test_project_control_resolver_r6.py` (four tests). The repository-root `test_stage_candidate.py` (five tests) also requires its own discovery target. Both are mandatory FULL stages, with an exact staging pattern at root. Both suites passed independently before inclusion.
2. **Read-only index acceptance:** W4's auditor could call a structurally valid but paused/incomplete inventory `passed=true`, because it checked schema/counters/SQLite health but not scan/recursive job lifecycle. W5 exposes an explicit `completed` flag and requires scan state `complete`, zero queued scan directories and no unfinished recursive refresh to return overall `passed=true`. A paused index still returns full diagnostics, including separately evaluated count parity and SQLite quick-check. This changes only the **diagnostic acceptance utility**, never stored inventory metadata.
3. **Regression cases:** add paused scan with physically correct counters and paused recursive job with apparently completed scan state. Both must fail acceptance rather than mislead the operator.

## Retained W4 source and outstanding limits

The recursive refresh logic, non-signalling Windows PID check, protected queue claims/discard, scoped deletion reconciliation, partial access-gap semantics, Git trust, portable registry IDs and PCC risk metadata remain inherited from W4. This pass does **not** implement an unattended watcher, a durable global scanner lease, database backups/recovery, project candidate adapter or Organization Review.

### Before R8J-B2

- Install W5 only over the R8J-A checkpoint and run Windows FULL/GUI acceptance. The additional two Python suites are now gate-mandatory, but the Windows-specific PowerShell/Cargo/native GUI results cannot be certified here.
- Validate the read-only audit against the live index when no scan/refresh is active. With a verified completed inventory the `completed` and `passed` fields should be true; access gaps remain reported. Do not rebuild to force a pass.
- Treat **cross-process initial-scan ownership** as a high-priority safety follow-up before unattended scheduling. The startup authority read is transactional but the entire scanner still lacks an exclusive durable lease/heartbeat across multiple Cortex processes. Avoid starting scans of one volume from two Cortex windows.
- Build one read-only adapter from the existing volume index to the Rust discovery/project graph; submit proposals for approval without automatic moving/merging/registering.
- Continue to certify G: ↔ D: portable identity with real Windows installations, nested projects and independent copies.
- Test responsive GUI navigation, Access Gaps/Storage Health, pause/resume, and actual 2.24M-row query latency on the user's device.

## Read-only acceptance command

From the existing Cortex repo root, after closing or pausing active inventory operations:

```powershell
python -B tools/control/CortexInventoryIndexAudit.py --cortex-root . --verify-counts --json
```

Optional `--quick-check` is heavier. These commands do not call scan/rebuild, cannot correct counters automatically, and do not modify the source files or Vault catalog. The live user's database is not part of the patch.
