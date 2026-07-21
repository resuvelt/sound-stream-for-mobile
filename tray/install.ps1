#requires -Version 5
# Включает автозапуск sound-stream при входе в систему и запускает его сейчас.
# Запуск (двойной клик по install.cmd или из PowerShell):
#     powershell -ExecutionPolicy Bypass -File tray\install.ps1
# Права администратора не нужны — ярлык кладётся в личную папку «Автозагрузка».

$ErrorActionPreference = 'Stop'

$here = $PSScriptRoot
$root = Split-Path -Parent $here
$vbs  = Join-Path $here 'launch-hidden.vbs'

if (-not (Test-Path (Join-Path $root 'server.js'))) {
  Write-Host "Не нашёл server.js рядом со скриптом — папка tray/ должна лежать в корне проекта." -ForegroundColor Red
  return
}

# Зависимости.
if (-not (Test-Path (Join-Path $root 'node_modules'))) {
  Write-Host "Ставлю зависимости (npm install)…" -ForegroundColor Cyan
  Push-Location $root
  try { & npm install } finally { Pop-Location }
}

# Ярлык автозапуска в личной папке «Автозагрузка».
$startup = [Environment]::GetFolderPath('Startup')
$lnkPath = Join-Path $startup 'sound-stream.lnk'
$ws  = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut($lnkPath)
$lnk.TargetPath       = Join-Path $env:WINDIR 'System32\wscript.exe'
$lnk.Arguments        = '"' + $vbs + '"'
$lnk.WorkingDirectory = $root
$lnk.Description       = 'sound-stream — стрим системного звука ПК на телефон'
$lnk.WindowStyle      = 7
$lnk.Save()
Write-Host "✔ Автозапуск включён: $lnkPath" -ForegroundColor Green

# Запустить прямо сейчас, если ещё не запущено.
$already = Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" -ErrorAction SilentlyContinue |
  Where-Object { $_.CommandLine -like '*tray.ps1*' }
if ($already) {
  Write-Host "✔ Трей уже запущен." -ForegroundColor Green
} else {
  Start-Process -FilePath (Join-Path $env:WINDIR 'System32\wscript.exe') -ArgumentList ('"' + $vbs + '"')
  Write-Host "✔ Запущено — ищи синий значок динамика у часов (в трее)." -ForegroundColor Green
}

Write-Host ""
Write-Host "Готово. Теперь sound-stream стартует сам при каждом входе в систему." -ForegroundColor Green
Write-Host "Отключить автозапуск: powershell -ExecutionPolicy Bypass -File tray\uninstall.ps1"
