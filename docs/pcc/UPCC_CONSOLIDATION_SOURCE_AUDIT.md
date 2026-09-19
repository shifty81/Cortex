# Universal PCC — Cortex / ForgeGUI consolidation audit and implementation contract

**Review date:** 2026-09-19 (UTC). **Status:** source-supported *partial implementation audit and migration specification*, not a fully enumerated GitHub/local-repository audit, compiled change, patch, or universal-PCC certification.

## 0. Authority, evidence and source cutoffs

- Cortex remote: `shifty81/Cortex`, `main`, `2ecf5b1b2c98b5e571504b892c4b4649659ced02` (2026-09-19). The supplied local Windows log explicitly records a successful Full Quality Gate and commit/push to `origin/main`. It certifies that run's source stages; it does not certify GUI launch, model inference, ForgeGUI interoperability, or universal PCC parity. Evidence: conversation log `Pasted text(20260919-013924).txt` and `https://github.com/shifty81/Cortex/commit/2ecf5b1b2c98b5e571504b892c4b4649659ced02`.
- ForgeGUI_Core published `main`: `532f7e1a0dfe4d7eefc46680a1567242059640a7`, dated 2026-09-15. Its published repository exposes reusable GUI/service crates. The user's September 17–18 docking/chrome development and more recent Forge Control Center implementation may be ahead of that public revision; no full corresponding source snapshot or Windows certificate is available here. Evidence: `https://github.com/shifty81/ForgeGUI_Core/commit/532f7e1a0dfe4d7eefc46680a1567242059640a7` and historical `Cortex_Universal_ForgeGUI_Integration_Audit_20260918.md`.
- This review additionally uses the user's supplied 3,134-line universal PCC research/pasted request; older claims about ForgePY, Havenwild, Subspace, Ember, Windstead and Stardew Modding Kit are **historical leads**, not independent current-build certification. Local D:\ trees, unpushed work and complete current repos were not examined.
- Existing GUI normalization specification: `Cortex_Standalone_GUI_Normalization_Spec_20260919.md`, designed against published Cortex and explicitly not implemented. Its approved visual target is Project Operations 10%, Chat 40%, console 40%, Health 10%; global `Projects | Project Workspace | Chat`, with `Vault / Forge` right-aligned.

## 1. Primary correction

**Do not build another complete CI/PCC UI. Audit and preserve the working interface, trace its exact backend, then converge the operations underneath it.** ForgeGUI_Core is a reusable widget/docking/chrome/services library, not the project-specific gate authority. Cortex is the intelligence, conversational/agent state, and standalone-application owner. Forge/ForgePY is the optional multi-project manager and current universal operational front-end. Each repository retains an independently runnable project-local provider and truthful command/gate configuration.

**Consolidation target:** one versioned *project operation protocol* with one dispatcher/authorization/operation registry per executing host and explicit delegation to project-owned PCCs. Avoid requiring all repositories to carry a giant Cortex or Forge application. Reuse ForgeGUI_Core for the interface only. No new mandatory `universal.pcc.v1` root schema: retain `forge.project.v1` and legacy importers, then introduce a versioned protocol independently of project config.

## 2. Source-traced execution surfaces (KEEP / EXTRACT / NORMALIZE)

