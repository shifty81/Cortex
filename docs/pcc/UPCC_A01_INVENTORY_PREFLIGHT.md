# U-PCC-A01 — Read-only inventory and typed contract preflight

**Baseline:** Cortex GitHub `main` commit `2ecf5b1b2c98b5e571504b892c4b4649659ced02`. This patch requires the exact SOURCE03R3 `tools/control/CortexPCC.py` SHA-256. No project files or remotes were changed by constructing this patch.

## What this delivers

- `tools/control/UniversalPCCAudit.py`: stand-alone, standard-library Python 3.11+ CLI for read-only project inventory, explicit contract validation, and bounded local PCC discovery.
- `CortexPCC.py universal-audit` delegates to the audit tool **before** constructing operational `CortexPCC`, so it does not launch Git maintenance, create session artifacts or mutate a repository merely to inspect it.
- JSON results identify actual project root, optional HEAD/branch (read-only `git rev-parse`), authoritative manifest SHA-256, launcher presence, declared commands, exact gate stage keys, and violations. A registered command is **DECLARED**, never `VERIFIED` merely because it has a name.
- Typed Rust compatibility preflight rejects `git_or_snapshot` and `process_stop` as presently unsupported rollback values without rewriting the file or pretending their rollback effects are implemented. The `--legacy-only` switch produces warnings for research when Rust compatibility is not being asserted.
- Duplicate JSON keys/commands/gates, missing command programs, unknown permissions risk/cancellation, unsupported schemas, and missing gate stage references fail closed. The Ember missing `migration-regressions`, `module-tests`, and `integration-smoke` case is a regression test.
- Scan only when `--scan-root` is explicitly supplied. Traversal does not follow symlinks and ignores common generated folders; depth, directory count and project count are capped and **incomplete** scans are reported as incomplete.

## Commands

From Cortex repo root:

```powershell
python tools/control/CortexPCC.py universal-audit --root .
python tools/control/CortexPCC.py universal-audit --root . --scan-root 'D:\' --max-depth 5 > cortex-pcc-census.json
python tools/control/UniversalPCCAudit.py inventory --root 'C:\Path\To\OtherRepo' --root .
python -B -m unittest -v tools/control/tests/test_universal_pcc_audit.py tools/control/tests/test_cortex_upcc_a01_integration.py
```

`--scan-root` only discovers directories with `project.control.json`. It is **not** a full inventory of every repository or a source-fingerprint computation. If the discovery limit is reached or paths cannot be read, JSON sets `summary.scan_incomplete=true` and the CLI exits 2. An unmanaged repository is `UNMANAGED`, not GREEN. Saved JSON could contain private project paths/identities; review before sharing.

## Baseline finding and follow-up

The current Cortex project manifest contains rollback `git_or_snapshot` for `fmt.apply` and `process_stop` for `forge.rust.run`; typed Rust accepts `none`, `snapshot`, `transactional`, and `provider_owned`. Therefore, **the new audit will flag this existing contract as INVALID for the Rust consumer**. This is intentional evidence; this package does **not** change the contract or map old names without proving rollback behavior. The next pass must correct semantics and verify startup/health separately. Do not add this strict check to the old Full Gate until compatibility remediation is ready, because doing so would convert an existing SOURCE_GREEN project into an unexplained failing gate.

## Boundaries

This is the first tested **inventory and semantic-preflight slice**, not a universal runtime replacement, complete D:\ repo census, forged GREEN certificate, Rust Forge takeover or GUI normalization patch. Project-local PCC remains authoritative. ForgePY remains operational front-end. No updates, builds, dependency hydration, source rewrites, Git changes, SourceQuay mirroring or external network requests are initiated by the auditor. Its process reads `.git` HEAD and branch through read-only Git commands only when `.git` exists.

The next pass (A02) traces GUI/CLI/AI Full Gate against identical fixture with shared operation identity. A03 will integrate the validator at the execution boundary after the legacy rollback semantics have been resolved; it must not silently normalize unknown enum values. Source-exact ForgeGUI Core changes must use the latest local source, not the older September 15 public revision.
