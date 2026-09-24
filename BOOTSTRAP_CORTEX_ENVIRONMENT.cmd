@echo off
setlocal EnableExtensions DisableDelayedExpansion
set "ROOT=%~dp0"
echo.
echo ============================================================
echo  CORTEX ENVIRONMENT BOOTSTRAP / HYDRATION
echo ============================================================
echo  Source : %ROOT%
echo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\Bootstrap-CortexEnvironment.ps1" -Mode Hydrate
set "RC=%ERRORLEVEL%"
if exist "%ROOT%.cortex\bootstrap-env.cmd" call "%ROOT%.cortex\bootstrap-env.cmd"
echo.
if not "%RC%"=="0" (
  echo Cortex environment hydration FAILED with exit code %RC%.
  echo See: %ROOT%artifacts\bootstrap\bootstrap.log
  pause
  exit /b %RC%
)
echo Core Cortex environment hydration complete.
echo See: %ROOT%artifacts\bootstrap\environment-health.json
echo.
echo For the large Windows C++/MSVC linker toolchain, run:
echo   HYDRATE_CORTEX_BUILD_TOOLS.cmd
pause
exit /b 0
