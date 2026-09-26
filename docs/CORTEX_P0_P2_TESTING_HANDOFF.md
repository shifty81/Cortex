# Cortex P0–P2 cumulative testing handoff — W9 (0.15.6)

Source basis: W9 supersedes W8 and all older B1 patches; published R8J-A `82d41230673ef2a179dc6fb2f1918493ad68837e`, plus **all** W7 cumulative files and this testing addition. Install ONE ZIP. Prior B1/W1–W7 zips are not prerequisites. This document explicitly distinguishes available code from pending system milestones. The first W8 Windows FULL attempt failed its expanded Python control discovery on three test-fixture defects; this W9 repair has local suite verification, but its Windows FULL re-run and live-volume acceptance are still pending.

## Testable in this package

- P0: W7 full-gate truth and broad regression discovery; separate read-only `CortexInventoryIndexAudit.py`; **new** explicit online metadata-index backup and offline integrity rehearsal. No live inventory database or Vault catalog is in the patch. This inventory backup is not a governed-source Vault mirror, and FULL does not certify source recovery.
- P1: W7 recursive incremental refresh, scan ownership and index acceptance; **new** read-only `CortexInventoryProjectReview.py` projects candidate proposal built from the *existing* index, stable `(volumeId, relativeRoot)` identity, nested relationships, same-name review groups, marker-path provenance. It does not open project contents or alter registration.
- P2: GUI `PCC-GUI-0.15.6` exposes **Preview Project Candidates** in Inventory and shows the artifact location/summary without blocking the UI. This is a preview affordance, not yet a full Organization Review queue or unified Rust/Forge GUI migration.

## Explicit boundaries (not delivered / not certified)

- P0: source-stable Vault deep mirror + verified recovery receipt tied to the FULL fingerprint; Windows FULL; provider runtime; real 2.24M-record backup timing and restore rehearsal.
- P1: Rust discovery consuming the JSON preview; approved project registration and organization decisions; `G:`↔`D:` live identity acceptance; automatic scheduled scan, reliable rename identity and full change journal; PID-start-identity validation.
- P2: unified event/operation transcript across native Cortex, Python PCC and Forge; Rust Forge production takeover; compiler/module extraction, stable native resize/DPI and GUI automation; Windows CI, signed installation and updater.

## When back at the Windows development PC

1. Confirm local checkout is published R8J-A and source has no unrelated modifications. Do **not** reset or rebuild inventory.
2. Close Cortex, overwrite with this one ZIP at the Cortex repository root. Confirm `PCC-GUI-0.15.6` and launch without an index rescan.
3. Run normal PCC FULL; this W9 bundle specifically repairs the three failures in Cortex_DebugBundle_20260926-184249_FULL_FAIL.zip (SQLite connection close, deterministic binary fixture bytes, stale-vs-live portable registry fixture). If it fails, retain its debug bundle and do not commit/push.
4. With no scan running, run read-only acceptance: `python tools\control\CortexInventoryIndexAudit.py --cortex-root . --verify-counts --json`. Optionally add `--quick-check` when idle (slow on large indexes).
5. Explicit metadata backup (requires free disk space; can take time): `python tools\control\CortexInventoryBackup.py --cortex-root . --backup --json`. Retain returned path and SHA-256. Rehearse validation with `--verify-copy <path> --expected-sha256 <sha256> --json`. Neither command replaces the live database.
6. In Vault / Forge → Inventory click **Preview Project Candidates**. Check artifact under `artifacts\reviews`; inspect nested-parent references, same-name candidate groups, marker evidence, and count. No source moves/registration should occur.
7. Exercise Projects/Git/Cortex/Access Gaps navigation, selected subtree refresh/pause/resume, Storage Health, close/relaunch; check GUI remains responsive and existing entries persist.
8. Run the separate `forge-rust-gate` only if testing the Rust Forge candidate; a Cortex FULL does not certify Rust Forge parity or authorize takeover. ForgePY/Python PCC remains the active controller.
9. Only after all relevant checks pass, use GREEN commit/push. If a new repository commit has appeared in the meantime, rebase/reconcile instead of overwriting blindly.

## Operating/safety contract

Review proposals remain recommendations for human classification. A marker is evidence of a *candidate*, not proven project type, source equivalence or duplicate. Same-name groups are *not merges*. All project locations stay where they are. No automatic source reads/moves, remote Git operations or project registration. `artifacts/reviews` and `artifacts/inventory-backups` are the only new output locations for these explicit tools.
