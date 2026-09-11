# FR02 — Project Contract Authority + Capability Discovery

FR02 hardens the interoperability boundary shared by ForgePY, project-owned PCC providers, Cortex, and the parallel Rust Forge candidate.

## Implemented

- `forge.project.v1` now requires an explicit supported schema and `schema_version`;
- project identity, command keys/programs, quality-gate stage references, artifact keys/paths, and provider/launcher paths fail closed when invalid;
- duplicate command, gate, and artifact identities are rejected;
- canonical project-provider operation aliases moved into `forge-contracts` so Forge consumers cannot silently drift;
- `forge.capabilities.v1` exposes project identity, contract version, provider readiness, canonical provider operations, direct project commands, gates, and artifacts;
- `forge-contract-inspect --root <project>` emits capability discovery JSON without launching the GUI;
- `forge-core::ProjectSession::capabilities()` exposes the same typed discovery surface to the native application.

## Authority boundary

FR02 does not make Rust Forge production authority. ForgePY remains the active universal controller. Projects remain independently buildable and retain their own project-local PCC/CLI spine. The contract is the bridge; ForgePY is not vendored into Cortex or project repositories.

## Next pass

FR03 should add durable operation identities/receipts, cancellation/stop authority, and session log persistence. Operation receipts should bind project ID, contract version, operation key, resolved provider/direct command, start/end timestamps, exit state, and produced artifact references.
