# Cortex R8J-B1-W4 — Cumulative project-wide audit, acceptance and gap matrix

## Authority and install boundary

- **Published prerequisite:** `shifty81/Cortex` `main` at `82d41230673ef2a179dc6fb2f1918493ad68837e` (R8J-A), checked on 2026-09-26. This package is one **repository-root overwrite** containing the full R8J-B1-W3 payload plus W4 corrections. The original B1, W1, W2 and W3 archives are superseded for installation; do not chain them. The patch manifest enumerates governed files and exact hashes.
- **Source used for this audit:** supplied `Cortex (2).zip` source plus R8A → R8J-A patch lineage, overlaid with the exact W3 cumulative payload and then W4 changes. Every W3 manifest replacement checksum was matched in the reconstructed working tree. This does not claim the reconstructed older full-source ZIP is byte-identical to every unchanged GitHub file.
- **Preservation:** no `.cortex/inventory/**`, `vault_catalog.db`, registry databases, user project sources, third-party archives, models, build output or Git internals are included. The existing 2,238,746-entry index is not rebuilt or imported. No automated source moves, project merges or registrations.
- **Visible GUI version:** `PCC-GUI-0.15.5d`. The current dedicated SQLite index stays at schema version 1; `recursive_refresh` and `recursive_refresh_queue` remain additive/lazy.

## What W3 carries forward unchanged

The full W2 scoped recursive refresh, non-signalling Windows owner check, deleted subtree queue pruning, root-recursion correction, durable pause/resume, stale access-gap cleanup, atomic job claim and discard guard. The W3 PCC-wide convergence is retained: full Python discovery in FULL, stable volume-relative Python registry identity, Git trust command/`gitReady` gate, more accurate command risk metadata and bounded gate-failure evidence. See `docs/R8J_B1_W3_PROJECT_WIDE_AUDIT.md` for that earlier pass; W4 replaces its ZIP, not its historical evidence.

## W4 corrections and certification tooling

1. **Startup authority reservation:** `run_scan` and single-folder `refresh_directory` now reserve an SQLite writer transaction before inspecting the existing recursive job/scan state. This avoids acting on a stale authority read between preflight and modification. This is not a claim that an entire long-running scanner is protected by a durable global process lease; that remains a follow-up.
2. **Recovered errors:** an initial/resumed scanner successfully enumerating a directory clears the previous `scandir` gap for that same directory. A directory replaced by a file/link clears its former host-level `scandir`/`refresh_scandir` gaps; genuinely unresolved child stat errors remain intact. All reconciliation is against the dedicated inventory index, never user files.
3. **Truthful completion:** the recursive-refresh GUI displays **complete with access gaps** in warning color when a job finished its queue but encountered inaccessible locations. It does not convert those locations into successful reads.
4. **Read-only acceptance tool:** `tools/control/CortexInventoryIndexAudit.py` inspects an **existing** marked-volume index without initializing, rescanning or migrating it. The fast view reads persisted status; explicit `--verify-counts` compares actual rows with denormalized counters in one SQLite snapshot; optional `--quick-check` invokes SQLite's own integrity check. Errors and mismatched IDs fail closed. Deep options can be expensive for a multi-million-entry index and must be run when the GUI is not busy, not on its UI thread.
5. **Regression coverage:** new temporary-volume fixtures cover recovered initial scan errors, folder-type conversion, read-only report/missing-index behavior, detectable counter drift and authority lock ordering. Existing tests still cover Windows process API behavior through non-signalling mocks; live Windows behavior remains uncertified until the user's FULL gate.

Read-only index audit on the user's machine, only when desired:

```powershell
python -B tools/control/CortexInventoryIndexAudit.py --cortex-root . --verify-counts --json
# Optional deeper physical SQLite verification (can take time):
python -B tools/control/CortexInventoryIndexAudit.py --cortex-root . --verify-counts --quick-check --json
```

Run from the existing Cortex checkout. `--cortex-root .` resolves the actual volume marker and volume-relative index path, regardless of whether the external disk is `G:` or `D:`. No audit command calls rebuild or refresh. The snapshot report is diagnostic evidence; it does not change stored counts to make them match.

## Project-wide audit evidence and scope boundary

