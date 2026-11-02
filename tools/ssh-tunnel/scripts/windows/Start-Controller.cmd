@echo off
setlocal
if exist "%~dp0scripts\windows\Start-Controller.ps1" (
  powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\windows\Start-Controller.ps1"
) else (
  powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0Start-Controller.ps1"
)
echo.
pause
