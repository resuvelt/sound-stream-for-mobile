' Запускает трей-обёртку sound-stream полностью скрыто — без вспышки консоли.
' Используется ярлыком автозапуска (см. install.ps1) и для ручного старта.
Set sh = CreateObject("WScript.Shell")
dir = Left(WScript.ScriptFullName, InStrRev(WScript.ScriptFullName, "\"))
cmd = "powershell.exe -NoProfile -ExecutionPolicy Bypass -STA -WindowStyle Hidden -File """ & dir & "tray.ps1"""
sh.Run cmd, 0, False   ' 0 = скрытое окно, False = не ждать
