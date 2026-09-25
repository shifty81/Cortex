CORTEX R20 — STARTUP CONTRACT STABILIZATION — CUMULATIVE OVER R18/R19
2026-09-24

Purpose
-------
Stop regression patching and restore a strict launcher contract.

Confirmed defect in R19
-----------------------
R19 stored portable Python commands using backslash-escaped quotes such as:
    set "PY_CMD=\"E:\...\python.exe\""
Windows cmd.exe does not use backslash as its quote escape. That made the root
bootstrap command representation unsafe/incorrect.

R20 corrections
---------------
1. Keeps the R18 shared/portable Python bridge for child project operations.
2. Replaces PY_CMD string composition with PY_EXE + PY_ARGS.
3. Bare PROJECT_CONTROL_CENTER.cmd launch has ONE meaning: launch Cortex PCC GUI.
4. GUI preflight failure is RED and stops. It does NOT silently enter CLI.
5. CLI requires --cli / cli (or the existing explicit FORCE_CLI environment switch).
6. FULL/build/test/debug-bundle remain explicit commands only.
7. GUI startup failure does not open the debug-artifact folder or imply FULL ran.
8. Adds a static launcher-contract regression test.

Required Windows certification
------------------------------
A. Double-click PROJECT_CONTROL_CENTER.cmd -> GUI appears; no CLI; no FULL.
B. PROJECT_CONTROL_CENTER.cmd --cli -> CLI appears.
C. PROJECT_CONTROL_CENTER.cmd status -> status only.
D. PROJECT_CONTROL_CENTER.cmd full -> FULL only.
E. GUI -> Havenwild_Bevy -> FULL -> routes to Havenwild native PCC with portable Python.

Do not claim GUI GREEN until A is observed on Windows.
