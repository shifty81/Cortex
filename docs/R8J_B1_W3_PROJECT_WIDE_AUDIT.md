# Cortex R8J-B1-W3 — Project-wide audit and roll-forward

## Source and publication boundary

- Published prerequisite: GitHub `shifty81/Cortex` main `82d41230673ef2a179dc6fb2f1918493ad68837e`, the user's GREEN R8J-A checkpoint (parent `70410709b2cd5f484d14234711eb13c89da4ffb7`).
- This ZIP is repository-root-relative and contains **all W2 B1 replacement payloads plus W3 corrections**. It installs directly over R8J-A. B1, B1-W1 and B1-W2 are superseded; do not chain them.
- Retains the existing per-volume inventory data at `.cortex/inventory/<volumeId>/inventory.sqlite3` on the volume. No DB, Vault catalog, model files, artifacts, generated Rust binaries or full source rollup is shipped. The recursive SQLite tables are additive/lazy within the existing v1 inventory; no full drive rescan or file movement.

## Verified integration findings and changes

| Area | Initial W2 defect / gap | W3 correction |
| --- | --- | --- |
| FULL gate | Narrow explicit Python selection omitted other existing PCC tests, permitting eight project-wide test failures/errors to escape mandatory certification | Require complete `tools/control/tests` and `tests` discovery in addition to original mandatory fixtures, with buffered output and real failure diagnostics |
| Portable PCC registry | Legacy helper functions absent; unstable absolute-drive-letter identity; duplicate historical registration | Stable volume-ID plus volume-relative registry IDs; read-only historical registry merge; explicit registration preserves active identity; unrelated local projects retained |
| Git checkout authority | GUI Trust Current Checkout had no matching PCC command; `summary-json` accepted `gitReady=false` | Add `git-trust` to parser/dispatch/catalog; quick gate enforces `gitReady`; scoped Git authority itself handles safe.directory |
| Mutating command description | Repair and selected Git/build/diagnostic commands incorrectly classified read-only | Correct command risk to `local_mutation` for the known write-bearing operations |
| Gate failure diagnostics | Expensive deep Vault checks could delay evidence; negative fixture output looked like actual patch failure | Bounded quick Vault summary in lightweight evidence; successful negative fixtures are buffered |
| Recursive recovery | `Discard Paused Queue` checked a job before beginning its SQLite write transaction | Read state/owner only after `BEGIN IMMEDIATE` writer reservation |
| Windows worker probe | W2 already fixed unsafe `os.kill(pid, 0)` behavior | Retained W2 non-signalling process-handle check and tests |
| Tree reconciliation | W2 already fixed missed root descendants, transient access gaps, deleted queued descendants, and resumable job claim | Retained all W2 fixes and fixtures |

## Verification boundary

- Source reconstructed locally from the supplied rollup and cumulative patch lineage, then audited against published R8J-A file hashes and GitHub's `PCCSurfaceCommon.py` blob.
- Full Python gate, both complete discovery groups, PCC provider tests, GUI smoke and static Python/JSON/Cargo parse checks run on a Linux isolated fixture. The Windows workstation FULL gate, real 2.24M-entry-volume acceptance, Cargo compile/test/clippy/build and PowerShell AST verification are **not** certified by the Linux audit.
- No claims of automatic change monitoring, guaranteed rename/relocation identity, project auto-registration, or finished Organization Review. Those remain later R8J-B2 work.
- `tools/control/PCCSurfaceCommon.py.fixpayload` is present in the older full-source snapshot, but is outside this focused overwrite package. Its tracking/ownership must be checked on the real Windows repository before deletion; no speculative cleanup was performed.

## Windows acceptance

Install only W3 over published R8J-A; confirm `PCC-GUI-0.15.5c`, load the saved inventory without rebuilding, test selected-tree refresh and pause/resume on a small project, open Access Gaps and Storage Health, verify Trust Current Checkout is a real explicit command, run authoritative FULL and publish through the existing GREEN workflow when satisfied. Do not discard or rebuild the inventory.
