Cortex R8C portable MSVC activation fix

Target state: Cortex R8B portable source on Windows.
Extract this archive over the Cortex source root (for example E:\Cortex) and overwrite files.

Then run:
  HYDRATE_CORTEX_BUILD_TOOLS.cmd

The hydrator installs/validates Visual Studio 2022 C++ Build Tools and persists VsDevCmd.bat amd64 activation into .cortex\bootstrap-env.cmd. It must resolve both link.exe and cl.exe before reporting success.

After it passes, close/reopen Project Control Center and run FULL QUALITY GATE / CERTIFY GREEN.
