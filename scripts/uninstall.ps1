# Uninstalls LiteChat. By default the session (WhatsApp login) is kept;
# pass -RemoveData to delete it as well.
param([switch]$RemoveData)

$AppName    = 'LiteChat'
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$DataDir    = Join-Path $env:LOCALAPPDATA $AppName

Set-Location $env:TEMP
Get-Process litechat -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

Remove-Item (Join-Path ([Environment]::GetFolderPath('Programs')) "$AppName.lnk") -ErrorAction SilentlyContinue
Remove-Item (Join-Path ([Environment]::GetFolderPath('Desktop')) "$AppName.lnk") -ErrorAction SilentlyContinue
Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name $AppName -ErrorAction SilentlyContinue
Remove-Item -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppName" -Recurse -ErrorAction SilentlyContinue
# Notification sender registration (name and icon shown on toasts)
Remove-Item -Path 'HKCU:\Software\Classes\AppUserModelId\LiteChat.Desktop' -Recurse -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue

if ($RemoveData) {
    Remove-Item -LiteralPath $DataDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'Data and session removed.'
}
Write-Host "$AppName uninstalled." -ForegroundColor Green
