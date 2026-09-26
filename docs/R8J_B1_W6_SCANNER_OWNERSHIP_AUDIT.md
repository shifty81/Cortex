# Cortex R8J-B1-W6 — Sixth audit and cumulative roll-forward

## Source/prerequisite authority

One repository-root overwrite ZIP based on published R8J-A `82d41230673ef2a179dc6fb2f1918493ad68837e`. It includes the entire W5 payload and supersedes the B1/W1–W5 archives. Visible GUI version: `PCC-GUI-0.15.5f`. The file hashes and R8J-A preimages are machine-readable in `docs/R8J_B_PATCH_MANIFEST.json`. The ZIP contains no existing inventory database, WAL/SHM, Vault catalog, project files outside the declared patch, model data, or source-moving operation.

## Newly reproduced W5 defect

The initial scanner's startup `BEGIN IMMEDIATE` serialized only a brief setup transaction. A second worker starting after that transaction committed saw `scan_state='running'` but did not reject it. The deterministic reproduction blocked worker one at its first `os.scandir` and observed that worker two finished the entire index while worker one was still alive. The same explicit `restart=True` path could delete a live worker's index. W5's recursive-refresh ownership checks did not cover this older scanner.

## W6 correction

- Adds only the auxiliary, one-row `inventory_scan_owner(id,owner_pid,token,updated_utc)` table in the dedicated inventory database when an initial scan/resume is explicitly started. No schema `user_version` bump and no rebuild.
- Reserves SQLite's writer before reading and claiming ownership. A live prior PID prevents both normal resume and explicit restart. A dead PID permits recovery, retaining the original pending queue. On Windows, process liveness continues to use W1's non-signalling handle check.
- Commits the owner-token heartbeat in the same transaction as each scan progress batch; a worker without the token cannot publish progress. Releases its own lease on success, pause/cancellation or ordinary exception, marking unexpected failures as paused rather than forever running.
- `run_scan` against a completed index without restart remains a no-op with respect to the contents and does not claim a lease. New tests exercise two simultaneous threads, explicit restart refusal, cancelled scan resume, dead owner takeover, crash-path cleanup and completed-index stability.

## Local verification/limits

Dedicated W6 scanner-lease tests: 5 passed. Full inventory test discovery: 72 run, one skip. Control test discovery: 284 passed. Nested resolver: 4 passed. Repository-root staging: 5 passed. Provider: 5 passed. These were run against the reconstructed cumulative source on Linux; real Windows FULL/Cargo/PowerShell and the live external-drive DB are NOT certified here. The original 2.24M-row database was not provided.

## Remaining project-wide gaps after W6

1. Real Windows FULL and GUI acceptance of the cumulative W6 archive; verify the existing index via the read-only `CortexInventoryIndexAudit.py` command, not a rebuild.
2. Process-start identity / PID reuse and operational stale-lease recovery UI should be hardened before unattended scheduling. The new table stops concurrent live initial scanners, but a reused PID may conservatively require restarting Cortex. The recursive-refresh lease also remains PID-based.
3. Read-only, inventory-backed adapter connecting the Python SQLite index to Rust project discovery/registry, with volume-relative identities and nested/composite project graph. Do not auto-register or move.
4. Approval-based Organization Review of new/moved/unclassified content with provenance and dry-run.
5. Scheduled incremental change journal, database backup and transaction/recovery coverage; whole-drive reindex is never the default.
6. Live navigation latency, Storage Health, Access Gaps, stop/resume and cross-device G:↔D: acceptance with the existing drive.

Installation: close Cortex; install this ZIP only over published R8J-A, verify `PCC-GUI-0.15.5f`, run a scoped recursive-refresh smoke test in a small project, then Windows FULL. Do not apply W5 first and do not rebuild the index.
