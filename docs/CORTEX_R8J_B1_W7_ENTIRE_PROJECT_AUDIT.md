# Cortex R8J-B1-W7 — Entire-project source, integration and certification audit

Date: 2026-09-26. Status: **local reconstructed-source audit; Windows FULL/live-drive acceptance outstanding**.

## Authority and scope

- Published GitHub source at audit start: `shifty81/Cortex` `main` commit `82d41230673ef2a179dc6fb2f1918493ad68837e` (published R8J-A, not B1/W6). GitHub main was read, not modified.
- Audit working tree: user-supplied `Cortex (2).zip` plus supplied R8A–R8J-A incremental source lineage and the exact W6 cumulative replacement reconstructed in `/mnt/data/r8j_w6_work/repo`, then W7 test/document changes. W6's 19 payload bytes match their manifest and reconstructed files. Unchanged historical files were **not** claimed byte-for-byte identical to GitHub main. The older September 7 standalone handoff and September 23 U01R1 rollup were treated as provenance, not current runtime authority.
- Inventory in the user's external drive is **not** included in the source ZIP or this audit: historical GUI evidence shows 2,238,746 entries and 278 access gaps. No scan, rebuild, migration, or live index query was performed here.
- This review extends across both desktop applications, Python PCC, Rust Cortex, independent Rust Forge candidate, project registry/discovery, Vault, Git/patch/permissions, AI agent/provider path, tests, documentation and release/recovery. It is neither a penetration test nor proof that all Windows UI and provider workflows run successfully.

## Independently reproduced issue and W7 correction

W6's five control regression cases still hard-coded `PCC-GUI-0.15.5e` while its actual GUI constant and patch manifest declare `PCC-GUI-0.15.5f`. An independent `unittest discover` of `tools/control/tests` therefore returned five assertion failures out of 284 tests, despite W6 documentation reporting that suite green. They were version-contract test drift, not observed GUI crashes. W7 removes the historical literal from unrelated tests, adds a dedicated test requiring one canonical GUI version matching `docs/R8J_B_PATCH_MANIFEST.json`, and leaves the GUI source at `0.15.5f` (no runtime GUI change). It also fixes an independently verified documentation/contract drift: FULL marks the source fingerprint but does not invoke deep Vault mirroring or recovery verification, so the quality-gate contract and project label now describe the actual implemented gate. This guards against recurring version bumps requiring edits in five unrelated behavior tests. The previous W6 archive is superseded; do not install W6 and W7 together.

## Source and test inventory (W7 reconstruction)

| Surface | Observed state | Confidence/limit |
|---|---|---|
| Root Cortex Rust workspace | 49 declared crates/apps; each member Cargo manifest exists; no duplicate package names | Static TOML, **not** Cargo compile in Linux audit environment |
| `products/forge-rust` | Separate 21-member workspace with own `forge-rust-gate` | Not included in root `cargo --workspace` gate; takeover matrix explicitly remains candidate |
| Python/JSON/TOML | All governed Python AST, JSON and TOML parse; detailed counts emitted by local audit script | Syntax/manifest validity, not functionality or Windows PowerShell AST |
| Python FULL | Explicit mandatory fixtures plus discovery of `tools/control/tests`, `tests`, `tests/control`, root staging and `tools/pcc/tests` | Run against reconstructed source; platform-dependent symlink fixture skipped on Linux |
| GUI | Tk PCC/Forge surface and separate native Rust Cortex desktop both exist | Headless Tk smoke and native Windows GUI interaction are separate evidence categories |
| Reusable event/job crates | `cortex_activity` and `cortex_task` are intentional compatibility facades re-exporting `cortex_jobs` | Do not count thin facades as incomplete implementations |
| Patch | W7 contains only declared repo-relative files plus its manifest | No live database, cache cleanup, asset/model content, user project move or automatic registration |

## Subsystem gap matrix

