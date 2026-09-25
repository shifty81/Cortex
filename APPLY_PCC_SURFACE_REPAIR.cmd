@echo off
setlocal EnableExtensions DisableDelayedExpansion
set "ROOT=%~dp0"
echo.
echo ============================================================
echo  CORTEX PCC SURFACE REPAIR
echo ============================================================
echo  Target: %ROOT%
echo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%APPLY_PCC_SURFACE_REPAIR.ps1" "%ROOT%"
set "RC=%ERRORLEVEL%"
echo.
if not "%RC%"=="0" (
  echo [FAIL] PCC surface repair did not apply.
  echo Nothing unknown should have been overwritten.
  pause
  exit /b %RC%
)
echo [PASS] Repair completed.
echo.
echo Now launch:
echo   PROJECT_CONTROL_CENTER.cmd
echo.
pause
exit /b 0
