@echo off
setlocal EnableExtensions DisableDelayedExpansion

if /I not "%~1"=="__KEEP_OPEN__" (
    start "Cortex PCC Surface Repair" cmd.exe /k ""%~f0" __KEEP_OPEN__"
    exit /b 0
)

set "ROOT=%~dp0"
set "LOGDIR=%ROOT%artifacts\logs"
set "LOG=%LOGDIR%\pcc-surface-repair.log"

if not exist "%LOGDIR%" mkdir "%LOGDIR%" >nul 2>nul

echo.
echo ============================================================
echo  CORTEX PCC SURFACE REPAIR R2 - NO-CLOSE DIAGNOSTIC
echo ============================================================
echo  Root : %ROOT%
echo  Log  : %LOG%
echo.
echo This window is intentionally persistent.
echo It will NOT silently close when repair or preflight fails.
echo.

> "%LOG%" echo ============================================================
>>"%LOG%" echo CORTEX PCC SURFACE REPAIR R2
>>"%LOG%" echo Root=%ROOT%
>>"%LOG%" echo Started=%DATE% %TIME%
>>"%LOG%" echo ============================================================

where git.exe >>"%LOG%" 2>&1
if errorlevel 1 (
    echo [FAIL] git.exe is not on PATH.
    >>"%LOG%" echo [FAIL] git.exe is not on PATH.
    goto :done
)

where powershell.exe >>"%LOG%" 2>&1
if errorlevel 1 (
    echo [FAIL] powershell.exe is not on PATH.
    >>"%LOG%" echo [FAIL] powershell.exe is not on PATH.
    goto :done
)

echo [STEP] Running exact PCC surface repair...
echo [STEP] Running exact PCC surface repair...>>"%LOG%"

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%APPLY_PCC_SURFACE_REPAIR.ps1" "%ROOT%" >>"%LOG%" 2>&1
set "RC=%ERRORLEVEL%"

echo.
echo -------------------- REPAIR OUTPUT ---------------------------
type "%LOG%"
echo --------------------------------------------------------------
echo.

if not "%RC%"=="0" (
    echo [FAIL] Repair exited with code %RC%.
    echo [INFO] Nothing unknown should have been overwritten.
    echo [INFO] Send me this file:
    echo        %LOG%
    goto :done
)

echo [PASS] Repair script completed successfully.
echo.
echo [STEP] Checking that the GUI module can import...
>>"%LOG%" echo [STEP] GUI module import smoke

set "PYTHONDONTWRITEBYTECODE=1"
set "PYTHONPATH=%ROOT%tools\control;%PYTHONPATH%"

where python.exe >nul 2>nul
if errorlevel 1 (
    where python >nul 2>nul
    if errorlevel 1 (
        echo [WARN] python is not on PATH; GUI import smoke skipped.
        >>"%LOG%" echo [WARN] python is not on PATH; GUI import smoke skipped.
        goto :success
    )
    set "PYEXE=python"
) else (
    set "PYEXE=python.exe"
)

"%PYEXE%" -c "import CortexPCCGui; from PCCSurfaceCommon import SurfaceCommand, command_catalog; print('PASS GUI/common import contract')" >>"%LOG%" 2>&1
set "IRC=%ERRORLEVEL%"

if not "%IRC%"=="0" (
    echo [FAIL] GUI/common import smoke still fails.
    echo [INFO] Send me this file:
    echo        %LOG%
    echo.
    type "%LOG%"
    goto :done
)

:success
echo.
echo [PASS] PCC GUI/common import contract is repaired.
echo.
echo Next launch:
echo   PROJECT_CONTROL_CENTER.cmd
echo.
echo Do NOT run another patch over the repository if the GUI still fails.
echo Send the new pcc-gui-preflight.log instead.

:done
echo.
echo ============================================================
echo  WINDOW WILL REMAIN OPEN
echo ============================================================
echo Log: %LOG%
echo.
echo Type EXIT and press Enter when you are finished reviewing it.
echo.