| Domain | Present source/authority | What is not yet proven or integrated | Next acceptance / owner |
|---|---|---|---|
| PCC and operations | Python PCC, typed project contract, patch preflight, Git authority, source fingerprint, debug bundle | FULL currently does not invoke the separately implemented Vault mirror/deep verification, contrary to prior documentation; source GREEN is not verified recovery | First correct documentation/typed label (W7); then implement source-stable mirror + deep verify + recovery receipt with failure fixtures |
| Cortex desktop | Rust CLI, desktop controller, native UI, workbench/chat and controlled project tools | Startup, high-DPI layout, large-chat responsiveness, process teardown and real provider/tool cycles on current Windows version | Runtime smoke with exact PID/window, provider round-trip, persistent conversation, structured artifact rendering |
| Chat/console | Python GUI supports explicit `/cortex`, `/inspect`, `/plan`, `/apply`, `/repair`, `!` shell and project command routing through Python bridge | A live broker fallback is not evidence the installed Rust worker or full transactional agent actually executed. Operator needs explicit execution mode/evidence in UI | Instrument intent → authority → tool → result → persisted transcript and failure case |
| Agent/permissions | Project-aware roles, default-deny permission contracts, transaction/rollback, validation barriers, mutation tools | Real malicious-input and destructive-action adversarial suite beyond static/synthetic fixtures not demonstrated; `cortex_review` contains two built-in trace-eval cases | Runtime adversarial fixtures and durable approval/event correlation |
| Models/providers | Native/LM Studio provider routing, tool-response replay, image and ComfyUI interfaces, model-host catalog | Real configured model endpoint, schema interoperability, cancellation, timeouts and restart behavior unverified by source gate | Provider acceptance matrix per active installation; do not infer from static tests |
| Persistent volume inventory | Dedicated metadata-only SQLite, paged browsing, access-gap reporting, scoped and recursive refresh, W6 live-worker scan lease | No live `G:`/`D:` acceptance here; SQLite online backup/recovery; PID reuse/start-time lease identity and stale-lease tooling remain | Read-only audit on real 2.24M index; crash/restart/lease trials; governed online backup |
| Project discovery/registration | Rust `cortex_discovery` bounded read-only filesystem traversal; Rust registry/project projections; Python portable PCC registry | No verified Rust consumer of Python inventory SQLite; independent registries require cross-language identity reconciliation and proof for nested/duplicate copies | R8J-B2 versioned read-only candidate feed keyed by `(volumeId, relativeRoot)`; no silent registration |
| Organization Review | Git/source review and registry candidate/classification primitives | No unified approval queue for new/moved/renamed/unclassified drive content; `cortex_review` is not automatically this workflow | R8J-B3 proposal evidence + approve/defer/reject journal; source move separate approval |
| Vault/caches | PCC Vault CAS/mirror/storage health, Rust `cortex_vault`, project namespaces | Unified index-to-Vault projection, operational backup restore and orphan/reclaim dry-run end-to-end acceptance not yet proven on real drive | No catalog rewriting during discovery; source-stable mirror/recovery tests |
| Git and portability | GREEN source Git authority, trust status, registry rebasing, local Forgejo tools, remote GitHub | Portable identity parity for Python and Rust together across actual `G:`↔`D:` with two independent same-name projects; no bulk remote mutations | Two-device integration fixtures and real Git trust/commit/push acceptance |
| Rust Forge candidate | Separate `products/forge-rust` source and explicit gate/parity matrix | Root Cortex FULL does **not** imply Forge candidate GREEN or takeover. Full UI, system service, update, storage and installed parity remain documented gaps | Run separate Forge gate and side-by-side ForgePY/PCC takeover checklist; keep Python production authority |
| Delivery and CI | Windows PCC local gate, source fingerprints, archive manifests, recovery processes | No `.github/workflows` file present in reconstructed project source; signing/installer/updater/self-recovery parity unverified | Add reproducible CI after local authority remains green; keep signed release gate separate |
| Source hygiene/docs | Broad architecture docs, source rollup and patch manifests; current bootstrap/root marker policies | Supplied source ZIP contains ignored generated artifacts/logs and `tools/control/PCCSurfaceCommon.py.fixpayload`; source docs include older D-drive layout examples | Audit actual Git tracking and usage before quarantine/removal; do not silently delete historical evidence |
| Maintainability | 49 Rust packages plus Python control modules have concrete source and tests | Large single files (`cortex_desktop_core` ~9.2K lines; `cortex_desktop_native` ~8.8K combined; `CortexPCCGui.py` ~233 KB) concentrate unrelated responsibilities and increase regression risk | Extract behind tested authority boundaries after W7 acceptance; no premature architecture rewrite |

