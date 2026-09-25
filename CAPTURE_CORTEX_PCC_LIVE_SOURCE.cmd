@echo off
setlocal EnableExtensions DisableDelayedExpansion

set "ROOT=G:\Cortex"
if not exist "%ROOT%\tools\control\PCCSurfaceCommon.py" (
    set "ROOT=%~dp0"
    if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
)

set "OUTDIR=%ROOT%\artifacts\handoff"
set "STAMP=%DATE:~10,4%%DATE:~4,2%%DATE:~7,2%-%TIME:~0,2%%TIME:~3,2%%TIME:~6,2%"
set "STAMP=%STAMP: =0%"
set "WORK=%OUTDIR%\Cortex_PCC_Live_Source_%STAMP%"
set "ZIP=%OUTDIR%\Cortex_PCC_Live_Source_%STAMP%.zip"
set "LOG=%TEMP%\Cortex_PCC_Live_Source_Capture.log"

if not exist "%OUTDIR%" mkdir "%OUTDIR%" >nul 2>nul
if exist "%WORK%" rmdir /s /q "%WORK%" >nul 2>nul
mkdir "%WORK%" >nul 2>nul
mkdir "%WORK%\tools\control" >nul 2>nul
mkdir "%WORK%\artifacts\logs" >nul 2>nul

> "%LOG%" echo CORTEX PCC LIVE SOURCE CAPTURE
>>"%LOG%" echo Root=%ROOT%
>>"%LOG%" echo Work=%WORK%
>>"%LOG%" echo Zip=%ZIP%
>>"%LOG%" echo Started=%DATE% %TIME%
>>"%LOG%" echo.

echo.
echo ============================================================
echo  CORTEX PCC LIVE SOURCE CAPTURE
echo ============================================================
echo Root : %ROOT%
echo.
echo This is READ-ONLY with respect to source.
echo It only copies evidence into artifacts\handoff.
echo.

for %%F in (
    "tools\control\PCCSurfaceCommon.py"
    "tools\control\CortexPCCGui.py"
    "tools\control\PCCAutoAdapter.py"
    "tools\control\PCCOperationHost.py"
    "tools\control\CortexPCC.py"
    "tools\control\CortexGitAuthority.py"
    "PROJECT_CONTROL_CENTER.cmd"
    "project.control.json"
) do (
    if exist "%ROOT%\%%~F" (
        echo [COPY] %%~F
        copy /y "%ROOT%\%%~F" "%WORK%\%%~F" >nul
    )
)

if exist "%ROOT%\artifacts\logs\pcc-gui-preflight.log" (
    copy /y "%ROOT%\artifacts\logs\pcc-gui-preflight.log" "%WORK%\artifacts\logs\pcc-gui-preflight.log" >nul
)
if exist "%ROOT%\artifacts\logs\pcc-surface-repair-r3.log" (
    copy /y "%ROOT%\artifacts\logs\pcc-surface-repair-r3.log" "%WORK%\artifacts\logs\pcc-surface-repair-r3.log" >nul
)

where git.exe >nul 2>nul
if not errorlevel 1 (
    git.exe -C "%ROOT%" status --short --branch > "%WORK%\git-status.txt" 2>&1
    git.exe -C "%ROOT%" rev-parse HEAD > "%WORK%\git-head.txt" 2>&1
    git.exe -C "%ROOT%" diff -- tools/control/PCCSurfaceCommon.py tools/control/CortexPCCGui.py tools/control/PCCAutoAdapter.py tools/control/PCCOperationHost.py tools/control/CortexPCC.py tools/control/CortexGitAuthority.py PROJECT_CONTROL_CENTER.cmd project.control.json > "%WORK%\live-diff.patch" 2>&1

    > "%WORK%\blob-ids.txt" (
        for %%F in (
            "tools\control\PCCSurfaceCommon.py"
            "tools\control\CortexPCCGui.py"
            "tools\control\PCCAutoAdapter.py"
            "tools\control\PCCOperationHost.py"
            "tools\control\CortexPCC.py"
            "tools\control\CortexGitAuthority.py"
            "PROJECT_CONTROL_CENTER.cmd"
            "project.control.json"
        ) do (
            if exist "%ROOT%\%%~F" (
                for /f "delims=" %%H in ('git.exe hash-object -- "%ROOT%\%%~F"') do echo %%H  %%~F
            )
        )
    )
)

> "%WORK%\README_CAPTURE.txt" echo This is a read-only live PCC source/evidence capture.
>>"%WORK%\README_CAPTURE.txt" echo No source files were modified by the capture process.
>>"%WORK%\README_CAPTURE.txt" echo Created=%DATE% %TIME%
>>"%WORK%\README_CAPTURE.txt" echo Root=%ROOT%

if exist "%ZIP%" del /q "%ZIP%" >nul 2>nul

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command ^
  "Compress-Archive -LiteralPath '%WORK%\*' -DestinationPath '%ZIP%' -CompressionLevel Optimal -Force" >>"%LOG%" 2>&1

if errorlevel 1 (
    echo [FAIL] Could not create ZIP.
    echo [INFO] Evidence folder still exists:
    echo        %WORK%
    echo [INFO] Log:
    echo        %LOG%
    goto :done
)

echo.
echo [PASS] Live PCC source/evidence captured.
echo.
echo Upload this ZIP to me:
echo   %ZIP%
echo.
echo Do NOT apply another source repair before I inspect it.

:done
echo.
pause
