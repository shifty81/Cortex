# Cortex Source Recovery — CTX-SOURCE01

This pass adds a standalone Python 3.11+ source-export and verification command to **both** known Cortex source lines without overwriting the PCC, project manifest, Rust crates, or existing patch files. It is an additive root-drop patch. It does not fix GUI startup, Git history, nested Forge, or product release certification.

## What it captures

- Every accessible regular source-tree file not explicitly excluded, rooted directly at repository root in the ZIP, including untracked source and configuration.
- A per-file SHA-256 and byte count, verified by re-reading the source after packaging and again from ZIP data before the ZIP is published.
- A limited Git snapshot (whether this is a Git repository at the specified root, HEAD, branch, changed-path count); it never copies `.git` or credential-bearing remote URLs.
- An allowlisted, sanitized summary of applied patch receipts, and filename/ID/hash metadata of recognized pending root patch ZIPs. **Pending patch payload ZIPs are not included**. Preserve them separately.
- All intentionally excluded paths and reasons in `SOURCE_ROLLUP_MANIFEST.json`.

Excluded: `.git`, `.cortex`, `artifacts`, `target`, logs, package caches, common binaries/models, ZIP transports, `.env`, private keys and named credential files, and linked/reparse locations. A source file exceeding 64 MiB or a total source set exceeding 1 GiB causes a clear error, not a false-complete package. The result is a recovery-grade *governed source snapshot with explicit exclusions*, not a byte-for-byte mirror of the machine.

## Run from the Cortex root

```powershell
python tools/control/CortexSourceRollup.py create --root .
```

The command prints `SOURCE_ROLLUP_CREATED=...` and `SOURCE_ROLLUP_SHA256=...`. Its output is written under `artifacts/source-rollups/`, alongside a `.sha256` sidecar. Verify it using:

```powershell
python tools/control/CortexSourceRollup.py verify --archive "artifacts/source-rollups/<actual-filename>.zip"
```

Use the **actual filename** printed by the create command. This archive is not a PCC patch; never drop it into patch intake. Upload this archive for an exact-source comparison. Keep the original checkout unchanged and preserve separately any pending update ZIPs that are needed in recovery. Review the archive before sharing: source code can itself contain accidentally committed credentials that filename checks cannot detect.

## Safety notes

The exporter modifies only its own output directory under `artifacts`, never the project source or Git. It does not initialize Git, run builds, apply updates, follow filesystem junctions, extract anything, or make network requests. If the tree changes during capture, it aborts without publishing a final ZIP. Git changes are detected by before/after metadata, and source files are content-hashed again. It is not a cross-process filesystem snapshot: write activity between validations remains a theoretical race. Pause source-mutating builds or editors before capture if possible.

The new tool is independently executable because the known Cortex PCC generations have divergent file hashes and update-menu layouts. Wiring it into a common PCC menu requires confirming the authoritative current source first. A successful export is **not** a Cortex Full Gate or Windows runtime certification.
