@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "ROOT=G:\Cortex"
if not exist "%ROOT%\tools\control\PCCSurfaceCommon.py" (
    set "ROOT=%~dp0"
    if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
)

set "LOG=%TEMP%\Cortex_PCC_Emergency_Surface_Fix.log"
set "SURFACE=%ROOT%\tools\control\PCCSurfaceCommon.py"
set "GUI=%ROOT%\tools\control\CortexPCCGui.py"
set "COMMIT=d22b4f459c4fa095e42a6b9e3ae8951e170baae8"
set "STALE=f55bfe557a9f79aabc860dfd5b9af5a2fe55567e"
set "TARGET=c456b1c4b9dc8724834fe00c97ad6ebd55a2ba3c"

> "%LOG%" echo CORTEX PCC EMERGENCY SURFACE FIX
>>"%LOG%" echo Started=%DATE% %TIME%
>>"%LOG%" echo Root=%ROOT%
>>"%LOG%" echo Script=%~f0
>>"%LOG%" echo.

echo.
echo ============================================================
echo  CORTEX PCC EMERGENCY SURFACE FIX
echo ============================================================
echo.
echo Root : %ROOT%
echo Log  : %LOG%
echo.
echo This file uses CMD only. No PowerShell.
echo This window will pause before closing.
echo.

if not exist "%SURFACE%" (
    echo [FAIL] Missing "%SURFACE%"
    >>"%LOG%" echo [FAIL] Missing surface file.
    goto :finish
)

if not exist "%GUI%" (
    echo [FAIL] Missing "%GUI%"
    >>"%LOG%" echo [FAIL] Missing GUI file.
    goto :finish
)

where git.exe >nul 2>nul
if errorlevel 1 (
    echo [FAIL] git.exe is not on PATH.
    >>"%LOG%" echo [FAIL] git.exe not found.
    goto :finish
)

set "CURRENT="
for /f "usebackq delims=" %%H in (`git.exe hash-object -- "%SURFACE%" 2^>^>"%LOG%"`) do set "CURRENT=%%H"

echo Current blob : %CURRENT%
echo Stale blob   : %STALE%
echo Target blob  : %TARGET%
>>"%LOG%" echo CurrentBlob=%CURRENT%
>>"%LOG%" echo StaleBlob=%STALE%
>>"%LOG%" echo TargetBlob=%TARGET%

if /I "%CURRENT%"=="%TARGET%" (
    echo [PASS] PCCSurfaceCommon.py already matches target.
    >>"%LOG%" echo [PASS] Already target object.
    goto :importcheck
)

if /I not "%CURRENT%"=="%STALE%" (
    echo.
    echo [FAIL] Live PCCSurfaceCommon.py is neither known stale nor target.
    echo [FAIL] NOTHING WAS CHANGED.
    echo.
    echo Send me this log:
    echo   %LOG%
    echo.
    echo And send this file:
    echo   %SURFACE%
    >>"%LOG%" echo [FAIL] UNKNOWN_OR_NEWER object. No source changes made.
    goto :finish
)

echo.
echo [STEP] Checking exact d22 commit in local Git...
git.exe -C "%ROOT%" cat-file -e "%COMMIT%^{commit}" >>"%LOG%" 2>&1
if errorlevel 1 (
    echo [FAIL] Required commit is not in local Git history.
    echo [FAIL] NOTHING WAS CHANGED.
    >>"%LOG%" echo [FAIL] Required commit absent.
    goto :finish
)

set "TMP=%TEMP%\Cortex_PCCSurfaceCommon_%RANDOM%_%RANDOM%.py"
echo [STEP] Extracting exact matching common surface...
git.exe -C "%ROOT%" show "%COMMIT%:tools/control/PCCSurfaceCommon.py" > "%TMP%" 2>>"%LOG%"
if errorlevel 1 (
    echo [FAIL] git show failed.
    >>"%LOG%" echo [FAIL] git show failed.
    if exist "%TMP%" del /q "%TMP%" >nul 2>nul
    goto :finish
)

set "TMPHASH="
for /f "usebackq delims=" %%H in (`git.exe hash-object -- "%TMP%" 2^>^>"%LOG%"`) do set "TMPHASH=%%H"
echo Extracted blob: %TMPHASH%
>>"%LOG%" echo ExtractedBlob=%TMPHASH%

