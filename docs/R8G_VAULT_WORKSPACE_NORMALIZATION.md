# R8G — Vault / Forge workspace normalization

## Scope

This is an incremental overwrite patch on the R8E/R8F baseline. GUI version: `PCC-GUI-0.14.0`.

The old vertically stacked Vault command sheet is separated into five first-class subtabs:
Inventory, Catalog, Intake, Storage and Health. The Inventory view is the initial tab.
The existing catalog tree, project scan/baseline commands, intake audit, mirror/storage
and lifecycle actions remain present under their respective tabs; no new GUI stack is added.

The Inventory tab has a persistent root and read-only indicator, the live scanned-entry
count, per-type counts, the exact 2,000,000-entry cap, an explicit Cancel action,
resizable results/details panes, a dark scrolling Treeview, a bounded (200 row)
path preview, quick filters and safe selected-path Copy/Reveal actions.

**The progress bar represents consumption of the entry cap, NOT percent of the drive
scanned.** On cap exhaustion the scan is PARTIAL, never reported as complete. Existing
in-memory scan results are shown after completion or cancellation, not during the scan.
Path searching and summary rendering happen in a worker with generation checks to
avoid blocking Tk or showing stale results. Progress callbacks are throttled.

The scanner and its 2M entry in-memory architecture are deliberately not replaced in
this UI pass. The long-term full-volume index needs a dedicated paged/streaming storage
implementation with resumability and memory bounds, reviewed separately. R8G does not
claim a complete volume index if the cap was reached, and does not persist a scan.

No drive files are moved, renamed, hashed, imported or deleted. No project registrations,
registry edits, or Vault catalog DB writes happen as a result of inventory.

## Installation

1. Allow any active R8F scan to finish, or cancel to obtain a partial report; close
   the running PCC GUI before overwriting Python files. Refresh does not reload code.
2. Extract this ZIP directly into the `Cortex` repository root (no outer folder).
3. Relaunch the PCC GUI; verify the version reads `PCC-GUI-0.14.0` and select Vault / Forge.
4. Run `python -B tools/control/CortexVolumeInventoryGate.py --root .` and the normal
   PCC FULL QUALITY GATE. Do not commit/push until the new source is GREEN/MATCH.

## Validation

- Mandatory 157-test Python PCC/inventory group passed in Linux; 1 Windows junction
  test skipped there (previous R8D Windows fixture was reported passing by the operator).
- Universal provider tests: 5 passed.
- Virtual-display GUI smoke: all five sections switch correctly; Inventory result
  pane renders; asynchronous filtered results, selection and Copy Path tested.
- Windows full quality gate and native GUI inspection remain operator-side checks.

## Files

- `tools/control/CortexPCCGui.py`: layout and inventory GUI interaction.
- `tools/control/CortexVolumeInventoryPresentation.py`: bounded read-only row selection.
- `tools/control/CortexPCC.py`: make new workspace tests mandatory in FULL gate.
- `tests/test_volume_inventory_workspace.py`: view behavior and source wiring.
- Existing GUI-version assertions and inventory gate test updated.
