@echo off
setlocal EnableExtensions DisableDelayedExpansion

if /I not "%~1"=="__KEEP_OPEN__" (
    start "Cortex PCC Surface Repair R3" cmd.exe /k ""%~f0" __KEEP_OPEN__"
    exit /b 0
)

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

set "LOGDIR=%ROOT%\artifacts\logs"
set "LOG=%LOGDIR%\pcc-surface-repair-r3.log"

if not exist "%LOGDIR%" mkdir "%LOGDIR%" >nul 2>nul

> "%LOG%" echo ============================================================
>>"%LOG%" echo CORTEX PCC SURFACE REPAIR R3
>>"%LOG%" echo Root=%ROOT%
>>"%LOG%" echo Started=%DATE% %TIME%
>>"%LOG%" echo ============================================================

echo.
echo ============================================================
echo  CORTEX PCC SURFACE REPAIR R3
echo ============================================================
echo  Root : %ROOT%
echo  Log  : %LOG%
echo.
echo Trailing repository slash has been removed before PowerShell.
echo This window remains open.
echo.

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\APPLY_PCC_SURFACE_REPAIR_R3.ps1" "%ROOT%" >>"%LOG%" 2>&1
set "RC=%ERRORLEVEL%"

echo -------------------- REPAIR OUTPUT ---------------------------
type "%LOG%"
echo --------------------------------------------------------------

if not "%RC%"=="0" (
    echo.
    echo [FAIL] Repair exited with code %RC%.
    echo [INFO] Nothing unknown should have been overwritten.
    echo [INFO] Log: %LOG%
    goto :done
)

echo.
echo [PASS] Repair and GUI/common import smoke completed.
echo.
echo Now run:
echo   %ROOT%\PROJECT_CONTROL_CENTER.cmd
echo.

:done
echo ============================================================
echo This window will remain open.
echo Type EXIT and press Enter when finished.
echo ============================================================
echo.
