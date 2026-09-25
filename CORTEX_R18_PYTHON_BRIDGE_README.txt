Cortex R18 portable Python bridge repair

Purpose:
- Export the shared-drive Python as CORTEX_PYTHON_EXE.
- Prepend its directory to PATH for native project PCC.cmd/.bat descendants.
- Preserve the environment through the GUI surface, operation host, and auto adapter.
- Discover versioned Python dynamically under <drive>:\shared\toolchains\python\*\python.exe.
- Honor CORTEX_SHARED_ROOT / PCC_SHARED_ROOT overrides.
- Do not set PYTHONHOME and do not require a second machine-local Python install.

Expected Havenwild_Bevy result after applying/restarting Cortex:
  CORTEX_PYTHON_EXE=E:\shared\toolchains\python\3.14.7\python.exe
  where python -> E:\shared\toolchains\python\3.14.7\python.exe
  E:\Source\Havenwild_Bevy\PCC.cmd full proceeds past its former Python-not-found bootstrap.

Validation performed in packaging environment:
- py_compile PASS for all four Python modules.
- shared-root discovery/PATH injection unit smoke PASS.
- Windows/Havenwild FULL gate not claimed; must be run on the target Windows machine.
