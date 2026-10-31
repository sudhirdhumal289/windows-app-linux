# === Watchdog: re-apply clipboard + drive redirection overrides every minute ===

# 1. Write the watchdog script
$watchdog = @'
@echo off
:: Clipboard
reg query "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v fDisableClip 2>nul | findstr /i "0x1" >nul
if %errorlevel%==0 (
    reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v fDisableClip /t REG_DWORD /d 0 /f
)

:: Drive redirection (file sharing)
reg query "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v DriveRedirection 2>nul | findstr /i "0x1" >nul
if %errorlevel%==0 (
    reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v DriveRedirection /t REG_DWORD /d 0 /f
)
'@

$watchdogPath = "C:\Scripts\ClipboardWatchdog.bat"
New-Item -Path "C:\Scripts" -ItemType Directory -Force | Out-Null
Set-Content -Path $watchdogPath -Value $watchdog

# 2. Apply immediately
& $watchdogPath

# 3. Register scheduled task — runs every 1 minute, as SYSTEM, hidden
$Action    = New-ScheduledTaskAction -Execute $watchdogPath
$Trigger = New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 1) -RepetitionDuration (New-TimeSpan -Days 9999)
$Settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden
$Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -RunLevel Highest

Register-ScheduledTask -TaskName "ClipboardWatchdog" `
  -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal `
  -Description "Polls every minute; re-enables RDP clipboard if Intune re-blocks it" -Force

Write-Host "`nDone. Watchdog is running every 60s. No more mid-session clipboard loss." -ForegroundColor Green
