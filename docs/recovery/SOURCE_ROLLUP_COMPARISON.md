# CTX-SOURCE02 — cumulative source recovery and comparison

This pass includes the *unchanged* CTX-SOURCE01 exporter and its 12 tests plus an additional read-only comparison tool and 12 comparison tests. It adds files only and does **not** edit Cortex's current project contract, PCC, Rust crates, Git state, patch receipts or source files. It is NOT a Cortex full source rollup and does not repair desktop startup or certify Forge.

## Installation variants (choose exactly one)

- **Cumulative root-drop**: use `Cortex_RootPatch_CTX_SOURCE02_Cumulative_Recovery_20260918.zip` only when `tools/control/CortexSourceRollup.py` has not yet been installed. This delivers both SOURCE01 and SOURCE02 functionality in one transactional operation. Move older SOURCE01 patch ZIPs out of the root first, to avoid competing patch transports.
- **Follow-on root-drop**: if CTX-SOURCE01 is already applied (confirm the receipt in `artifacts/patches/receipts`), use `Cortex_RootPatch_CTX_SOURCE02_Comparator_20260918.zip` instead. It adds only the comparator and new tests/documentation, requires the SOURCE01 receipt, and leaves the exporter untouched.

Both packages are exact new-path patches; fail closed if any destination already exists. **Do not queue both variants or the historical SOURCE01 patch together.** Apply one through PCC with explicit approval.

## Recovery sequence — no source overwrite

1. Run the installed exporter from each independent checkout. Do not delete or merge either source directory:

   `python tools/control/CortexSourceRollup.py create --root "C:\Users\Shifty\Desktop\Cortex-Main"`

   `python tools/control/CortexSourceRollup.py create --root "C:\Users\Shifty\Desktop\Cortex-main"`

   Each checkout needs its own installed exporter; alternatively copy an exported archive from each separately. SOURCE01 lists exclusions and omits `.git` and patch ZIP payloads; separately preserve pending patch transports. Verify each ZIP using `python tools/control/CortexSourceRollup.py verify --archive "<actual-path>.zip"`.

2. Compare **two created source rollup ZIPs**, not a GitHub download or root-drop patch ZIP:

   `python tools/control/CortexSourceCompare.py --left "<older-source-rollup>.zip" --right "<newer-source-rollup>.zip"`

   To get EVERY differing file for exact-source reconciliation: append `--full` and optionally redirect output to a report file outside the project source. Default preview lists at most 30 entries in each category; counts always cover the full verified inventory. `--preview 0` prints metadata and counts only.

3. Review `onlyLeft`, `onlyRight`, `changed`, and `onlyLeftAppliedPatchIds` / `onlyRightAppliedPatchIds`. SHA-256 and byte counts identify source differences. The comparator DOES NOT select the authoritative tree, create a patch, install dependencies, touch Git, write source, or extract either ZIP. Differences return exit 0; invalid source archives/checksum sidecars return exit 1.

## Checks and limitations

Each input is first subjected to SOURCE01's complete ZIP/inventory/content validation. A present `.sha256` sidecar must match the archive SHA-256 and exact basename; missing sidecars are disclosed as `missing`, never silently called verified. Hashes establish evidence integrity and version identity, **not** trust in the party who authored the files. SOURCE01 excludes generated/binary/operational/sensitive-name files, and its inventory explicitly records exclusions. It is not a byte-for-byte machine image. Inspect source text for accidentally committed secrets before sharing.

No Windows build, live desktop smoke or current-local-branch compatibility is claimed. The known September 18 logs show `Cortex-main` has no `.git` and the desktop failed parsing the project contract; source comparison identifies version differences but does not resolve those runtime faults by itself.
