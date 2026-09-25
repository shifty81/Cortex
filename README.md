# Cortex PCC Build-Gate Recovery R1

Built from the exact `PROJECT_CONTROL_CENTER.cmd` captured in
`Cortex_DebugBundle_20260924-195210_FULL_FAIL.zip`.

Captured launcher Git blob:
`9f62406a0253dc1574c037bb5daf78e78659bd34`

Patched launcher Git blob:
`f828d6e170db9d4bf8df3787463a3b53769e7ef1`

## What the debug bundle proved

The FULL gate is currently stopping in QUICK at `windows-linker-toolchain`.
Cargo and Rust are present and Cargo metadata resolves the 49-package workspace,
but `link.exe` and `cl.exe` are missing from the PCC process environment.

The live fail-closed launcher removed the older startup environment bootstrap.
The older launcher called `scripts\Bootstrap-CortexEnvironment.ps1 -Mode Startup`
and then called `.cortex\bootstrap-env.cmd`. That generated handoff includes the
Visual Studio `VsDevCmd.bat` activation when MSVC Build Tools are installed.

R1 restores those two startup steps while preserving:
- GUI-first launch
- fail-closed GUI preflight
- no silent CLI fallback
- explicit `--cli` recovery

## Apply

Extract directly into the Cortex root and run:

`APPLY_PCC_BUILDGATE_RECOVERY_R1.cmd`

Then close all PCC windows, relaunch `PROJECT_CONTROL_CENTER.cmd`, and run
**QUICK** first.

If QUICK still reports `link.exe/cl.exe missing`, run the existing
`HYDRATE_CORTEX_BUILD_TOOLS.cmd` once, restart PCC, then QUICK again.

## Separate known blocker

Git on `G:\Cortex` is also currently rejected because of dubious ownership.
That is not changed by this patch. It needs an explicit checkout-trust/rebind
step after build-environment recovery.
