# R7E Portable Project Build Authority

This establishes the environment contract for projects stored under the portable volume.

A project keeps its own `project.control.json`, internal PCC and source. Cortex supplies a common child-process environment:
- stable VolumeId / discovered mount root
- shared Cargo home and sccache
- project-namespaced Cargo target directory
- npm/pip/Gradle/NuGet/vcpkg caches
- volume `Artifacts/projects/<ProjectId>` evidence authority
- portable toolchain paths when installed, machine tools as fallback
- `RUSTUP_AUTO_INSTALL=0` so a build does not silently hydrate an unexpected Rust toolchain

Nothing in this pass changes a project's build definition.

Run:
`python tools\control\CortexProjectBuildDoctor.py --root <Volume>\projects\<project>`

The next integration slice replaces PCCSharedEnvironment's legacy Vault-derived cache logic with this contract and makes PCCAutoAdapter inject it for every dispatched project command.
