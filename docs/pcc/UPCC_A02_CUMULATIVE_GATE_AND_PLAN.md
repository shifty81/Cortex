# U-PCC-A02 — Cumulative universalization checkpoint

**Target:** Cortex `main` commit `2ecf5b1b2c98b5e571504b892c4b4649659ced02`, with SOURCE01–SOURCE03R3 already published. This is a **source-base-specific cumulative PCC root-drop**, **not a full source rollup**. It includes every payload from U-PCC-A01 plus A02, and requires none of A01's receipts when installed directly on the specified published baseline. If A01 was previously applied, use the separate A02-only variant with A01 as an explicit dependency and A01 exact preimages. Never queue both variants.

## Implemented now

1. Preserve the U-PCC-A01 read-only repository inventory and semantic audit, including bounded scanning and unresolved-gate checks.
2. Introduce `tools/control/UniversalPCCPlan.py`: any repository with `project.control.json` can request a read-only, machine-readable gate plan with exact command order, program availability, project-contained working directories, risks, policies, manifest hash and gate-definition fingerprint. Status is `RESOLVED_NOT_TESTED`, not GREEN. No unregistered build command is guessed or executed.
3. Add `CortexPCC.py universal-plan --root . [--project-root OTHER] [--gate-key full] [--rust-consumer]` before Cortex operational bootstrapping. The command neither mutates other repositories nor creates session artifacts.
4. Strengthen Cortex Quick Gate's JSON contract stage: rejects duplicate JSON fields, unsupported schema, malformed command/risk/cancellation metadata, and missing gate stages. Legacy rollback descriptors remain WARN for historical Python-only consumers, explicitly **not** Rust-ready; the underlying manifest is not rewritten.
5. Add mandatory Python controller/inventory/plan/provider tests to Full Gate **before** Cargo stages or GREEN. Missing test fixtures or provider suites fail closed. Previous self-test action remains backward compatible.
6. Desktop launch preflights the project against typed Rust contract **before** rebuilding or spawning; an invalid rollback descriptor now fails with useful errors. When valid, capture stdout/stderr in `artifacts/logs/runtime/`, detect immediate process exit and report *process running, GUI readiness not certified*. No false UI-ready claim.
7. Include the standalone GUI normalization spec and the universal PCC consolidation audit in this patch as **design/reference documentation**, not falsely implemented UI features.

## Explicitly not implemented, and why

- The existing Cortex manifest contains `git_or_snapshot` and `process_stop`, neither represented by the four typed Rust rollback variants. Mapping these to another string without implementing corresponding rollback, process ownership, and recovery would discard a safety guarantee. Desktop launch will correctly refuse until a separate explicitly reviewed semantic migration is supplied. **Do not substitute `none` or `provider_owned` silently.**
- GUI 10/40/40/10, global Chat home, Projects Remove button relocation, right-hand Health and native conversation wiring require a source-exact GUI patch. Existing GUI was not modified in this pass. The imported GUI spec locks the approved direction.
- A cross-project command executor and shared operation service with streamed/cancelable operation IDs are not yet implemented. This pass is nonmutating discovery and gate planning only; the original project-owned PCC remains command and certification authority.
- SourceQuay is a *proposed* display and source-history system name, not an applied rename or actual GitHub mirror. Current local remotes and history are untouched.
- ForgePY remains active universal manager; Rust Forge is shadow/candidate until Windows parity and explicit approval. No external repo or D: drive was accessed during patch creation.
- Full Gate has a stricter set of tests after A02. A previous GREEN receipt does not certify the new source or new gate; rerun the Full Gate after installation. We did not run Windows Cargo/desktop or actual user repository gate here.

## Commands after successful PCC application/restart

```powershell
python tools/control/CortexPCC.py universal-audit --root .
python tools/control/CortexPCC.py universal-plan --root . --gate-key full
python tools/control/CortexPCC.py universal-plan --root . --project-root "D:\\Projects\\OtherProject" --gate-key full
python tools/control/CortexPCC.py universal-plan --root . --rust-consumer
python tools/control/CortexPCC.py full --root .
```

`universal-audit` and `universal-plan --rust-consumer` currently exit 2 for Cortex's two legacy rollback declarations. Run `universal-plan` without that strict switch to get an honest Python-provider plan. `launch-gui` now returns 2 before launching for the incompatible contract. The Full Gate still tests SOURCE only and will issue its SOURCE GREEN only if Python and Rust stages pass; this is not GUI/runtime/release certification.

## Source-exact installation safeguards

- Cumulative package: requires current published `CortexPCC.py` SHA-256 `9eb296cb0c1aff02b77632302d5d4d1e410ddd1e69fba5bd3d717787aeaaebb0`, and requires every A01/A02 new path to be absent. Does not require A01 receipt.
- Incremental package: requires A01 `CortexPCC.py` SHA-256 `152ab125d9aa82961e2f6d8c96c22adc182a131559232530dce4957e07f4fa75`, and requires the A01 receipt. It does not redeclare already installed A01 files.
- Existing `.git`, local work, project configuration and ForgeGUI source are not modified by either package. `PATCH_MANIFEST.json` inside the ZIP is transport metadata, not a source file.
- Do not put the cumulative and incremental variants, or A01 and cumulative A02, in the same patch queue. If a checksum mismatch occurs, stop and rebase on the actual local files rather than bypassing the check.

## Next pass: U-PCC-A03

1. Capture and compare exact GUI/backend source, and trace GUI → registered provider → operation/receipt; publish a source-exact GUI implementation for approved Projects/Workspace/Chat/Vault/Health layout.
2. Implement a semantically correct rollback policy conversion with tests for `fmt.apply` snapshot and `forge.rust.run` process ownership; typed-deserialize full manifest, then certify desktop window-visible handshake.
3. Normalize one shared operation ID, JSONL events, cancellation and diagnostics across local PCC, ForgePY and Cortex; never manufacture a second execution authority.
4. Expand parity fixtures across Cortex, ForgePY, Subspace, Havenwild and Ember using exact source snapshots; only then extract reusable components. Existing project-specific PCCs remain independently operable.
