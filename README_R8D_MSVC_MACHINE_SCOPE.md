# Cortex R8D — MSVC Machine-Scope Hydration Repair

R8D corrects the Windows C++ Build Tools boundary discovered on a removable E: Vault.

- The Cortex Vault remains the external-drive authority (for example `E:\`).
- Python, Git, Rust, Cargo caches, downloads, project mirrors, and CAS remain Vault-governed.
- Visual Studio 2022 C++ Build Tools are treated as a machine-scoped Windows dependency.
- The Build Tools bootstrapper is cached in the Vault, but the Visual Studio product is no longer forced onto the removable drive.
- Build Tools hydration requests UAC elevation before setup.
- Setup runs with passive progress and `--wait`.
- Exit codes are recorded and interpreted.
- Visual Studio setup logs written during the operation are copied into `artifacts\bootstrap\visual-studio-setup`.
- The workload explicitly requests `Microsoft.VisualStudio.Workload.VCTools` and `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`, with recommended components (including the Windows SDK selection supplied by the current workload).
- A successful result still requires `VsDevCmd.bat` to expose both `link.exe` and `cl.exe` in an amd64 developer environment.
- A legacy failed portable MSVC directory under the Vault is ignored, reported, and never automatically deleted.

Apply this overlay over the existing R8C/R8B source, then run:

`HYDRATE_CORTEX_BUILD_TOOLS.cmd`

A UAC prompt is expected. After the installer completes successfully, restart/open PCC and run FULL QUALITY GATE.
