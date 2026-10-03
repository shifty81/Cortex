CORTEX W13B2C2 CUMULATIVE SOURCE OVERWRITE ROLLUP R6 — 2026-10-02

R6 supersedes R5.

LATEST WINDOWS FULL RESULT
--------------------------
R5 passed:
- QUICK PROJECT GATE
- 196 focused PCC regressions
- 350 all-control tests
- 81 inventory tests
- nested project-control tests
- root-staging tests
- universal provider tests
- cargo fmt --all -- --check
- cargo check --workspace --all-targets
- cargo test --workspace --all-targets

The remaining gate is cargo clippy --workspace --all-targets -- -D warnings.

ROOT CAUSE / REPAIR
-------------------
The current desktop core still carries three M11U2 compatibility helper
functions used only by #[cfg(test)] tests:
- m11u2_prompt_requests_runtime
- m11u2_prompt_requests_window
- m11u2_prompt_requests_title_change

They are not production runtime APIs. In a normal library build, Clippy with
-D warnings treats them as dead code. R6 marks those helpers #[cfg(test)] so
they exist only in the test target where they are actually used.

R6 also rewrites the bounded NewProject qualifier expression into a simpler
boolean form while preserving the same qualifier-before-project semantics.

DEBUG EVIDENCE FIX
------------------
The latest debug bundle recorded only "cargo-clippy failed / exit 101" and did
not include Cargo's actual stderr capture. R6 corrects that too:
- command capture filenames now include the operation phase;
- debug bundles include up to 12 recent command-output files;
- individual captures are capped at 2 MiB;
- total command-output evidence is capped at 4 MiB.

If any later gate fails, its actual compiler/linter diagnostics should now be
inside the debug bundle rather than requiring another console transcript.

APPLY
-----
1. Close Cortex/PCC.
2. Extract R6 directly over the existing <Vault>\Cortex folder.
3. Choose Replace/Overwrite All. Do NOT delete the existing Cortex folder.
4. Run APPLY_CORTEX_R6_CORRECTIONS.cmd once.
5. Run VERIFY_CORTEX_SOURCE_OVERWRITE.ps1.
6. Launch PROJECT_CONTROL_CENTER.cmd and run FULL QUALITY GATE.

The R6 wrapper chains the existing R5/R4 normalization, including portable
ToolchainBroker-backed canonical Rust formatting.

A Windows FULL GREEN is not claimed until the final FULL gate succeeds.
