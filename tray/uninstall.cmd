@echo off
rem Двойной клик — отключить автозапуск и остановить sound-stream.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1"
echo.
pause
