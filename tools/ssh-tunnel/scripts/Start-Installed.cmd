@echo off
setlocal
findstr /x /c:"controlled" "%~dp0role.txt" >nul 2>nul
if errorlevel 1 (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\windows\Start-Controller.ps1"
) else (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\windows\Start-Controlled.ps1"
)
if errorlevel 1 pause
