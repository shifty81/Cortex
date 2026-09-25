@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

set "LIVE=%ROOT%\PROJECT_CONTROL_CENTER.cmd"
set "PAYLOAD=%ROOT%\PROJECT_CONTROL_CENTER.cmd.fixpayload"
set "LOG=%ROOT%\artifacts\logs\pcc-buildgate-recovery-r1.log"
set "EXPECTED=9f62406a0253dc1574c037bb5daf78e78659bd34"
set "PATCHED=f828d6e170db9d4bf8df3787463a3b53769e7ef1"

if not exist "%ROOT%\artifacts\logs" mkdir "%ROOT%\artifacts\logs" >nul 2>nul

echo.
echo ============================================================
echo  CORTEX PCC BUILD-GATE RECOVERY R1
echo ============================================================
echo Root : %ROOT%
echo Log  : %LOG%
echo.

if not exist "%LIVE%" (
  echo [FAIL] Missing launcher: %LIVE%
  pause
  exit /b 2
)
if not exist "%PAYLOAD%" (
  echo [FAIL] Missing payload: %PAYLOAD%
  pause
  exit /b 2
)
if not exist "%ROOT%\scripts\Bootstrap-CortexEnvironment.ps1" (
  echo [FAIL] Startup bootstrap script is missing:
  echo        %ROOT%\scripts\Bootstrap-CortexEnvironment.ps1
  echo [INFO] Nothing was changed.
  pause
  exit /b 2
)

where git.exe >nul 2>nul
if errorlevel 1 (
  echo [FAIL] git.exe is required for exact preimage verification.
  pause
  exit /b 2
)

set "CUR="
for /f "delims=" %%H in ('git.exe hash-object -- "%LIVE%"') do set "CUR=%%H"
set "NEW="
for /f "delims=" %%H in ('git.exe hash-object -- "%PAYLOAD%"') do set "NEW=%%H"

> "%LOG%" echo CORTEX PCC BUILD-GATE RECOVERY R1
>>"%LOG%" echo Root=%ROOT%
>>"%LOG%" echo CurrentBlob=%CUR%
>>"%LOG%" echo RequiredPreimage=%EXPECTED%
>>"%LOG%" echo PayloadBlob=%NEW%
>>"%LOG%" echo RequiredPayload=%PATCHED%
>>"%LOG%" echo Started=%DATE% %TIME%

echo Current launcher : %CUR%
echo Required preimage: %EXPECTED%
echo Payload          : %NEW%
echo Required payload : %PATCHED%
echo.

if /I "%CUR%"=="%PATCHED%" (
  echo [PASS] Launcher recovery is already applied.
  >>"%LOG%" echo [PASS] Already applied.
  goto :diagnose
)

if /I not "%CUR%"=="%EXPECTED%" (
  echo [FAIL] Launcher no longer matches the captured FULL_FAIL preimage.
  echo [FAIL] Nothing was changed.
  >>"%LOG%" echo [FAIL] PREIMAGE_MISMATCH
  pause
  exit /b 3
)

if /I not "%NEW%"=="%PATCHED%" (
  echo [FAIL] Payload verification failed.
  echo [FAIL] Nothing was changed.
  >>"%LOG%" echo [FAIL] PAYLOAD_MISMATCH
  pause
  exit /b 4
)

set "STAMP=%DATE:~10,4%%DATE:~4,2%%DATE:~7,2%-%TIME:~0,2%%TIME:~3,2%%TIME:~6,2%"
set "STAMP=%STAMP: =0%"
set "BACKUPDIR=%ROOT%\artifacts\repair-backups\PCC-BUILDGATE-R1-%STAMP%"
mkdir "%BACKUPDIR%" >nul 2>nul

copy /y "%LIVE%" "%BACKUPDIR%\PROJECT_CONTROL_CENTER.cmd.before" >nul
if errorlevel 1 (
  echo [FAIL] Backup failed.
  pause
  exit /b 5
)

copy /y "%PAYLOAD%" "%LIVE%" >nul
if errorlevel 1 (
  copy /y "%BACKUPDIR%\PROJECT_CONTROL_CENTER.cmd.before" "%LIVE%" >nul
  echo [FAIL] Write failed; backup restored.
  pause
  exit /b 6
)

set "AFTER="
for /f "delims=" %%H in ('git.exe hash-object -- "%LIVE%"') do set "AFTER=%%H"
if /I not "%AFTER%"=="%PATCHED%" (
  copy /y "%BACKUPDIR%\PROJECT_CONTROL_CENTER.cmd.before" "%LIVE%" >nul
  echo [FAIL] Post-write verification failed; backup restored.
  pause
  exit /b 7
)

echo [PASS] Startup environment bootstrap restored without removing fail-closed GUI behavior.
>>"%LOG%" echo [PASS] Launcher updated.
>>"%LOG%" echo Backup=%BACKUPDIR%\PROJECT_CONTROL_CENTER.cmd.before

:diagnose
echo.
echo [STEP] Running bootstrap in CHECK mode for evidence...
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\Bootstrap-CortexEnvironment.ps1" -Mode Check >>"%LOG%" 2>&1
echo.
echo [INFO] Environment handoff:
if exist "%ROOT%\.cortex\bootstrap-env.cmd" (
  echo   %ROOT%\.cortex\bootstrap-env.cmd
  >>"%LOG%" echo [PASS] environment handoff exists
) else (
  echo   [MISSING] %ROOT%\.cortex\bootstrap-env.cmd
  >>"%LOG%" echo [WARN] environment handoff missing after check
)

echo.
echo ============================================================
echo  RECOVERY APPLIED - RESTART PCC
echo ============================================================
echo.
echo Close all Cortex PCC windows, then launch:
echo   %ROOT%\PROJECT_CONTROL_CENTER.cmd
echo.
echo Then run QUICK first, not FULL.
echo.
echo If QUICK still reports link.exe/cl.exe missing, run the existing:
echo   %ROOT%\HYDRATE_CORTEX_BUILD_TOOLS.cmd
echo once, restart PCC, and run QUICK again.
echo.
echo This patch does NOT change Git trust and does NOT claim GREEN.
echo.
pause
