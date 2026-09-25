@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

set "LIVE=%ROOT%\tools\control\PCCSurfaceCommon.py"
set "PAYLOAD=%ROOT%\tools\control\PCCSurfaceCommon.py.fixpayload"
set "LOG=%ROOT%\artifacts\logs\pcc-live-compatibility-fix.log"
set "EXPECTED=26e187170781464ac053495f3c03546f49a1a0e5"
set "PATCHED=e69bcbe4993de64875be573fa55cb882d989861c"

if not exist "%ROOT%\artifacts\logs" mkdir "%ROOT%\artifacts\logs" >nul 2>nul

echo.
echo ============================================================
echo  CORTEX PCC LIVE COMPATIBILITY FIX
echo ============================================================
echo Root : %ROOT%
echo Log  : %LOG%
echo.

if not exist "%LIVE%" (
  echo [FAIL] Missing live file: %LIVE%
  pause
  exit /b 2
)
if not exist "%PAYLOAD%" (
  echo [FAIL] Missing patch payload: %PAYLOAD%
  pause
  exit /b 2
)

where git.exe >nul 2>nul
if errorlevel 1 (
  echo [FAIL] git.exe is required for exact blob verification.
  pause
  exit /b 2
)

set "CUR="
for /f "delims=" %%H in ('git.exe hash-object -- "%LIVE%"') do set "CUR=%%H"

set "NEW="
for /f "delims=" %%H in ('git.exe hash-object -- "%PAYLOAD%"') do set "NEW=%%H"

> "%LOG%" echo CORTEX PCC LIVE COMPATIBILITY FIX
>>"%LOG%" echo Root=%ROOT%
>>"%LOG%" echo CurrentBlob=%CUR%
>>"%LOG%" echo ExpectedBlob=%EXPECTED%
>>"%LOG%" echo PayloadBlob=%NEW%
>>"%LOG%" echo ExpectedPayload=%PATCHED%
>>"%LOG%" echo Started=%DATE% %TIME%

echo Current live blob : %CUR%
echo Required preimage : %EXPECTED%
echo Payload blob      : %NEW%
echo Expected payload  : %PATCHED%
echo.

if /I "%CUR%"=="%PATCHED%" (
  echo [PASS] Live PCCSurfaceCommon.py already contains this compatibility repair.
  >>"%LOG%" echo [PASS] Already applied.
  goto :smoke
)

if /I not "%CUR%"=="%EXPECTED%" (
  echo [FAIL] Refusing overwrite. Live source no longer matches the captured preimage.
  echo [FAIL] Nothing was changed.
  >>"%LOG%" echo [FAIL] PREIMAGE_MISMATCH. Nothing changed.
  pause
  exit /b 3
)

if /I not "%NEW%"=="%PATCHED%" (
  echo [FAIL] Patch payload failed its own exact-object verification.
  echo [FAIL] Nothing was changed.
  >>"%LOG%" echo [FAIL] PAYLOAD_MISMATCH. Nothing changed.
  pause
  exit /b 4
)

set "STAMP=%DATE:~10,4%%DATE:~4,2%%DATE:~7,2%-%TIME:~0,2%%TIME:~3,2%%TIME:~6,2%"
set "STAMP=%STAMP: =0%"
set "BACKUPDIR=%ROOT%\artifacts\repair-backups\PCC-LIVE-COMPAT-%STAMP%"
mkdir "%BACKUPDIR%" >nul 2>nul

copy /y "%LIVE%" "%BACKUPDIR%\PCCSurfaceCommon.py.before" >nul
if errorlevel 1 (
  echo [FAIL] Could not create backup.
  >>"%LOG%" echo [FAIL] Backup failed.
  pause
  exit /b 5
)

copy /y "%PAYLOAD%" "%LIVE%" >nul
if errorlevel 1 (
  echo [FAIL] Could not write repaired file. Restoring backup.
  copy /y "%BACKUPDIR%\PCCSurfaceCommon.py.before" "%LIVE%" >nul
  >>"%LOG%" echo [FAIL] Copy failed; backup restored.
  pause
  exit /b 6
)

set "AFTER="
for /f "delims=" %%H in ('git.exe hash-object -- "%LIVE%"') do set "AFTER=%%H"
if /I not "%AFTER%"=="%PATCHED%" (
  echo [FAIL] Post-write verification failed. Restoring backup.
  copy /y "%BACKUPDIR%\PCCSurfaceCommon.py.before" "%LIVE%" >nul
  >>"%LOG%" echo [FAIL] Post-write verification failed; backup restored.
  pause
  exit /b 7
)

>>"%LOG%" echo [PASS] Exact compatibility payload applied.
>>"%LOG%" echo Backup=%BACKUPDIR%\PCCSurfaceCommon.py.before
echo [PASS] Exact live compatibility repair applied.
echo Backup: %BACKUPDIR%\PCCSurfaceCommon.py.before

:smoke
echo.
echo [STEP] Static symbol verification...
findstr /c:"class SurfaceCommand" "%LIVE%" >nul
if errorlevel 1 goto :smokefail
findstr /c:"def command_catalog" "%LIVE%" >nul
if errorlevel 1 goto :smokefail
findstr /c:"def popen_contract" "%LIVE%" >nul
if errorlevel 1 goto :smokefail
findstr /c:"def popen_argv" "%LIVE%" >nul
if errorlevel 1 goto :smokefail
findstr /c:"def popen_shell" "%LIVE%" >nul
if errorlevel 1 goto :smokefail

echo [PASS] Required GUI/common API is present.
>>"%LOG%" echo [PASS] Required symbols present.

echo.
echo ============================================================
echo  PATCH APPLIED - SOURCE IS STILL UNVERIFIED
echo ============================================================
echo.
echo Next run:
echo   PROJECT_CONTROL_CENTER.cmd
echo.
echo If preflight passes and the GUI opens, send me the result.
echo Do not call the source GREEN until the normal FULL gate passes.
echo.
echo IMPORTANT GIT NOTE:
echo Git also reported G:\Cortex as a dubious-ownership repository.
echo We will correct that separately after GUI startup is restored.
echo.
pause
exit /b 0

:smokefail
echo [FAIL] Static compatibility smoke failed after apply.
echo [INFO] Log: %LOG%
>>"%LOG%" echo [FAIL] Static symbol smoke failed.
pause
exit /b 8
