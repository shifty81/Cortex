# CTX-UPCC-A04 — ForgePY parity foundation / inventory

Date: 2026-09-19. Classification: cumulative root-drop patch **for an already-installed A03**. This is an implemented read-only capability-inventory slice, **not feature parity**.

## Exact authority and source versions

- Cortex published GREEN baseline: `5579ecdc1bbb2fa0935315bec7b838d496752f8b` (A01–A03). This handoff requires that A03's files are still byte-exact; any newer local checkout must be rebased, not overwritten.
- ForgePY published source baseline: `f00ca0fea2dfa29dc930fcc18ddcedc347a7c6ad` (`FORGEPY-F797` candidate). Its README distinguishes the older certified reference `FORGEPY-F60R415` from this newer integrated candidate. Published metadata do not establish whether the user's **9.19.1** PCC installation is identical to either source version.
- ForgeGUI_Core source: `eafa8e78efd54142a19e66d8be7b7d3985af23d2`, internal PCC provider 0.4.13. The newer public `forge_gui_shell` is a UI framework donor, not an execution authority.

References:
- https://github.com/shifty81/Cortex/commit/5579ecdc1bbb2fa0935315bec7b838d496752f8b
- https://github.com/shifty81/Forge/commit/f00ca0fea2dfa29dc930fcc18ddcedc347a7c6ad
- https://github.com/shifty81/Forge/blob/f00ca0fea2dfa29dc930fcc18ddcedc347a7c6ad/README.md
- https://github.com/shifty81/ForgeGUI_Core/blob/eafa8e78efd54142a19e66d8be7b7d3985af23d2/docs/INTERNAL_PCC_STANDARD.md

## Why copying ForgePY wholesale would recreate friction

Cortex already owns root Full Gate/GREEN, guarded Git, patch transactions, verified debug evidence, source rollups, typed command schemas and basic cross-project inventory/plan. It also has a narrower second Python PCC package and a Rust PCC bridge hardwired to a project's vendored `tools/pcc`. ForgePY additionally exposes machine-wide registry, discovery/onboarding, adapter selection, patch review/routing, Downloads watcher, ForgeGit, artifact indexing, Vault scanning, backup/dependency resolution, and GUI operation surfaces. A file with a similar name in both repositories is not evidence that the same operation has equal behavior.

**Authority invariant:** project PCC remains authoritative for project-specific gates and patch/commit rules; Cortex universal orchestration invokes that provider once via an operation ID. ForgePY remains the active universal production front end until an alternative passes behavioral parity and is approved. ForgeGUI supplies components/shell, not another executor. No patch may silently consume pending updates during Build/Full Gate.

## Implemented A04

1. `tools/control/ForgePYParity.py`: bounded, read-only source-anchor inventory for 22 feature groups. Takes explicit Cortex and optional ForgePY roots; no network access, recursion, donor import or subprocess. Emits JSON with separate donor vs consumer presence, source reference and unverified status. It never reports semantic parity from file presence.
2. `CortexPCC.py forgepy-parity --root . [--forgepy-root <directory>] [--strict]`: dispatch before controller construction, avoiding session-log writes and startup mutation. `--strict` exits 2 for known contract blockers or missing Cortex anchors; default reports without treating expected gaps as a command failure.
3. Full Gate now requires the existing A03 restart tests **and** the new A04 parity inventory tests; missing suites fail closed.
4. Eight disposable-fixture tests cover no false certification, missing source separation, no donor inference, unchanged invalid rollback strings, donor candidate identification, strict status and zero repository writes.

No ForgePY executable is vendored; no GUI replacement, version bump, rollout claim, or automatic asset/log cleanup is included. Two known typed rollback blockers (`git_or_snapshot` and `process_stop`) are intentionally reported, not lexically replaced.

## How to inspect YOUR actual ForgePY installation

From Cortex root after applying and restarting:

```powershell
python tools/control/CortexPCC.py forgepy-parity --root . --forgepy-root 'C:\Users\Shifty\Desktop\ForgePY' > artifacts\forgepy-parity.json
```

Optional strict preflight:

```powershell
python tools/control/CortexPCC.py forgepy-parity --root . --forgepy-root 'C:\Users\Shifty\Desktop\ForgePY' --strict
```

The inventory's `parityCertified` remains false even if all anchors exist. Confirm the local directory and compare its source version and manifest; don't infer it from the published F797 Git repository. Reports can be saved under `artifacts/`; their storage/retention is not modified by this pass.

## Parity implementation lanes and feature acceptance criteria

