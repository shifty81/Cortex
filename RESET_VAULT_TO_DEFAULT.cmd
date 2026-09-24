@echo off
setlocal
set "ROOT=%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\Clear-CortexVaultRootOverride.ps1"
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" (
  echo Reset FAILED with exit code %RC%.
  pause
  exit /b %RC%
)
echo Restart Cortex/PCC to return to the default Vault policy.
pause
exit /b 0
