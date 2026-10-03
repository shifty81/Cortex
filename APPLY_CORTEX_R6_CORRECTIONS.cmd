@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0APPLY_CORTEX_R6_CORRECTIONS.ps1"
set RC=%ERRORLEVEL%
echo.
if not "%RC%"=="0" (
  echo Cortex R6 cumulative normalization FAILED.
  pause
  exit /b %RC%
)
echo Cortex R6 cumulative normalization PASS.
pause
exit /b 0
