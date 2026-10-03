# Cortex

Cortex is the governed development-intelligence layer for the portable Forge/Cortex development environment. It combines project-aware chat, project operations, build/test/run control, Git and patch workflows, Vault/storage authority, model/provider routing, execution evidence, and recovery tooling behind one project control surface.

## Current product surface

`PROJECT_CONTROL_CENTER.cmd` is the supported entry point for the current production control surface. It launches the PCC application, which exposes Projects, Cortex Chat, Project Console/Workspace, source/patch operations, diagnostics, build/test gates, and project registration.

The former standalone `cortex_desktop` application is retained only as donor/compatibility source while shared controller capabilities are converged into Forge Rust. Do not use it as the primary Cortex UI.

Forge Rust under `products/forge-rust/` is the permanent host direction. It is substantial candidate source and must be target-machine build/runtime certified before replacing the current production PCC/ForgePY surface.

## Normal workflow

1. Launch `PROJECT_CONTROL_CENTER.cmd`.
2. Select or register the project you want Cortex to operate on.
3. Use **Chat** for natural-language project work, **Inspect/Plan** for read-only analysis, and **Apply/Repair** for explicit transactional modes.
4. Use Project Console for detailed operational output and registered project commands.
5. Run QUICK during iteration and FULL when the milestone is ready for certification.
6. Commit/push only after the intended source state is verified.

Cortex Chat may classify an ordinary natural-language request as governed developer work. It is not a read-only chat surface. Mutating work remains subject to the project transaction, permission, validation, and evidence systems.

## Live execution visibility

PCC projects the safe Cortex execution snapshot while work is active: execution mode/phase, elapsed time, current action, active tool/target, model/iteration where available, dependency grounding, and next expected action. Project Console keeps the detailed operational stream. These are operational events only; private model reasoning is never exposed.

## Portable storage authority

Cortex is designed to run from a marked portable development volume without depending on a fixed Windows drive letter. The volume marker and Cortex storage resolver are authoritative. Runtime state belongs outside governed source, primarily under `.cortex/` or the configured portable/user-local authority.

Legacy PCC presentation-cache state under `data/conversations` is migrated to `.cortex/conversations` and is excluded from source rollups.

## Repository layout

Governed source is limited by `config/cortex/repository_layout.v1.json`. Generated state such as `.cortex/`, `.project_control/`, `artifacts/`, build outputs, logs, model data, and patch transports is operational state, not source.

Fast/FULL gates normalize known historical root residue and fail closed when canonical repository-layout violations remain.

## Source recovery

Create a source-only recovery archive through PCC or:

```text
python tools/control/CortexSourceRollup.py create --root <CortexRoot>
```

The rollup excludes runtime/operational state, build outputs, model files, transports, secrets, and legacy `data/conversations` / `data/registry` state. The generated manifest records file hashes and source-state metadata for verification.

## Verification

Primary project certification happens on the target Windows development environment through PCC FULL. A source handoff or audit should not be described as GREEN until its required Rust/Cargo/Windows runtime checks have completed successfully on that environment.

Useful commands:

```text
PROJECT_CONTROL_CENTER.cmd
PROJECT_CONTROL_CENTER.cmd --cli
python tools/control/CortexPCC.py status --root <CortexRoot>
python tools/control/CortexPCC.py quick --root <CortexRoot>
python tools/control/CortexPCC.py full --root <CortexRoot>
python tools/control/CortexPCC.py root-hygiene --root <CortexRoot>
python tools/control/CortexPCC.py root-hygiene-fix --root <CortexRoot>
```

## Near-term convergence target

The current functional-alpha target is an end-to-end governed workflow in which Cortex can create/register a small project, edit it, build it, run it, show useful live execution status, repair a controlled failure, verify the result, and retain evidence. Native image generation and resource-aware local-model routing are the next major product lanes after that execution path is certified.
