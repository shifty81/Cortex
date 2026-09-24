@echo off
setlocal EnableExtensions DisableDelayedExpansion
chcp 65001 >nul 2>nul

set "CORTEX_ROOT=%~dp0"
if "%CORTEX_ROOT:~-1%"=="\" set "CORTEX_ROOT=%CORTEX_ROOT:~0,-1%"

set "PCC_CORE=%CORTEX_ROOT%\tools\control\CortexPCC.py"
set "PCC_GUI=%CORTEX_ROOT%\tools\control\CortexPCCGui.py"
set "PCC_CONSOLE=%CORTEX_ROOT%\tools\control\CortexPCCConsole.py"
set "PCC_EXIT=1"
set "PY_CMD="
set "PYW_CMD="

rem Pre-Python bootstrap: check/hydrate the shared Cortex environment first.
call :bootstrap_environment
if exist "%CORTEX_ROOT%\.cortex\bootstrap-env.cmd" call "%CORTEX_ROOT%\.cortex\bootstrap-env.cmd"

call :resolve_python
if errorlevel 1 goto :python_fail

if /I "%~1"=="--cli" goto :interactive_cli
if /I "%~1"=="cli" goto :interactive_cli
if /I "%CORTEX_PCC_FORCE_CLI%"=="1" goto :interactive_cli

rem Any explicit command is a headless/machine-facing PCC request.
if not "%~1"=="" goto :headless

rem Normal double-click/operator launch: GUI is primary. The console shell remains a fallback.
if exist "%PCC_GUI%" (
  %PY_CMD% "%PCC_GUI%" --self-test --root "%CORTEX_ROOT%" >nul 2>nul
  if errorlevel 1 (
    echo [WARN] PCC GUI preflight failed. Falling back to the formatted console shell.
    goto :interactive_cli
  )
  if defined PYW_CMD (
    start "Cortex Project Control Center" %PYW_CMD% "%PCC_GUI%" --root "%CORTEX_ROOT%"
    exit /b 0
  )
  start "Cortex Project Control Center" %PY_CMD% "%PCC_GUI%" --root "%CORTEX_ROOT%"
  exit /b 0
)

echo [WARN] PCC GUI surface is missing. Falling back to the formatted console shell.
goto :interactive_cli

:interactive_cli
if exist "%PCC_CONSOLE%" (
  %PY_CMD% "%PCC_CONSOLE%" --root "%CORTEX_ROOT%"
) else (
  echo [WARN] Formatted PCC console surface is missing. Falling back to CortexPCC.py.
  %PY_CMD% "%PCC_CORE%" --root "%CORTEX_ROOT%"
)
set "PCC_EXIT=%ERRORLEVEL%"
goto :done

:headless
%PY_CMD% "%PCC_CORE%" %* --root "%CORTEX_ROOT%"
set "PCC_EXIT=%ERRORLEVEL%"
goto :done

:bootstrap_environment
if not exist "%CORTEX_ROOT%\scripts\Bootstrap-CortexEnvironment.ps1" exit /b 0
where powershell.exe >nul 2>nul
if errorlevel 1 (
  echo [WARN] Windows PowerShell is unavailable; automatic Cortex dependency hydration cannot run.
  exit /b 0
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%CORTEX_ROOT%\scripts\Bootstrap-CortexEnvironment.ps1" -Mode Startup
if errorlevel 1 echo [WARN] Cortex environment bootstrap reported a launch blocker.
exit /b 0

:resolve_python
if defined CORTEX_PYTHON_EXE if exist "%CORTEX_PYTHON_EXE%" (
  "%CORTEX_PYTHON_EXE%" -c "import sys; raise SystemExit(0 if sys.version_info >= (3,11) else 1)" >nul 2>nul
  if not errorlevel 1 (
    set PY_CMD="%CORTEX_PYTHON_EXE%"
    if defined CORTEX_PYTHONW_EXE if exist "%CORTEX_PYTHONW_EXE%" set PYW_CMD="%CORTEX_PYTHONW_EXE%"
    exit /b 0
  )
)

where py.exe >nul 2>nul
if not errorlevel 1 (
  py.exe -3 -c "import sys; raise SystemExit(0 if sys.version_info >= (3,11) else 1)" >nul 2>nul
  if not errorlevel 1 (
    set "PY_CMD=py.exe -3"
    where pyw.exe >nul 2>nul
    if not errorlevel 1 set "PYW_CMD=pyw.exe -3"
    exit /b 0
  )
)

where python.exe >nul 2>nul
if not errorlevel 1 (
  python.exe -c "import sys; raise SystemExit(0 if sys.version_info >= (3,11) else 1)" >nul 2>nul
  if not errorlevel 1 (
    set "PY_CMD=python.exe"
    where pythonw.exe >nul 2>nul
    if not errorlevel 1 set "PYW_CMD=pythonw.exe"
    exit /b 0
  )
)
exit /b 1

:python_fail
echo [FAIL] Cortex Project Control Center could not hydrate or locate Python 3.11 or newer.
echo Run BOOTSTRAP_CORTEX_ENVIRONMENT.cmd and inspect artifacts\bootstrap\environment-health.json.
set "PCC_EXIT=1"
goto :done

:done
if not "%PCC_EXIT%"=="0" (
  echo.
  echo Cortex Project Control Center exited with code %PCC_EXIT%.
  echo Environment evidence: %CORTEX_ROOT%\artifacts\bootstrap\environment-health.json
  echo Latest debug evidence: %CORTEX_ROOT%\artifacts\debug\LATEST_DEBUG_BUNDLE.txt
  if exist "%CORTEX_ROOT%\artifacts\debug" start "" explorer.exe "%CORTEX_ROOT%\artifacts\debug"
  echo.
  pause
)
exit /b %PCC_EXIT%