## Priority sequence

**P0 — certify accurate existing behavior and close the recovery contract gap:** install only W7 over the exact published R8J-A baseline, check GUI version remains `0.15.5f`, run Windows FULL (now its newly discovered test-version assertions must pass); preserve reported failure evidence if not. With GUI idle, run read-only `CortexInventoryIndexAudit.py --cortex-root . --verify-counts --json`, then small-project tree refresh, pause/resume, errors and storage health without rebuilding. Run separate Rust Forge gate when working on takeover; do not conflate the two.

**P1 — first new architectural implementation (R8J-B2/B3):** connect indexed metadata to one canonical project-candidate boundary; preserve nested projects/composites, independent copies and stable portable identities; show proposals with provenance, confidence, exact source paths and explicit approvals. No automatic moves, merges, registrations or remote changes. Reuse the existing registry/PCC contracts instead of making a second project DB.

**P1 — operational reliability (R8J-C):** background incremental refresh from detected changes, volume-disconnected versus deleted distinctions, durable change journal, PID/start-time worker identities, SQLite online backup and restore drill, query plan/latency budget on live index. Continue source-agnostic provider/GUI runtime acceptance.

**P2 — product convergence:** clarify the primary Forge landing/project manager UX versus independent native Cortex desktop and the Python PCC surface; normalize one command/operation/event transcript and project context across them. Split large modules without duplicating authority. Certify Rust Forge side-by-side before any operational takeover. Then release/installer/update/signing workflows.

## What this audit deliberately does not claim

- The W6 ZIP itself was green in complete control discovery: it was not; five stale-version assertions failed independently.
- W7 has passed Windows FULL, Cargo check/clippy/test/build, real providers or live 2.24M-row database validation: pending on the user's workstation.
- Rust project discovery already queries the Python SQLite index or Organization Review already safely onboards everything: those are missing integration milestones.
- The September 7/23 handoff files represent the latest running source; they are historical context only.

## W7 local test evidence at packaging

- Selected historical mandatory Python fixtures: **196 run / 1 skipped / 0 failures**.
- Complete `tools/control/tests` discovery: **287 run / 0 failures** (W6 independently had 284 run / 5 failures; W7 adds two version-contract cases).
- Complete `tests` discovery: **72 run / 1 skipped / 0 failures**.
- Nested project-control discovery: **4 run / 0 failures**; repository-root staging: **5 run / 0 failures**; separate PCC provider discovery: **5 run / 0 failures**.
- The suite outputs were observed individually; an enclosing combined test-run invocation exceeded the tool execution timeout even though its individual child-suite logs later showed each stage finishing. Do not assert a successful terminal return from that enclosing invocation. The Windows PCC FULL still determines source certification.
- Static review of the governed reconstructed tree: **97 Python files**, **152 JSON files**, **74 TOML files** parsed without errors before adding this audit's JSON gap file (153 JSON including it). Root 49 members and nested Forge 21 members all had Cargo.toml files and unique names. No Rust toolchain was installed in this Linux container, so no Cargo fmt/check/test/clippy/build was attempted here.

## Additional W7 source/contract alignment finding

`docs/QUALITY_GATE_CONTRACT.md` previously listed Vault capture and deep SHA-256 verification as *implemented* FULL steps, and `project.control.json` described FULL as adding Vault certification. Actual `GateEngine.full()` calls the Python fixture chain, Cargo stages and `GitAuthority.mark-green`; its Git implementation records only a source fingerprint. The Vault mirror and `vault-verify` commands are separately registered but are not dispatched by that path. W7 corrects the documentation and gate label to the actual behavior and adds one test that will detect accidental reintroduction of the unsupported promise. Deep mirror/recovery certification remains a future implementation and should be tested fail-closed before restoring the stronger GREEN contract. No existing mirror or runtime state is modified by W7.
