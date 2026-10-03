@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0APPLY_CORTEX_R2_TEST_CONTRACT_FIX.ps1"
set RC=%ERRORLEVEL%
echo.
if not "%RC%"=="0" (
  echo Cortex R2 test-contract normalization FAILED.
  pause
  exit /b %RC%
)
echo Cortex R2 test-contract normalization PASS.
pause
exit /b 0
