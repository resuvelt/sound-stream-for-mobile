#requires -Version 5
# sound-stream — трей-обёртка.
# Держит сервер (node server.js) запущенным в фоне без окна и показывает
# значок у часов: адрес для телефона, «открыть на этом ПК», перезапуск, выход.
# Запускается скрыто через launch-hidden.vbs (см. install.ps1).

$ErrorActionPreference = 'Stop'

# --- один экземпляр на систему ---
$created = $false
$script:mutex = New-Object System.Threading.Mutex($true, 'Global\sound-stream-tray', [ref]$created)
if (-not $created) { return }   # уже запущено — тихо выходим

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

# --- пути и настройки ---
$script:root   = Split-Path -Parent $PSScriptRoot          # tray/ -> корень репозитория
$script:server = Join-Path $script:root 'server.js'
$script:port   = if ($env:PORT) { [int]$env:PORT } else { 47823 }

$script:node = (Get-Command node -ErrorAction SilentlyContinue).Source
if (-not $script:node) { $script:node = Join-Path $env:ProgramFiles 'nodejs\node.exe' }

$logDir = Join-Path $env:LOCALAPPDATA 'sound-stream'
[void][System.IO.Directory]::CreateDirectory($logDir)
$script:logFile = Join-Path $logDir 'server.log'

$script:proc = $null

# --- локальные (LAN) IPv4-адреса, без виртуальных адаптеров ---
function Get-LanIPv4 {
  $skip = 'vEthernet|WSL|Hyper-V|Default Switch|outline|VirtualBox|VMware|Loopback|docker|veth|VPN|TAP'
  $ips = @()
  try {
    $ips = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction Stop |
      Where-Object { $_.IPAddress -ne '127.0.0.1' -and $_.InterfaceAlias -notmatch $skip } |
      Select-Object -ExpandProperty IPAddress
  } catch {
    $ips = [System.Net.Dns]::GetHostAddresses([System.Net.Dns]::GetHostName()) |
      Where-Object { $_.AddressFamily -eq 'InterNetwork' -and $_.ToString() -ne '127.0.0.1' } |
      ForEach-Object { $_.ToString() }
  }
  $rank = { param($ip) if ($ip -like '192.168.*') { 0 } elseif ($ip -like '10.*') { 1 } else { 2 } }
  @($ips | Sort-Object { & $rank $_ })
}

# --- запуск / остановка сервера ---
# Node запускаем через cmd с перенаправлением вывода в лог: так вывод пишется
# на уровне ОС и не зависит от цикла сообщений трея. Окна нет (CreateNoWindow).
function Start-Server {
  if ($script:proc -and -not $script:proc.HasExited) { return }
  "=== sound-stream $(Get-Date -Format s) — порт $script:port ===" |
    Out-File -FilePath $script:logFile -Encoding utf8
  $inner = '"' + $script:node + '" "' + $script:server + '" 1>>"' + $script:logFile + '" 2>&1'
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName         = $env:ComSpec                 # cmd.exe
  $psi.Arguments        = '/d /s /c "' + $inner + '"'
  $psi.WorkingDirectory = $script:root
  $psi.UseShellExecute  = $false
  $psi.CreateNoWindow   = $true
  $psi.EnvironmentVariables['PORT'] = "$script:port"
  $script:proc = [System.Diagnostics.Process]::Start($psi)
}