if /I not "%TMPHASH%"=="%TARGET%" (
    echo [FAIL] Extracted file failed exact-object verification.
    echo [FAIL] NOTHING WAS CHANGED.
    >>"%LOG%" echo [FAIL] Extracted hash mismatch.
    del /q "%TMP%" >nul 2>nul
    goto :finish
)

set "BACKUP=%SURFACE%.before-emergency-fix.bak"
copy /y "%SURFACE%" "%BACKUP%" >nul
if errorlevel 1 (
    echo [FAIL] Could not create backup:
    echo   %BACKUP%
    >>"%LOG%" echo [FAIL] Backup failed.
    del /q "%TMP%" >nul 2>nul
    goto :finish
)

echo [STEP] Replacing only PCCSurfaceCommon.py...
copy /y "%TMP%" "%SURFACE%" >nul
del /q "%TMP%" >nul 2>nul

set "AFTER="
for /f "usebackq delims=" %%H in (`git.exe hash-object -- "%SURFACE%" 2^>^>"%LOG%"`) do set "AFTER=%%H"
echo After blob   : %AFTER%
>>"%LOG%" echo AfterBlob=%AFTER%
>>"%LOG%" echo Backup=%BACKUP%

if /I not "%AFTER%"=="%TARGET%" (
    echo [FAIL] Verification failed. Restoring backup...
    copy /y "%BACKUP%" "%SURFACE%" >nul
    >>"%LOG%" echo [FAIL] Replacement verification failed. Backup restored.
    goto :finish
)

echo [PASS] Exact PCCSurfaceCommon.py replacement succeeded.
>>"%LOG%" echo [PASS] Exact replacement succeeded.

:importcheck
echo.
echo [STEP] Static symbol checks...
findstr /c:"class SurfaceCommand" "%SURFACE%" >nul 2>>"%LOG%"
if errorlevel 1 (
    echo [FAIL] SurfaceCommand is still missing.
    >>"%LOG%" echo [FAIL] SurfaceCommand missing.
    goto :finish
)
findstr /c:"def command_catalog" "%SURFACE%" >nul 2>>"%LOG%"
if errorlevel 1 (
    echo [FAIL] command_catalog is still missing.
    >>"%LOG%" echo [FAIL] command_catalog missing.
    goto :finish
)
echo [PASS] SurfaceCommand and command_catalog are present.
>>"%LOG%" echo [PASS] Required symbols present.

set "PY="
where python.exe >nul 2>nul
if not errorlevel 1 set "PY=python.exe"
if not defined PY (
    where py.exe >nul 2>nul
    if not errorlevel 1 set "PY=py.exe -3"
)

if not defined PY (
    echo [WARN] Python not found on PATH. Static repair succeeded.
    >>"%LOG%" echo [WARN] Python unavailable; import test skipped.
    goto :success
)

echo [STEP] Python import check...
set "PYTHONDONTWRITEBYTECODE=1"
set "PYTHONPATH=%ROOT%\tools\control;%PYTHONPATH%"
%PY% -c "from PCCSurfaceCommon import SurfaceCommand, command_catalog; import CortexPCCGui; print('PASS GUI/common import contract')" >>"%LOG%" 2>&1
if errorlevel 1 (
    echo [FAIL] Python import check failed.
    echo.
    echo ----- LOG -----
    type "%LOG%"
    echo ---------------
    goto :finish
)

echo [PASS] Python GUI/common import contract passed.
>>"%LOG%" echo [PASS] Python GUI/common import contract passed.

:success
echo.
echo ============================================================
echo  REPAIR PASSED
echo ============================================================
echo.
echo Now run:
echo   G:\Cortex\PROJECT_CONTROL_CENTER.cmd
echo.
echo If PCC still fails, send:
echo   G:\Cortex\artifacts\logs\pcc-gui-preflight.log
echo.
goto :finish

:finish
echo.
echo ============================================================
echo Log is always here:
echo   %LOG%
echo ============================================================
echo.
echo Press any key to close this window.
pause >nul
exit /b