| Surface | Observed source/test state | Evidence limit or next check |
| --- | --- | --- |
| Python/PCC | 65 inventory-discovery tests (1 skipped), 284 control-discovery tests, 5 PCC provider tests: all passing in Linux fixtures; original fixed mandatory gate also retained | Run real Windows FULL after install, including native PowerShell AST checks and application startup |
| Rust workspace | Root manifest enumerates 49 resolvable members; source contains `cortex_registry`, `cortex_project_registry`, `cortex_discovery`, `cortex_review`, `cortex_vault`, `cortex_desktop_native` | Static Cargo TOML parsing is not Cargo fmt/check/test/clippy/build; run these through PCC FULL on Windows |
| Syntax/contracts | Python AST, JSON parse and Cargo manifest parse performed over the reconstructed source | Parsing does not prove runtime behavior or module contract parity |
| GUI | Headless Tk launch exercised Vault / Forge sections at 1280×860 and 1040×720 | Windows native window behavior, high-DPI text/panel usage and live 2.24M SQLite latency remain runtime acceptance |
| Index | Python `CortexPersistentVolumeInventory.py` owns the persistent metadata index; no fixed entry cap, paged UI, scoped refresh | Main drive's actual index bytes were not supplied to this chat and were not scanned by this audit |
| Rust discovery | `crates/cortex_discovery/src/lib.rs` has separate bounded `ignore::WalkBuilder` traversal/classification, not a verified SQLite-index consumer | Do not claim the index has already onboarded/classified every project or replaced Rust discovery |
| Registries | Python portable `ProjectRegistry`, Rust `cortex_registry` and `cortex_project_registry` each have their own existing roles | Cross-language identity, alias reconciliation and one authoritative project graph need explicit adapter tests |
| Repo hygiene | Original supplied full-source archive includes `PCCSurfaceCommon.py.fixpayload`; W4 does not remove it | Inspect Git tracking and actual checkout before declaring it generated/obsolete |

## Remaining gaps, staged without source reorganization

### P0 — before the next major feature (R8J-B2)

- **Windows acceptance of W4:** verify saved index opens without another scan; small `projects/<project>` recursive refresh; pause, resume, Access Gaps and Storage Health; fast audit; optional explicit count audit; PCC FULL followed by the user's existing GREEN publication workflow. Capture a debug bundle on a failure. Linux tests cannot certify the native runtime.
- **Inventory-backed project candidate adapter:** read the existing SQLite metadata as observations, feed the established Rust/PCC project-marker classifiers, and produce stable `{volumeId, relativeRoot, markerEvidence}` candidates. Keep source files unchanged; treat ordinary nested source modules as modules, but recognize true nested projects and project-family edges. Bind to the canonical registry authority rather than creating a competing project DB.
- **Review/approval and evidence:** Organization Review should show *proposed* new/moved/renamed/unclassified/duplicate candidates with evidence and confidence, allow reject/defer/approve, and journal approvals. No silent move, overwrite, merge, registration or Git remote change. A deleted path plus newly added path is only a relocation **candidate**, not a proved identity.

### P1 — operational reliability and UX

- **Global multi-process worker ownership:** current recursive claim/discard checks are transactional and W4 protects scan/refresh startup checks, but the long-running initial scanner and external operations need a durable lease/heartbeat and single-writer policy across Cortex processes, including crash/PID-reuse handling.
- **Delta journal/scheduler:** record per-directory change observations and resumable verification batches. Add explicitly controlled startup/incremental refresh and watcher-plus-periodic reconciliation, while distinguishing unavailable volume, access denial, aborted scan and actual deletion. Avoid repeated full-volume scans.
- **Recovery/backup:** use a governed SQLite online backup/recovery workflow (including WAL-consistent copies) before any future index migration or repair. Verify index counter parity and query plans against the actual multi-million-row database.
- **GUI normalization:** project-aware tree navigation, better pane sizing and DPI/text scaling, access-error grouping, and lightweight status snapshots. Keep Storage Health and deep verification off the Tk UI thread; surface backend job identity/heartbeat and explicit command-vs-chat execution state.
- **Registry portability parity:** test `G:` ↔ `D:` across Python and Rust attachments with two independent copies and nested projects; prevent duplicate registrations without guessing whether actual duplicate source trees should merge.

### P2 — ecosystem functionality after review authority is established

- Per-project persistent context/knowledge graph from *approved* project boundaries; incremental ingestion and source/content indexing must respect authorization and provenance. Metadata-only drive inventory is **not** permission to read or hash every content file indiscriminately.
- Vault artifact/asset lineage, optional duplicate-content verification and recovery snapshots across project families; expose a unified nonduplicated Forge/PCC activity timeline and command catalogue.
- Standardized portable/installed release/update flows only after the existing ForgePY operational authority and Rust takeover parity are explicitly certified.

## Windows acceptance checklist

1. Close the GUI; extract **only** this W4 cumulative ZIP into the current published R8J-A Cortex project root, overwriting declared project files. No older B1 ZIP is needed.
2. Relaunch `PCC-GUI-0.15.5d`; verify the saved inventory is loaded with prior counts and no automatic scan. Do not use Rebuild Index.
3. Test one small project tree's refresh, pause and resume. Open Access Gaps and Storage Health during normal navigation; the application must stay responsive.
4. Run `python -B tools/control/CortexInventoryIndexAudit.py --cortex-root . --verify-counts --json` when no scan is active. Preserve any discrepancy as a diagnostic; do not automatically reset counters. `--quick-check` is optional and heavier.
5. Run the authoritative Windows FULL QUALITY GATE, inspect the true terminal result/debug bundle, and publish only on GREEN. Do not mistake deliberately failing patch-preflight unit fixtures for a real patch-intake error.

This package is source/test/packaging validated in an isolated Linux reconstruction, **not Windows FULL certified** and not claimed to have run against the user's actual inventory database.
