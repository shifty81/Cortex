@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0APPLY_CORTEX_R4_CORRECTIONS.ps1"
set RC=%ERRORLEVEL%
echo.
if not "%RC%"=="0" (
  echo Cortex R4 cumulative normalization FAILED.
  pause
  exit /b %RC%
)
echo Cortex R4 cumulative normalization PASS.
pause
exit /b 0
