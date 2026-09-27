# Cortex W11 — Repair Agent Correctness (Cortex-only cumulative test checkpoint)

## Scope and authority

W11 is a cumulative overwrite of the W10 payload over the published R8J-A lineage. It changes only **Cortex** implementation, regression tests and documentation. The Havenwild Bevy installation, its Cargo manifest/lockfile, and the earlier optional DX12 probe are **not** part of W11. No existing portable-volume index, SQLite inventory, Vault catalog, Git repository, registry, model, user conversation, or recovery artifact is shipped or replaced.

This is a **Windows acceptance candidate, not a certified GREEN result**. Python suites can run on the isolated source reconstruction here; Rust/Cargo, MSVC, real local-model inference, native project PCC execution and Cortex Windows FULL must be executed on the user's machine before the package is called GREEN or published.

## Failure reproduced in the earlier real-world test

The W10 `/repair` case targeted an existing project with `Cargo.toml`, `Cargo.lock`, `src`, assets and a local `PCC.cmd`. Cortex's optional `SOURCE_MANIFEST.json` count was zero and the explicit Cargo workspace-member count was zero, then the model incorrectly interpreted these metadata fields as proof of an empty project. A successful `source.write_text` touched the manifest; the post-agent generic Cargo check failed and the transactional candidate was later rolled back, while the CLI misleadingly said it was still active. Project-native PCC FULL independently found a Windows wgpu-hal dependency conflict. The CLI produced many seconds of repeated inference heartbeats and the model printed hypothetical tool commands as prose.

## Changes now implemented

1. `workspace.status` exposes a distinct, controller-observed shallow root enumeration with marker/file/directory flags. `source_manifest.file_count` and `cargo_workspace.member_count` are explicitly scoped to their respective optional sources. A standalone Cargo package is recognized independently of `[workspace].members`.
2. Repair entry runs a physical-root/marker preflight **before opening the source transaction**. No authoritative root marker is a blocker, not authorization to initialize/overwrite a project. Verified facts accompany the model prompt and evidence receipt; generic recommendations or fenced pseudo-commands cannot count as mutation.
3. `PCCBuildDoctor` admits actual project-local `.pcc/logs` and `.forgepy/logs`, reads bounded tails, recognizes `FULL GATE: PASS/FAIL`, failed phases and Rust compiler errors. No recursive drive/file indexing is triggered by this intake.
4. Python bridge prioritizes root identity before the bounded diagnostic excerpt; long histories are capped and cannot drop the active workspace's root markers. Bridge version 0.9; GUI version 0.15.6b.
5. Rust repair tool profile excludes unavailable `vscode.*` adapter tools while retaining these for workflows that explicitly support them.
6. A real project-local `PCC.cmd` together with its Python controller now selects `ProjectControlCenter.py full` as the **project-native validation authority** (without model-generated shell commands); where no native checkpoint is known, the generic `build.project_validate` path remains. The native checkpoint command uses exact process execution without formatting-source preflight side effects.
7. Post-agent reporting records the chosen validation tool, controller-observed transaction state and touched/created file receipt. A failed check **cannot** claim the transaction remains active if the controller has already rolled back or ended it; the model narrative is marked unverified. Manual `tx commit` also checks the selected project authority instead of hardcoding Cargo.
8. Synthetic tests cover PCC log intake, restored root evidence, old-failure suppression by newer successful gate, bounded context and conversation, no unnecessary ordinary-chat diagnostic traversal, standalone Cargo preflight and native PCC selection.

## Windows acceptance sequence

1. Preserve the current published GREEN Cortex checkout/rollback point. Close Cortex PCC GUI, native Cortex client and any ongoing repair before applying. Apply only W11 ZIP as a repository-root overwrite; it includes W10. Use PCC's governed patch intake; do not extract unrelated third-party archives into the repo.
2. Confirm GUI `PCC-GUI-0.15.6b`, bridge `CORTEX-PY-BRIDGE-0.9`; check that the pre-existing portable inventory is opened without Rebuild Index. No initial scanner, registry migration, Vault rewrite or project source moves are expected from W11.
3. Run **Cortex PCC FULL**. Cargo fmt/check/test/clippy/build and Python regression suites must be GREEN. If Rust fails, send its full bundle; do not publish.
4. On an **independent disposable fixture** with an actual `Cargo.toml`, `src/main.rs`, local PCC wrapper/controller and synthetic failed PCC session log, run read-only `inspect`/`plan`, then `/repair` with approval. The execution must show the real root/marker evidence, project-local failure classification, genuine structured `source.*` calls, a bounded mutation in a durable candidate and the chosen project-native checkpoint.
5. Assert that if the fixture's controller returns failure or rolls back, the receipt reports the actual state rather than a guaranteed active transaction or false GREEN. Compare source hashes and touched/created paths before and after rollback. Check Python/Native Models per-turn latency separately from Cargo; collecting heartbeats alone is not progress.
6. Only after the fixture passes should a live registered project be used for another authorized repair. Preserve actual project/build logs and do not auto-commit/push project changes as a side effect of agent repair.

## Known remaining gaps / follow-up (do not claim complete)

- This W11 source has not been compiled or rustfmt-certified in the isolated environment; source inspection and Python tests do not establish Rust validity.
- Project-native PCC's `full` can perform governed update/asset actions by that project's own contract; acceptance must verify local PCC behavior before it is permitted on a live project. Explicit `.cortex/checkpoint.json` remains the primary override.
- Baseline-before-mutation quality comparison, multi-cycle fix/build/repair and automatic controller rollback on regression require further end-to-end testing. W11 makes the *post-check authority and report truthful*, but does not promise that a 9B local model can reliably repair arbitrary external Rust dependency conflicts.
- The model can still take many seconds/minutes to infer; this patch bounds evidence ingestion and removes failed VSCode tools in Repair, but does not add a resident inference worker or guarantee responsiveness.
- `PCCBuildDoctor` samples up to 512 direct entries per log directory: unusually huge folders may require a later indexed/latest-receipt query, not renewed recursive scanning.
- Native Python PCC discovery is limited to the explicit `PCC.cmd` + `ProjectControlCenter.py` pair. Other external project runners need a registered adapter/contract rather than guessed commands.

## Acceptance definition

A real repair requires: authentic root and failure evidence -> actual structured source reads -> authorized reversible source edit -> controller-owned project-native verification -> correct touched-files and transaction receipt -> real gate outcome. A prose repair plan, source snippet, synthetic GREEN, or model narrative by itself is **not** repair completion.