function Stop-Server {
  if ($script:proc -and -not $script:proc.HasExited) {
    # Гасим всё дерево (cmd + дочерний node).
    try {
      Start-Process -FilePath (Join-Path $env:WINDIR 'System32\taskkill.exe') `
        -ArgumentList "/PID $($script:proc.Id) /T /F" -WindowStyle Hidden -Wait
    } catch {}
  }
  $script:proc = $null
}

# --- значок в трее (рисуем динамик через GDI) ---
function New-TrayIcon {
  $bmp = New-Object System.Drawing.Bitmap 32, 32
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.Clear([System.Drawing.Color]::Transparent)
  $blue = [System.Drawing.Color]::FromArgb(46, 125, 255)
  $brush = New-Object System.Drawing.SolidBrush $blue
  $pts = @(
    (New-Object System.Drawing.Point 5, 13),
    (New-Object System.Drawing.Point 11, 13),
    (New-Object System.Drawing.Point 17, 7),
    (New-Object System.Drawing.Point 17, 25),
    (New-Object System.Drawing.Point 11, 19),
    (New-Object System.Drawing.Point 5, 19)
  )
  $g.FillPolygon($brush, $pts)
  $pen = New-Object System.Drawing.Pen $blue, 2.2
  $g.DrawArc($pen, 18, 11, 6, 10, -55, 110)
  $g.DrawArc($pen, 20, 8,  9, 16, -50, 100)
  $g.Dispose()
  [System.Drawing.Icon]::FromHandle($bmp.GetHicon())
}

$script:notify = New-Object System.Windows.Forms.NotifyIcon
$script:notify.Icon    = New-TrayIcon
$script:notify.Text    = 'sound-stream'
$script:notify.Visible = $true

$script:menu = New-Object System.Windows.Forms.ContextMenuStrip
$script:notify.ContextMenuStrip = $script:menu

# Меню пересобирается при каждом открытии — адреса всегда актуальны.
$script:menu.add_Opening({
  $script:menu.Items.Clear()

  $title = $script:menu.Items.Add('sound-stream')
  $title.Enabled = $false
  $title.Font = New-Object System.Drawing.Font($title.Font, [System.Drawing.FontStyle]::Bold)

  $running = ($script:proc -and -not $script:proc.HasExited)
  $status = $script:menu.Items.Add($(if ($running) { '● сервер работает' } else { '○ сервер остановлен' }))
  $status.Enabled = $false

  [void]$script:menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator))

  $hint = $script:menu.Items.Add('Открой на телефоне (та же Wi-Fi):')
  $hint.Enabled = $false
  $ips = Get-LanIPv4
  if ($ips.Count -eq 0) {
    $none = $script:menu.Items.Add('   LAN-адрес не найден')
    $none.Enabled = $false
  } else {
    foreach ($ip in $ips) {
      $url = "http://$ip`:$script:port/"
      $it = $script:menu.Items.Add("   📱  $url")
      $it.Tag = $url
      $it.add_Click({
        $u = $this.Tag
        try { [System.Windows.Forms.Clipboard]::SetText($u) } catch {}
        $script:notify.ShowBalloonTip(2500, 'Адрес скопирован', $u, [System.Windows.Forms.ToolTipIcon]::Info)
      })
    }
  }

  [void]$script:menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator))

  $open = $script:menu.Items.Add('Открыть на этом ПК')
  $open.add_Click({ Start-Process "http://localhost:$script:port/" })

  $restart = $script:menu.Items.Add('Перезапустить сервер')
  $restart.add_Click({
    Stop-Server
    Start-Server
    $script:notify.ShowBalloonTip(2000, 'sound-stream', 'Сервер перезапущен', [System.Windows.Forms.ToolTipIcon]::Info)
  })

  [void]$script:menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator))

  $exit = $script:menu.Items.Add('Выход')
  $exit.add_Click({ $script:appCtx.ExitThread() })
})

# Двойной клик по значку — открыть плеер на этом ПК.
$script:notify.add_MouseDoubleClick({ Start-Process "http://localhost:$script:port/" })

# --- следим за сервером: если упал сам, сообщаем и обновляем подсказку ---
$script:crashNotified = $false
$script:watch = New-Object System.Windows.Forms.Timer
$script:watch.Interval = 3000
$script:watch.add_Tick({
  if ($script:proc -and $script:proc.HasExited) {
    if (-not $script:crashNotified) {
      $script:crashNotified = $true
      $script:notify.Text = 'sound-stream — сервер остановлен'
      $script:notify.ShowBalloonTip(4000, 'sound-stream',
        'Сервер неожиданно завершился. Открой меню → «Перезапустить сервер». Лог: ' + $script:logFile,
        [System.Windows.Forms.ToolTipIcon]::Warning)
    }
  } else {
    $script:crashNotified = $false
    $script:notify.Text = 'sound-stream — работает'
  }
})
$script:watch.Start()

# --- старт ---
try {
  Start-Server
  $ips = Get-LanIPv4
  $addr = if ($ips.Count) { "http://$($ips[0])`:$script:port/" } else { "http://localhost:$script:port/" }
  $script:notify.ShowBalloonTip(3500, 'sound-stream запущен', "Адрес для телефона: $addr", [System.Windows.Forms.ToolTipIcon]::Info)
} catch {
  $script:notify.ShowBalloonTip(5000, 'sound-stream — ошибка запуска', "$_", [System.Windows.Forms.ToolTipIcon]::Error)
}

# --- цикл сообщений; держит трей живым до «Выхода» ---
$script:appCtx = New-Object System.Windows.Forms.ApplicationContext
try {
  [System.Windows.Forms.Application]::Run($script:appCtx)
} finally {
  $script:watch.Stop()
  Stop-Server
  $script:notify.Visible = $false
  $script:notify.Dispose()
  try { $script:mutex.ReleaseMutex() } catch {}
}
