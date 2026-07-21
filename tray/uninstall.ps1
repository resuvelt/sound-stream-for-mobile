#requires -Version 5
# Отключает автозапуск sound-stream и останавливает работающий трей и сервер.
#     powershell -ExecutionPolicy Bypass -File tray\uninstall.ps1

$ErrorActionPreference = 'Continue'

# Убрать ярлык автозапуска.
$startup = [Environment]::GetFolderPath('Startup')
$lnk = Join-Path $startup 'sound-stream.lnk'
if (Test-Path $lnk) {
  Remove-Item $lnk -Force
  Write-Host "✔ Автозапуск отключён." -ForegroundColor Green
} else {
  Write-Host "Автозапуск не был включён." -ForegroundColor Yellow
}

# Остановить трей (powershell с tray.ps1).
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" -ErrorAction SilentlyContinue |
  Where-Object { $_.CommandLine -like '*tray.ps1*' } |
  ForEach-Object {
    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    Write-Host "✔ Трей остановлен (PID $($_.ProcessId))." -ForegroundColor Green
  }

# Остановить сервер (node с server.js).
Get-CimInstance Win32_Process -Filter "Name='node.exe'" -ErrorAction SilentlyContinue |
  Where-Object { $_.CommandLine -like '*server.js*' } |
  ForEach-Object {
    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    Write-Host "✔ Сервер остановлен (PID $($_.ProcessId))." -ForegroundColor Green
  }

Write-Host "Готово." -ForegroundColor Green