| Current path | Verified code/source evidence | Classification and decision |
|---|---|---|
| **Cortex Tk Project Control Center** | `tools/control/CortexPCCGui.py` builds `Projects`, `Project Workspace` and right-aligned `Vault / Forge`; Workspace's `FULL GATE` button calls `_start_command("full")`, and BUILD and RUN are separately dispatched. `PCC-GUI-0.10.1` in published source. | **KEEP as transitional shell** and trace `_start_command -> BackendClient -> project provider -> operation host -> CortexPCC` with instrumented operation IDs. Do not label it already ForgeGUI-native. |
| **Cortex root project provider** | `tools/control/ProjectControlCenter.py` forwards non-Forge commands to `CortexPCC.main`; `project.control.json` declares that provider and CortexPCC implementation authority. Actual Windows Full Gate log shows fmt/check/test/Clippy/build, source GREEN marker, and Git/patch status. | **KEEP as independent local execution fallback; EXTRACT** generic streaming, cancellation, logging, gate and patch mechanisms only after parity. Cortex-only Cargo/tool/remote assumptions stay in adapter. |
| **Cortex reusable Python PCC** | `tools/pcc/src/pcc` is another package, invoked by typed Rust bridge through `python -m pcc --project-root ... --json`. Its command catalog reads project descriptors. | **NORMALIZE as a project-provider candidate.** Audit parity with root CortexPCC; currently not proven identical. Do not switch providers simply because both say PCC. |
| **Cortex native AI → PCC** | `crates/cortex_tools/src/lib.rs` has model-facing `pcc.status`, `pcc.catalog`, etc.; `crates/cortex_pcc/src/lib.rs` discovers project-local `tools/pcc/src` and implements typed status/catalog/doctor/gate/registered commands. | **EXTRACT typed API and policy.** Discovery presently assumes that bundled Python path, so it is not yet a universal adapter for arbitrary repos. Never expose model-generated raw argv as an exact command. |
| **Cortex project/transaction models** | `crates/cortex_project/src/lib.rs` contains typed identity, risks, cancellation and rollback; project operations/transactions have rollback fixtures evidenced in Windows Cargo tests. | **PRIMARY CONTRACT/TRANSACTION DONOR**; generalize types without imposing Cortex cargo structure. Keep realistic backend rollback semantics, not just renamed enum strings. |
| **ForgeGUI_Core published source** | Reusable crates include `forge_gui_services`, `forge_gui_command`, `forge_gui_console`, `forge_gui_chrome`, `forge_gui_egui`, etc. Published latest `main` predates newer local docking/host work. | **KEEP UI layer; audit code-level capability of each module against exact newer local source.** Do not copy Python process host into GUI widgets or require GUI open for builds. |
| **Forge GUI standalone consumer / Cortex bridge candidate** | Prior `ForgeGUI_RootDrop_Cortex_FirstClass_Surface_CTXFG02_20260918.zip` contains a ForgeGUI `.patch` with a hard-coded sibling `Cortex-Main` path. Historical `products/forge-rust/crates/cortex-bridge` includes a controller/host bridge. | **HOLD older consumer patch**; exact source rebase and path-independent package/IPC required. This patch's existence does not prove installed/current consumer or a working Cortex tab. |

Useful exact published source links:

- `https://github.com/shifty81/Cortex/blob/2ecf5b1b2c98b5e571504b892c4b4649659ced02/tools/control/CortexPCCGui.py`
- `https://github.com/shifty81/Cortex/blob/2ecf5b1b2c98b5e571504b892c4b4649659ced02/tools/control/ProjectControlCenter.py`
- `https://github.com/shifty81/Cortex/blob/2ecf5b1b2c98b5e571504b892c4b4649659ced02/tools/control/CortexPCC.py`
- `https://github.com/shifty81/Cortex/blob/2ecf5b1b2c98b5e571504b892c4b4649659ced02/crates/cortex_pcc/src/lib.rs`
- `https://github.com/shifty81/Cortex/blob/2ecf5b1b2c98b5e571504b892c4b4649659ced02/crates/cortex_project/src/lib.rs`
- `https://github.com/shifty81/ForgeGUI_Core/tree/532f7e1a0dfe4d7eefc46680a1567242059640a7/crates`

**Unverified link in execution trace:** The full `_start_command` and `PCCSurfaceCommon.BackendClient` implementation in the latest local GUI was not provided. The above is a source-supported entrypoint and downstream backend identification, **not** proof of every intermediate callback, authorization step, or streamed event.

## 3. Duplicate authorities and hard incompatibilities

