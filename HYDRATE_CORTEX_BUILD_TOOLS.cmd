@echo off
setlocal EnableExtensions DisableDelayedExpansion
set "ROOT=%~dp0"
echo.
echo ============================================================
echo  CORTEX WINDOWS BUILD-TOOLS HYDRATION
echo ============================================================
echo This installs the Visual Studio 2022 C++ Build Tools workload into
echo the shared Cortex Vault toolchain area. It is a large one-time
echo download and may request Windows elevation.
echo.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%ROOT%scripts\Bootstrap-CortexEnvironment.ps1" -Mode Full -IncludeBuildTools
set "RC=%ERRORLEVEL%"
if exist "%ROOT%.cortex\bootstrap-env.cmd" call "%ROOT%.cortex\bootstrap-env.cmd"
echo.
if not "%RC%"=="0" (
  echo Build-tools hydration FAILED with exit code %RC%.
  echo See: %ROOT%artifacts\bootstrap\bootstrap.log
  pause
  exit /b %RC%
)
where link.exe >nul 2>nul
if errorlevel 1 (
  echo [FAIL] Build Tools finished but link.exe is still unavailable in the activated Cortex environment.
  echo See: %ROOT%artifacts\bootstrap\environment-health.json
  echo Visual Studio setup logs: %ROOT%artifacts\bootstrap\visual-studio-setup
  pause
  exit /b 20
)
where cl.exe >nul 2>nul
if errorlevel 1 (
  echo [FAIL] Build Tools finished but cl.exe is still unavailable in the activated Cortex environment.
  echo See: %ROOT%artifacts\bootstrap\environment-health.json
  echo Visual Studio setup logs: %ROOT%artifacts\bootstrap\visual-studio-setup
  pause
  exit /b 21
)
echo [PASS] MSVC amd64 developer environment active.
where link.exe
where cl.exe
echo Build environment hydration complete.
echo See: %ROOT%artifacts\bootstrap\environment-health.json
pause
exit /b 0
