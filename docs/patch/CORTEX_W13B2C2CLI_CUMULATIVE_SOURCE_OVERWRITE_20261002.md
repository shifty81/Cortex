# Cortex W13B2C2 CLI cumulative source overwrite — 2026-10-02

This source-overwrite rollup consolidates the current W13B2C2 work that was being certified through separate handoffs.

Included source authorities:

- W13B2C2A runtime-evidence foundation: project intent contracts, process stdout/stderr/exit evidence, runtime launch/verify tools, and acceptance proof.
- CLI convergence/request-route instrumentation: deterministic NewProject routing stages and embedded NewProject conversation isolation.
- NewProject classifier correction: phrases such as `new standalone Rust console project` route to `NewProject` before generic `Code` cues.
- ToolchainBroker 0.5 environment merge correction: Windows `PATH`/`Path` overlays are case-insensitive so VsDevCmd activation exposes `cl.exe` and `link.exe` reliably.
- Portable project environment 0.6: Rustup state, Cargo home, Cargo target outputs, and Cargo/Rust shims are Vault-owned and drive-letter independent. `RUSTUP_AUTO_INSTALL=0` prevents silent host-machine bootstrap.

The archive is intentionally a **cumulative overwrite source overlay**, not a destructive reconstruction of every unchanged file in the Cortex repository. Extract it over the existing current Cortex source root and overwrite included paths. Do not delete the repository first.

The archive does not include generated `target/`, `.cortex/`, build outputs, logs, artifacts, or machine-specific state.


## R2 regression-contract correction

The first post-overwrite Windows FULL gate proved that ToolchainBroker 0.5 itself
is active and that QUICK is GREEN, but one Python regression still asserted the
old literal `PCC-TOOLCHAIN-BROKER-0.4`.

R2 adds a merge-safe source normalizer rather than overwriting the three affected
test files wholesale. It updates only the stale version assertion in:

- `tools/control/tests/test_cortex_pcc.py`
- `tools/control/tests/test_pcc_failure_evidence_performance.py`
- `tools/control/tests/test_pcc_runtime_recovery_r8.py`

The normalizer preflights all three files, creates recovery copies before mutation,
and verifies the post-write source shape.


## R3 Windows environment-key canonicalization

The next Windows FULL gate passed QUICK and all 196 focused PCC regressions,
then reached the complete control suite. Of 348 control tests, exactly one
errored because `BackendClient._embedded_env()` could not index
`env["npm_config_cache"]`.

The ToolchainBroker 0.5 case-insensitive merge had retained the casing of an
already-inherited Windows environment key. R3 keeps case-insensitive duplicate
removal, but uses the authoritative overlay spelling for non-PATH variables.
`PATH` remains explicitly canonicalized to uppercase.

This preserves both requirements:

1. Windows process environments never carry duplicate keys that differ only by case.
2. Python mappings expose the exact dependency-policy keys declared by
   `PCCVaultStorage.dependency_environment()`, including `npm_config_cache`.


## R4 bounded NewProject classifier + canonical Rust formatting

The latest Windows FULL passed every Python/control test stage and reached
`cargo fmt --all -- --check`. R4 therefore provides an explicit canonical
format application using the same portable ToolchainBroker environment before
the next FULL certification.

R4 also tightens the NewProject classifier. The earlier broad implementation
could classify `build this project with a new feature` as NewProject because it
looked for the qualifier `new` anywhere in the sentence. The bounded classifier
preserves explicit creation phrases but otherwise requires the qualifier in the
preceding token window before `project`/`projects`.


## R5 rollup regression source-shape normalization

The R4 Windows FULL reached 350 control tests and failed exactly one
rollup-specific regression. The production classifier itself was not the
problem: R4 correctly replaced the broad qualifier array with a bounded
qualifier-before-project matcher.

The stale test still required the old literal source fragment `"standalone",`.
R5 replaces that assertion with a rustfmt-stable `"standalone"` token check while
retaining the stronger `qualified_project` bounded-classifier assertion.


## R6 Clippy dead-code repair and failure-evidence closure

R5 reached `cargo clippy --workspace --all-targets -- -D warnings` after every
earlier Python, format, check, and test stage passed.

The three `m11u2_prompt_requests_*` methods are compatibility test helpers, not
production controller APIs. R6 gates them with `#[cfg(test)]`, preserving their
test coverage while removing them from the normal library build where
`-D dead-code` can reject them.

R6 also closes a PCC evidence gap discovered by the same failure: the debug
bundle contained the Clippy exit code but not its captured stdout/stderr.
`CortexPCC.py` now names command captures by phase and copies a bounded set of
recent command-output captures into every debug bundle.
