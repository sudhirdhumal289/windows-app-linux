# === Run once as admin in Cloud PC ===

$watchdog = @'
@echo off
:: === Clipboard ===
reg add "HKLM\SOFTWARE\Microsoft\PolicyManager\Current\Device\ADMX_TerminalServer" /v TS_CLIENT_CLIPBOARD_ProviderSet /t REG_DWORD /d 0 /f
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v fDisableClip /t REG_DWORD /d 0 /f
reg add "HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp" /v fDisableClip /t REG_DWORD /d 0 /f

:: === Drive Redirection ===
reg add "HKLM\SOFTWARE\Microsoft\PolicyManager\Current\Device\ADMX_TerminalServer" /v TS_CLIENT_DRIVES_ProviderSet /t REG_DWORD /d 0 /f
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v fDisableCdm /t REG_DWORD /d 0 /f
reg add "HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp" /v fDisableCdm /t REG_DWORD /d 0 /f
'@

$watchdogPath = "C:\Scripts\ClipboardWatchdog.bat"
New-Item -Path "C:\Scripts" -ItemType Directory -Force | Out-Null
Set-Content -Path $watchdogPath -Value $watchdog

# Apply immediately
& $watchdogPath

# Register task
$Action    = New-ScheduledTaskAction -Execute $watchdogPath
$Trigger   = New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 1) -RepetitionDuration (New-TimeSpan -Days 9999)
$Settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden
$Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -RunLevel Highest

Register-ScheduledTask -TaskName "ClipboardWatchdog" `
  -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal `
  -Description "Unconditionally re-applies clipboard + drive redirection every 60s" -Force

Write-Host "`nDone. Log off and reconnect to test." -ForegroundColor Green
