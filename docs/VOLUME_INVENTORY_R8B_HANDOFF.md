# R8B — Safe inventory certification bridge

This additive patch is based on the R8A archive and the September 26 GREEN log. The available complete source rollup is September 23 and **must not** be used to overwrite the newer PCC or GUI.

## Included

- `tools/control/CortexVolumeInventoryGate.py`: optional test-only entry point, uses the active Python executable and repository-relative paths.
- `tests/test_volume_inventory_gate.py`: wrapper tests.

## Run from the active Cortex root

```powershell
python -B tools/control/CortexVolumeInventoryGate.py --root .
python -B -m unittest -v tests/test_volume_inventory_gate.py
```

The first command runs R8A's five temporary-fixture tests. It does not inventory the external drive. Both commands can be run before the existing PCC full gate.

## Boundaries

No changes to existing source files, `project.control.json`, GUI, registry, `vault_catalog.db`, project locations, or the whole-volume inventory script. No automatic inventory scan. The optional bridge is **not yet wired into the authoritative PCC full gate**: that change requires the current September 26 `project.control.json` and PCC gate implementation, rather than substituting September 23 versions. GUI integration and command routing also remain pending current-source review.

The existing R8A scanner retains the `projects/` and top-level `Git/` spelling and outputs JSON to stdout only. If you choose to scan the actual drive later, use the root of the drive currently containing Cortex; do not hard-code G:.
