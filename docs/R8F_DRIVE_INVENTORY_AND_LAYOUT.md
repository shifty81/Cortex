# Cortex R8F — Read-only drive inventory and tracked layout preservation

Baseline: September 26 R8E GREEN commit `c3d62a7`. This ZIP is an incremental repository-root overwrite; it does not ship/replace `CortexPCC.py`, `PCCOperationHost.py`, Git authority, the Vault database or the Rust crates.

## GUI
Open **Vault / Forge → External Drive Inventory · Read Only → Inventory Current Drive**. The drive is derived at runtime from the active project's resolved path anchor (`G:\`, `D:\`, etc.); no drive letter is hardcoded or stored. The scan is opt-in, asynchronous, metadata-only and in memory. It does not import/register projects or update the Vault catalog. Search the scan results by relative path using **Filter Paths**. Empty search shows top-level directories; result display is limited to 100 rows per filter. Click **Cancel Inventory** to stop early; partial/cancelled state remains visible. An upper bound of 2,000,000 entries prevents unlimited GUI memory consumption; a bounded result is explicitly labelled partial, not complete. Access failures are listed and are not silently certified as complete.

This is an initial visual discovery surface, not a persistent whole-volume catalog. A future streaming/indexing pass should add durable snapshots and search without loading millions of records in Tk memory, with operator opt-in and separate retention policy.

## Convergence fix
`CortexRepositoryConvergence.py --root <Cortex>`, without `--apply`, remains a non-mutating plan. Explicit `--apply` moves non-tracked runtime files by verified copy, refuses differing destination contents, and deletes their source only after all scheduled copies verify. Tracked paths, including the six log `.gitkeep` files, are preserved. The layout validator permits the six declared placeholders while still rejecting actual runtime logs left in-repo. Source migrations and archive globs now exclude tracked files. No convergence command runs as part of an inventory scan.

## Windows validation
After extracting into the installed Cortex root, run the mandatory test runner and then the normal PCC FULL gate before attempting Commit + Push GREEN. The complete FULL gate and native GUI were not exercised on this Linux build host. If gate returns RED, attach its current debug bundle instead of bypassing source-fingerprint enforcement.

```powershell
cd G:\Cortex
python -B tools/control/CortexVolumeInventoryGate.py --root .
```

## Patch contents
- config/cortex/repository_layout.v1.json
- tools/control/CortexRepositoryConvergence.py
- tools/control/CortexRepositoryLayout.py
- tools/control/CortexVolumeInventory.py
- tools/control/CortexVolumeInventoryPresentation.py (new)
- tools/control/CortexPCCGui.py
- tests/test_volume_inventory.py
- tests/test_volume_inventory_pcc_integration.py
- docs/R8F_DRIVE_INVENTORY_AND_LAYOUT.md

No overwrite of the published R8E gate integration. R8F tests are added to the existing mandatory R8E test files, so missing or failing fixtures block the FULL gate.
