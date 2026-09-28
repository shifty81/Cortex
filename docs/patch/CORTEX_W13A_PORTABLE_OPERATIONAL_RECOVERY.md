# Cortex W13A: portable operational recovery

**Baseline:** supplied `Cortex_SourceRollup_20260928-002458_f9143f2c.zip` (2026-09-28), not an older Git checkout. **Package:** one manifest-authoritative transactional root-intake patch, all existing-file preimages checked by SHA-256 and byte count. Source overwrite only; no live Vault or user-project payload is included. This patch is a **test candidate**, not a Windows-certified GREEN checkpoint.

## Why this is necessary

A successful `cargo`/Python FULL gate did not exercise the registered storage command dispatch. The dispatcher eagerly accessed a nonexistent `PCCVaultStorage.retention_plan` attribute, preventing unrelated commands from running. In addition, `storage.health` exposed an invalid command alias and `vault-mirror-all` invoked a single-project mirror. The model's blank chat response and the mismatch between PCC chat and Desktop's NewProject workflow are **different issues**; this patch does not pretend those operations are certified.

## Changes

- PCC storage command resolver is one lazy, executable provider map, with parser/provider regression coverage. `vault-retention-plan` now invokes `snapshot_retention_plan`; `storage.health` has a valid command. Previously unavailable `storage-reclaim-plan-all` is recognized and returns a read-only plan.
- `vault-mirror-all` now iterates actual registered roots using the marked portable volume and skips missing/unsafe/duplicate registrations; an unavailable registry produces an explicit failure, never a false success. Existing registry/source trees are not rewritten.
- `G:/projects` remains for source projects. New project **mirror metadata** is placed under `G:/Vault/ProjectMirrors/<project-key>`. Existing genuine old mirror metadata under `G:/projects/<key>` remains in place and is recognized through its schema. A normal source project with a `project.json` is not treated as a mirror. Both generations are considered in CAS reference scans/health. No automatic metadata moves or GC apply.
- Shared environment paths now export `CORTEX_PROJECTS_ROOT=G:/projects`, `CORTEX_LOCAL_GIT_ROOT=G:/Git`. Rust library layout, native model host, and local Git default are aligned. Actual volume letter comes from the currently mounted checkout.
- On a marked volume, PowerShell bootstrap writes `bootstrap-env.cmd` under `G:/.cortex`, not under the `G:/Cortex` source checkout. Both root bootstrap CMD wrappers read it there; unmarked setups retain their legacy source-local fallback. No bootstrap copies of old machine-local library authority into portable state.
- A native Cortex Desktop launch from its exe checks the volume marker/layout and binds the portable environment process-locally before opening its registry, preserving any explicit overrides. It does not force a D-to-G change or overwrite a different live library authority.
- New tests under `tools/control/tests/test_w13_operational_recovery.py` ensure the command signatures really resolve and prevent recurrence in the next Python FULL gate.

## Known limitations and next certification

1. Rust/Windows formatting, compile, and launch were unavailable in the artifact-generation environment. Execute `cargo fmt --all`, then FULL on Windows. Do **not** commit/push until GREEN. If format updates any source, let PCC certify the changed source before committing.
2. If `G:/.cortex/home/registry/library.json` itself stores `D:\`, and D: now identifies a different physical disk, the existing protective error can still occur. This is *correct fail-closed behavior*, not permission to reset/replace the registry. Inspect stored volume GUID and actual `Get-Volume` identity before proposing a reviewable authority rebind. Existing data is never silently migrated by this patch.
3. The natural-language project creation path from the PCC console and blank local-provider reply remain unproven. Reproduce with native Desktop after FULL, then certify actual create/register/build/run of HelloWorld_W12. No manually created fixture may be counted as proof.
4. Model replacement/automatic fast-code routing are deferred until the chosen smaller model completes tool-call/function-calling certification. Do not confuse a model configuration change with the missing project-creation orchestration.
5. No destructive retention, CAS cleanup, files relocation, source tree move, or inventory database manipulation is included. Multi-project mirror can create new metadata and objects when explicitly invoked; do not run it as part of a read-only diagnosis.

## Apply via normal root intake

1. Copy the **unextracted** root patch ZIP and its `.sha256` sidecar into the current checkout's registered patch intake; `G:\Cortex\updates\inbox` is the normal root intake folder. Replace `G:` with whatever drive letter holds the marked portable checkout. Do not extract over source, and close the old Desktop before replacing its source dependencies.
2. Use the project's root PCC patch scan, review the exact patch and its preimage hashes, then explicitly apply it. Control updates require PCC restart when instructed.
3. Start a fresh PCC, run Full Quality Gate. Review the W13 operational test outcome and `storage.health`, `vault-verify`, `vault-gc-plan`, `storage-status`, `storage-prepare` separately; a missing latest mirror is not itself a dispatch crash.
4. Rebuild native Desktop and retry launching directly with the Cortex source root as argument. Confirm its registry comes from the portable volume and that project registration/display is not doubled. Inspect before taking any action if an old D: authority is still reported.
5. Only then attempt the real HelloWorld_W12 end-to-end creation from native Desktop, preserving the currently selected Cortex project until Cortex proposes a new project for approval.

## Portable volume authority

`G:\Cortex` remains the source/application checkout. Persistent registry/state lives at `G:\.cortex`; source projects at `G:\projects`; imported source at `G:\Source`; Git at `G:\Git`; models at `G:\Models`; shared toolchains at `G:\shared`; controlled Vault metadata at `G:\Vault`. Source-local `artifacts/` is an ignored build/log transport directory, not the global Vault authority. This patch does not attempt a bulk scan or automatic reclassification of the existing drive.
