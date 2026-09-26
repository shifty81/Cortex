# Cortex R8H — Persistent Volume Inventory & Vault Workspace

## Scope and upgrade order

R8H is cumulative **from the previously installed R8F baseline**: it includes R8G's
five-tab Vault workspace normalization as well as the R8H persistence upgrade. Extract
this patch directly at the Cortex repository root, close/relaunch the PCC GUI, and run
its FULL quality gate. Do not publish until Windows FULL certifies the current source.

The legacy `CortexVolumeInventory.py` and its R8A–R8D tests remain available as
historical, read-only fixture coverage; the GUI now calls
`CortexPersistentVolumeInventory.py`, not the former two-million-entry list scanner.

## Authorities and safety

- Read the existing `.cortex-volume.json` marker through `PCCVolumeAuthority`.
  The identity is `volumeId`, never an assumed `G:` or `D:` drive letter.
- Write a dedicated metadata index only to the existing state authority:
  `<volume-root>/.cortex/inventory/<volumeId>/inventory.sqlite3` (derived from the
  volume layout, not an independently manufactured top-level folder).
- No source file move, rename, delete, contents read, hash, or project registration.
  No write to `vault_catalog.db`, Git registry, source repository, or OS registry.
  Filesystem discovery is **source read-only**; the authorized index database itself
  necessarily receives writes.
- The index's own directory is recorded as an excluded inventory boundary, not
  recursively inventoried. Junctions/reparse points, symlinks and distinct mounted
  devices are never traversed. Permission errors are retained and surfaced as gaps.
- Existing inventory identities are checked before read, query or resumed writes.
  The index format uses SQLite `user_version=1`. Unknown future versions fail closed.

## Durability and performance

- WAL-mode SQLite, committed in bounded batches; no full-volume in-memory list.
- An indexed, durable directory queue records unfinished directories.
  Cancellation/closing preserves completed batches. An unfinished directory is
  deliberately revisited after restart; path-keyed upserts prevent duplicate rows.
- Start / Resume continues an incomplete index. A completed index is not implicitly
  rebuilt: the **Rebuild Index** button explicitly asks for confirmation first.
- Relative original-spelling paths are stored in the database; the current mount
  location is supplied at query/presentation time. Index files can move along with
  the portable volume from `G:` to `D:`.
- GUI filtering/count queries happen off the Tk thread and repeated requests are
  coalesced; results are paginated at 200 records with Previous / Next navigation.
  During scanning, committed pages may be searched without waiting for completion.
- Progress is an activity indicator with exact indexed count and pending directory
  count. No inaccurate percentage or arbitrary two-million-entry maximum is shown.
- Source access errors do not masquerade as full coverage. The Access Gaps button
  retrieves a bounded sample of error records from the index.

## Known boundaries

This first persistent generation uses a literal case-insensitive path substring
query, not a semantic full-text engine. A filter across several million records can
still require a database scan; it is backgrounded, not instantaneous. A finished
index is a point-in-time snapshot and requires an explicit rebuild for fresh coverage.
Changing a volume marker's identity invalidates reuse of its old index. Resume
revisits unfinished directories and is not a transactional whole-drive snapshot of
concurrently changing filesystem state. Later passes can add targeted delta scans,
FTS/facet queries, retention policies, and operational performance gates.

## Windows acceptance

1. Apply patch, close and relaunch PCC. Header should show `PCC-GUI-0.15.0`.
2. Vault / Forge should expose Inventory, Catalog, Intake, Storage and Health.
3. Start / Resume Index scans the marked drive explicitly, without a two-million cap.
4. While scanning, filter for `projects/`, `Git/` and `Cortex`; use the page controls.
5. Pause, close and relaunch; confirm indexed rows persist, and Start / Resume
   continues. Verify the file count does not double on re-enumeration.
6. Use Access Gaps to inspect denied directories without changing permissions.
7. Move the portable drive between drive letters; paths should resolve under the
   currently mounted root, while the stored `volumeId` remains unchanged.
8. Run `python -B -m unittest -v tests/test_persistent_volume_inventory.py` and
   the authoritative PCC FULL gate; only then commit/push the matching GREEN source.

The first scan on a large drive can take a substantial period and consume disk space
for metadata/indexes. Pause is preferable to repeatedly discarding scans.