| Finding | Risk | Normalization test |
|---|---|---|
| Root `CortexPCC` Full Gate and `tools/pcc` declared gate are separate paths. | A GUI may certify different stages from Cortex AI, yet both display GREEN. | Run both on same source/gate fixture; compare exact stage graph, toolchains, fingerprints, receipts and failure conditions. Missing Python regression must block the relevant gate. |
| Manifest has historical `git_or_snapshot` and `process_stop` rollback strings while Rust enum accepts `none`, `snapshot`, `transactional`, `provider_owned`. Screenshot shows desktop fatal parse and `launch-gui PASS`. | False runtime readiness; renaming string may not implement actual restoration or process termination. | Validate effective Rust contract **before** launch; require runtime/window handshake; prove actual rollback/process-tree stop. Separate SOURCE_GREEN, PROCESS_STARTED, GUI_READY, PROVIDER_E2E, RELEASE_CERTIFIED. |
| `ForgeGUI_Core` currently available public revision is 09-15; newer local dock/chrome/CI changes are not in this audit. | A patch against GitHub main may overwrite working local UI. | Export current ForgeGUI source and receipts, preserve source hashes/commit, compare to published revision, then apply preimage-guarded patches only. |
| A ForgeGUI-to-Cortex patch uses relative workstation sibling path. | Rename/move breaks build and project isolation. | Use versioned dependency/artifact package or authenticated loopback IPC; verify independent standalone build with repository moved. |
| Discovered command vs executable command vs verified functionality are conflated. | Green controls for nonexistent providers. | Five states: DECLARED, RESOLVED, VERIFIED, UNAVAILABLE, FAILED; gate references must resolve to exact registered keys, exact provider, permission and executable. |
| Patch/gate update side effects differ per project. | Unexpected self-mutation during certify, stale pending counts, source drift. | Default Full Gate certifies existing state; separately named Apply Updates + Certify performs authorized intake. Preserve legacy behavior as explicit compatibility profile until converted. Failed/held/superseded routes carry receipts and rescan post-state. |
| Source-only GREEN misses binary assets/environment and process/app behavior. | False game/editor release certification. | Source+assets+dependencies+contract+environment fingerprints, per-stage and runtime receipts. Declare required input scope per project; independently certify actual app and clean restore. |
| GUI startup currently performs hygiene `apply=True` according to published `CortexPCCGui.py`. | Merely opening UI can move files. | Startup default discovery/dry-run; any repair/move requires explicit permission and evidence. |

## 4. Single operation contract (proposed; NOT implemented)

**Compatibility:** keep `forge.project.v1` as public project config where supported; adapt Cortex `schema_version: 1` and project-specific formats through lossless, read-only importers. Unknown risk, duplicate command, missing required gate stage, unknown rollback behavior and unsupported schema fail closed. Preserve original project files during conversion.

**Protocol `pcc.operations.v2`** (independently versioned):

```
protocol.negotiate         project.identity       project.health
capabilities.list/verify  commands.list/describe
operations.start          operations.status     operations.subscribe
operations.cancel         operations.history
quality.plan/execute      quality.receipt/verify
updates.scan/preview/apply/recover
artifacts.list/verify     diagnostics.bundle
source.status/export      source.mirror/verify  source.publish
```

Every operation returns project identity, provider identity/version, idempotency key, `operation_id`, correlation/parent IDs, mutation risk, authorization decision/receipt, source fingerprint, stage results, artifact references, and terminal status. Live stdout/stderr is an event stream distinct from the authoritative result. State persists across GUI detach/restart. Cancellation and rollback are independently tested. Never allow recursion `Cortex → PCC → Cortex` for the same operation ID.

**Ownership:** project-local PCC executes its own commands and remains standalone; universal operation service authenticates/coordinates and delegates; Cortex investigates/plans/edits only with authorization; ForgePY remains active universal manager pending Rust successor certification; ForgeGUI_Core presents shared state. SourceQuay is a *proposed* Git-history/mirror name (existing `forgegit` aliases remain); Vault owns large assets. Mirroring from GitHub must not silently publish local work; no force pushes or automatic remote replacement.

## 5. Exact first implementation boundary (no big-bang rewrite)

### U-PCC-A01 — Freeze and source census (read only)

- Record each current repo GitHub ID, default branch/SHA, available local root/SHA/source fingerprint, PCC version/launcher, provider identity, declared commands/gates, execution engine, patch and rollback semantics, language/build stacks, assets, external integrations, test receipt. Mark inaccessible/local-only rows `NOT AUDITED`, not green.
- Mandatory first donor set: Cortex, ForgePY, ForgeGUI_Core, Codename Subspace, Havenwild, Ember. Next: Windstead, Stardew Modding Kit, then all other discovered repos. Do not infer a complete census from filename search. No mirror claimed until refs + LFS/wikis as applicable are inventory-verified.
- Read-only, no remotes/branches renamed, no Git init unless explicitly approved; do not merge duplicate clone histories.

### U-PCC-A02 — Trace and instrument one real action

