@echo off
rem Двойной клик — включить автозапуск sound-stream в трее.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
echo.
pause
