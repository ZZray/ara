@echo off
setlocal
if exist "%~dp0scripts\windows\Start-Controlled.ps1" (
  powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\windows\Start-Controlled.ps1"
) else (
  powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0Start-Controlled.ps1"
)
echo.
pause