| Lane | Missing work / required semantic proof | Primary Cortex owner |
|---|---|---|
| 1: Authority & contract | One normalized project ID, provider priority project PCC > project contract > selected adapter > discovery; report unknown/ambiguous/unsupported, never invent confidence; contract validation for risk, cwd, cancel, rollback. Fix `fmt.apply` snapshot and `forge.rust.run` process-stop as two **different** semantics with disposable recovery tests. | `tools/pcc/src/pcc/`, `crates/cortex_project`, `crates/cortex_pcc`, `UniversalPCCPlan.py` |
| 2: Operations | Same operation ID from GUI, Cortex chat, CLI and Forge; durable queue/lease, streamed JSONL and logs, cancel/timeout/reconnect; source mutation approval; avoid recursive dispatch. | `cortex_project_ops`, `cortex_jobs`, root PCC process runner |
| 3: Project intake | Component-aware Cargo/CMake/Python/Node/.NET/Gradle/Unreal/Godot detection; dependency/toolchain doctor; package-adapter proposal requiring approval; cache, provenance and no expensive startup full-drive scans. | `UniversalPCCAudit`, `UniversalPCCPlan`, project registry |
| 4: Updates | Bound patch identity distinct from applicability; review/queue only; approved apply with preimage+postimage, safe archive, failed/rebase/superseded disposition, cumulative replacement, cross-patch rollback and provider restart handshake; explicit gate after apply. | `CortexPatchAuthority`, root PCC, GUI provider |
| 5: Certification & Git | One canonical stage graph and GREEN fingerprint; correct false-positive exit-code output, immutable evidence, guarded commit/push, FF-only pull, optional local ForgeGit mirror; asset/dependency scopes separately identified. | `CortexGitAuthority`, gate engine, contract/receipts |
| 6: Artifacts / Vault / diagnostics | Artifact Central registry; debug/handoff lineage; project-scoped Vault catalog, content provenance and license; dependency hydration with approval; on-demand heavy CSV; log rotation and dry-run-first retention (never silently delete). | `CortexPCCMaintenance`, `PCCVaultCatalog`, `cortex_vault` |
| 7: Standalone GUI | ForgeGUI public shell pinned by exact commit; Projects/Workspace/Chat/Vault nav; 10% Operations / 40% Chat / 40% Console / 10% Health; persisted docking, floating resize, one actual conversation store, no separate PCC build engine. | Cortex desktop native + `forge_gui_shell`, Tk fallback |
| 8: System parity gate | Golden fixtures compare normalized outputs from ForgePY and Cortex for success, failures, mismatches, cancellation, stale GREEN, dirty source, failed update, interrupted restart, broken provider, absent tools, multi-project and large-catalog loads. Mark each capability matched / intentionally different / incomplete with evidence. | Dedicated acceptance matrix; Windows runtime |

## Mandatory checkpoint ladder

- **A04 inventory:** source anchors and direct blockers, not behavior.
- **A05 recovery:** typed contract semantics, no false GUI-ready, patch result/restart handshake.
- **A06 core convergence:** provider discovery and exact command authority, durable operation envelope.
- **A07 project onboarding:** multi-component adapter fixtures and project-identity reconciliation.
- **A08 patch parity:** review/queue/transaction/supersession and recovery fixtures.
- **A09 source and certification:** source/asset scopes, ForgeGit, GREEN/commit gate parity.
- **A10 storage:** Vault, artifact lineage, bounded catalog/log retention with dry-run.
- **A11 GUI:** public ForgeGUI Core shell integrated with existing Cortex Chat/Console and PCC operation service.
- **A12 certification:** local Windows Full Gate + native desktop smoke + ForgePY differential fixtures; only after these can claim parity for a tested feature set.

Each later handoff must contain cumulative source changes since A01 and use source-specific preimages appropriate to the **currently installed** patch sequence. A cumulative update is not a full source rollup. Do not install A01/A02 again after A03; A04 targets A03 explicitly.

## Known limitations / actions before broader code merge

- User-reported PCC `9.19.1` identity cannot be mapped to a published donor/source fingerprint from available evidence. This tool reads its actual `project.control.json` when `--forgepy-root` is provided; a complete current source rollup or verified local Git SHA is required for exact module-by-module parity migration.
- Published ForgePY `FORGEPY-F797` is a candidate, not proof that all modules are certified. Published ForgeGUI main 0.4.13 includes a later merger; live native GUI certification has not been performed in this authoring environment.
- Cortex A03's Windows Full Gate GREEN establishes source certification only. Actual desktop launch failed because of the legacy typed rollback contract; A04 intentionally does not claim to repair it.
- The running A03 GUI may still report a misleading failure when self-updating because it loaded the old controller. Apply A04 using the standalone `CortexPatchAuthority.py` CLI with PCC GUI closed; validate the receipt and relaunch before quality gate. Its user-facing GUI restart remains an A05 blocker.
