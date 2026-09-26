# R8D inventory safety tests (additive)

This patch extends `tests/test_volume_inventory.py` only. It does not change the certified PCC, the inventory implementation, or the GUI. It verifies nested hidden paths, no writes, truncation, invalid limits, and Windows junction non-traversal (where supported).

Run from the Cortex root:

```powershell
python -B tools/control/CortexVolumeInventoryGate.py --root .
```

This is a fixture-only test run; it does not scan the external drive. The R8D tests are not yet wired into the authoritative FULL gate because the current September 26 gate source has not been supplied. Do not overwrite newer gate files with the September 23 source rollup.
