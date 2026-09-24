# Cortex U01 — Universal Project Intent + Runtime Acceptance

Date: 2026-09-23

This pass corrects the failure where an explicit **native C++ Windows desktop/CMake**
request could be silently converted into a Rust/Cargo console project and then reported
GREEN because the substituted project compiled.

## Authority changes

1. New-project requests now persist `.cortex/project-intent.json`.
2. Explicit C++/CMake Windows GUI requests scaffold CMake + `src/main.cpp` instead of
   `cargo new`.
3. Existing unspecified new-project behavior keeps a Rust compatibility default, but the
   compatibility default is not recorded as user intent.
4. Code/Repair completion now requires both:
   - detected project quality gate GREEN;
   - project-intent acceptance GREEN.
5. CMake project operations configure under `.cortex/build/cmake`, build Debug, then test.
6. `runtime.launch_project` and `runtime.verify_project` use the universal project plan and
   `RuntimeArtifact` instead of refusing non-Rust projects.
7. Runtime window proof is required only when the request actually asks for a GUI/window.
8. M11U2 mutation accounting is moved from attempted writes to successful write results,
   so denied writes do not create phantom mutation-to-validation barriers.
9. Chat presentation fixes the malformed FileBubble delimiter and avoids redundant full-file
   cards when a useful diff card already exists.

## Regression contract

A project that requests C++/CMake/Windows GUI but ends as Cargo/Rust must now fail intent
acceptance even if Cargo compiles successfully.

A valid C++/CMake GUI result must expose C++ source, CMake, a windowed target/runtime profile,
pass the detected build gate, and—when launch was requested—pass runtime window acceptance.

## Verification in authoring environment

- Python PCC/control suite: 153 tests PASS.
- Static delimiter/source-contract checks: PASS.
- Cortex root patch validation/apply simulation: performed when packaging.
- Rust Cargo build/check/test/clippy: NOT RUN in the authoring environment because Rust tooling
  is unavailable there. Windows build and runtime acceptance remain required before declaring
  product GREEN.
