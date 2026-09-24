@echo off
setlocal
set "ROOT=%~dp0"
set "DRIVE=%~d0\"
echo.
echo ============================================================
echo  CORTEX EXTERNAL DRIVE ROOT VAULT SETUP
echo ============================================================
echo  Repository : %ROOT%
echo  Vault root : %DRIVE%
echo.
echo This configures the ENTIRE drive root as Cortex Vault authority.
echo Cortex will create/use governed folders such as:
echo   %DRIVE%projects
echo   %DRIVE%objects
echo   %DRIVE%shared
echo   %DRIVE%quarantine
echo.
echo Unrelated files already on the drive are not moved or deleted.
echo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\Set-CortexVaultDriveRoot.ps1" -DriveRoot "%DRIVE%"
set "RC=%ERRORLEVEL%"
echo.
if not "%RC%"=="0" (
  echo Vault setup FAILED with exit code %RC%.
  pause
  exit /b %RC%
)
echo Vault setup complete. Close and reopen Cortex/PCC before running Full Gate.
pause
exit /b 0
