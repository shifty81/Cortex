Use only the .cmd file.

1. Download Cortex_PCC_Emergency_Surface_Fix.cmd.
2. Double-click it. Do NOT right-click -> Run with PowerShell.
3. It targets G:\Cortex automatically when that folder exists.
4. It writes a log immediately to:
   %TEMP%\Cortex_PCC_Emergency_Surface_Fix.log
5. The window pauses before closing on success or failure.
6. It modifies only PCCSurfaceCommon.py and only when the file exactly matches
   the known stale Git object. Unknown/newer files are not overwritten.
7. After PASS, run G:\Cortex\PROJECT_CONTROL_CENTER.cmd normally.