- **Cortex exact files:** `tools/control/CortexPCCGui.py`, `tools/control/PCCSurfaceCommon.py`, `tools/control/ProjectControlCenter.py`, `tools/control/CortexPCC.py`, `tools/pcc/src/pcc/`, `crates/cortex_pcc/src/lib.rs`, `crates/cortex_tools/src/lib.rs`.
- **ForgeGUI exact files need latest local source freeze first:** consumer's `app.rs`/CI panel, ForgeGUI command/console/services/process/panel crates, project PCC authority, Cargo manifests, package manifest.
- Exercise GUI Full Gate, CLI Full Gate and Cortex tool-broker gate against one disposable fixture. Capture the full path: UI request → selected project → lookup → risk/permission → operation ID → provider → process → streamed events → stage results → receipt → GUI/AI status. Compare fingerprints and results. Instrument without replacing backend.

### U-PCC-A03 — Validate contract once at provider boundary

- Build a schema/semantic validator shared by GUI, CLI, AI and future Forge native. Rust typed enum checks must match manifest; remove unproven lexical rollback substitutions. Preserve complete stage graphs and extension metadata. Negative fixture: Ember's missing `migration-regressions`, `module-tests`, `integration-smoke` must fail before any GREEN/build dispatch.

### U-PCC-A04 — Unify operation IDs, not whole applications

- Introduce the versioned request/result/event adapter wrapping existing Cortex/ForgePY/project-owned executors. Test one active job visible from both Cortex and Forge GUI without double scheduling, with detach/reconnect and cancellation. No in-process embedding of a foreign native HWND or copying Cortex source into ForgeGUI widgets.

### U-PCC-A05 — Patch, GREEN and recovery parity

- Test duplicate/invalid/preimage/failed/held/superseded/cumulative patch, real rollback, self-update, post-state rescan; assert Full Gate does not modify source without explicit compatibility policy/approval. Include asset and environment inputs where required. Test source-rollup create/verify and isolated restore.

### U-PCC-A06 — UI consumption, independently testable

- Cortex standalone target retains global `Projects | Project Workspace | Chat` and right `Vault / Forge`. Project workspace uses Operations 10%, **Chat 40% instead of Dashboard**, **separate Console 40%**, Health 10%. Global Chat is conversation library and uses same persisted conversations. ForgeGUI is a reusable UI donor, never mandatory to local PCC CLI. Run Windows resizing/docking/keyboard/runtime tests before GUI_READY.

## 6. Acceptance / takeover policy

1. Clean project without existing PCC: explicit bootstrap only, accurate unresolved toolchains, preserves source. Freshly installed project can build/test/diagnose offline without Cortex, Forge or GUI.
2. Project with existing PCC: no overwrite until exact old/new command, gate, patch and source-control parity is verified; domain-specific commands preserved through namespace and adapter.
3. Mixed language and asset project: actual existing build targets and all mandatory gate stages executed; unknown/missing stages block; source+asset cert correctly invalidates on change.
4. Failed root patch: original bytes restored or recovery marked BLOCKED; failed package archived with reason, never repeatedly stuck in root or incorrectly reported Pending.
5. Cortex request: Inspect→Plan→Approve→Write→Verify→project PCC Build→Report on disposable fixture; no fabricated mutation, no recursive jobs, permission enforced at backend.
6. ForgeGUI UI, Cortex standalone app and CLI operate independently; same operation ID/evidence yields same truthful result. Closing a panel does not terminate host job unexpectedly.
7. SourceQuay: all requested repos inventoried; refs verified; LFS/wiki/release omissions reported; isolated restore tested; mirroring is separate from authorized publish.
8. Rust Forge may replace ForgePY **only after parity, Windows gate, clean install/recovery and explicit user approval**. A commit message or source GREEN is insufficient.

## 7. Immediate artifact and next required evidence

This report is an **audit/specification, not a root-drop patch**. No repository was modified; no local Windows build or ForgeGUI native test was executed during this review. For safe implementation, preserve the now-published Cortex `2ecf5b1` source and capture exact newer ForgeGUI Core/Forge Control Center full source plus its PCC receipts and current ForgePY source/version. Then implement A01–A03, produce one checksum-guarded patch for the actual owning repository, and test in an isolated fixture before Windows acceptance.

**First gate to pass:** the same `full` command dispatched from the Cortex GUI, Cortex AI, Forge GUI and a headless project-local CLI resolves to the same project-specific provider/stage graph and produces one source-bound operation record. No duplicate full-gate engine is introduced.
