Cortex R19 - PCC GUI bootstrap truth + portable Python authority
2026-09-24

Purpose
- Preserve GUI PCC as the normal no-argument PROJECT_CONTROL_CENTER.cmd launch.
- Prefer CORTEX_PYTHON_EXE when supplied by Cortex/R18.
- Discover shared\\toolchains\\python when the root PCC is launched before Cortex has populated its environment.
- Capture and display the real GUI preflight failure before falling back to recovery CLI.
- Preserve explicit --cli/cli and explicit headless command behavior.

Test
1. Overlay at E:\\Cortex.
2. Double-click PROJECT_CONTROL_CENTER.cmd with no arguments.
3. Expected: Cortex PCC GUI opens.
4. If preflight fails, the console now prints the actual reason and writes artifacts\\logs\\pcc-gui-preflight.log before entering recovery CLI.
5. Once GUI opens, select Havenwild_Bevy and rerun FULL to continue R18 portable-Python certification.
