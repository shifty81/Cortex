# Cortex R8A — read-only whole-volume inventory

This patch adds an **opt-in metadata-only scanner**. It does not relocate projects, reorganize folders, register projects, change the registry, modify `vault_catalog.db`, or create any files. Its sole output is JSON written to standard output. Cortex/Vault ingestion is a separate, later reviewed operation; this patch does not perform ingestion.

## Established volume layout

The scanner preserves the existing physical layout and original spelling/case, including a lowercase `projects/` directory and top-level `Git/`. Neither is renamed, merged, moved, or treated as an alias for the other. All discoverable entries are inventoried relative to the supplied root; no fixed `D:`, `E:`, or `G:` assumption exists. Hidden folders, `.git`, and other entries are included. No inferred project registration or canonical path rewrite occurs.

## Usage

From the repository root, run:

```powershell
python tools/control/CortexVolumeInventory.py "G:\" 
```

Replace `G:\` with the **current** drive letter or an existing directory on the portable volume. The process emits one JSON object to stdout; redirecting it to a file is a **separate, explicit user action**, not a scanner side effect. For a smaller dry run use a test directory. Optional `--max-entries 2000000` bounds memory and output. A partial scan reports `truncated: true` and exits 1; unreadable entries appear in `errors` and also cause exit 1. Invalid root or arguments exit 2. Complete scans exit 0.

## Safety and scope

Uses `os.scandir` and no-follow stat; does not open file contents, calculate hashes, traverse symlinks/junctions, or descend into another device/mount point. Metadata and filenames may be sensitive: treat stdout as local private data. A full volume with millions of entries can use substantial RAM because the returned inventory is in memory; use `--max-entries` to bound it. This is discovery, not a permission bypass: inaccessible directories are reported, not escalated. Inventorying a live filesystem is not an atomic snapshot. No filesystem mutations, project moves, registry writes, or Vault database changes are performed by the module.

## Tests

```powershell
python -m unittest discover -s tests -p test_volume_inventory.py -v
```

Patch archive is repository-root-relative and contains exactly the three R8A paths.
