@echo off
cd /d "%~dp0"
"%~dp0handwave-node.exe" %*
if errorlevel 1 pause

