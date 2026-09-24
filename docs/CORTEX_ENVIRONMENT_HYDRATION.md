# Cortex environment hydration

Cortex portable startup is bootstrapped by Windows PowerShell before Python is required.

## Startup contract

`PROJECT_CONTROL_CENTER.cmd` runs `scripts/Bootstrap-CortexEnvironment.ps1 -Mode Startup` first. The bootstrap:

1. resolves the governed Vault root;
2. prefers project-owned/shared toolchains when a drive-root or explicit Vault authority is configured;
3. checks Python, Git, Cargo/rustc, rustfmt, clippy and Windows MSVC build readiness;
4. hydrates missing lightweight/core toolchains into `<Vault>\shared\...` when network access is available;
5. writes `.cortex\bootstrap-env.cmd` so the PCC and all child processes inherit the same tools/caches;
6. writes `artifacts\bootstrap\environment-health.json` and `bootstrap.log`.

The portable core toolchains are shared by all projects using the Vault rather than copied into every repository.

### Portable locations

- Python: `<Vault>\shared\toolchains\python\3.14.7`
- Git MinGit: `<Vault>\shared\toolchains\git\mingit`
- rustup: `<Vault>\shared\toolchains\rust\rustup-home`
- Cargo home: `<Vault>\shared\dependencies\rust\cargo-home`
- pip cache: `<Vault>\shared\dependencies\python\pip-cache`
- npm cache: `<Vault>\shared\dependencies\node\npm-cache`

For an external-drive-root Vault such as `E:\`, these paths live directly under `E:\shared\...`.

## First-run commands

`BOOTSTRAP_CORTEX_ENVIRONMENT.cmd` performs explicit core hydration.

`HYDRATE_CORTEX_BUILD_TOOLS.cmd` additionally installs the large Visual Studio 2022 C++ Build Tools workload needed for Windows MSVC linking. It is intentionally separate so ordinary PCC startup never silently begins a multi-gigabyte Visual Studio installation.

## Security / provenance

Python is obtained from python.org. Rustup is obtained from static.rust-lang.org and verified against the official SHA-256 sidecar. MinGit is resolved from the latest stable Git for Windows GitHub release and its release digest is verified when the API exposes one. Download/install evidence remains under the governed shared Vault and repository bootstrap logs.
