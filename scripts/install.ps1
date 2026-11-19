# Installs LiteChat for the current user (no administrator rights needed).
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1 [-Autostart] [-NoDesktopShortcut]
param(
    [string]$Source,
    [switch]$Autostart,
    [switch]$NoDesktopShortcut
)
$ErrorActionPreference = 'Stop'

$AppName   = 'LiteChat'
$ExeName   = 'litechat.exe'
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
$UninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$AppName"
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

# Locate the executable: parameter, same folder as this script (release zip) or local build.
if (-not $Source) {
    $candidates = @(
        (Join-Path $PSScriptRoot $ExeName),
        (Join-Path $PSScriptRoot "..\$ExeName"),
        (Join-Path $PSScriptRoot "..\target\release\$ExeName")
    )
    $Source = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Source -or -not (Test-Path $Source)) {
    throw "Cannot find $ExeName. Build it first with 'cargo build --release' or pass -Source <path>."
}

Write-Host "Installing $AppName to $InstallDir"
Get-Process litechat -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$exe = Join-Path $InstallDir $ExeName
Copy-Item -LiteralPath $Source -Destination $exe -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'uninstall.ps1') -Destination $InstallDir -Force

# Shortcuts
$shell = New-Object -ComObject WScript.Shell
function New-AppShortcut([string]$Path) {
    $lnk = $shell.CreateShortcut($Path)
    $lnk.TargetPath = $exe
    $lnk.WorkingDirectory = $InstallDir
    $lnk.IconLocation = "$exe,0"
    $lnk.Description = 'Lightweight desktop client for WhatsApp Web'
    $lnk.Save()
}
$startMenu = Join-Path ([Environment]::GetFolderPath('Programs')) "$AppName.lnk"
New-AppShortcut $startMenu
if (-not $NoDesktopShortcut) {
    New-AppShortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) "$AppName.lnk")
}

# Entry in Windows "Installed apps"
$version = (Get-Item $exe).VersionInfo.ProductVersion
New-Item -Path $UninstallKey -Force | Out-Null
$props = @{
    DisplayName     = $AppName
    DisplayIcon     = "$exe,0"
    DisplayVersion  = $version
    Publisher       = $AppName
    InstallLocation = $InstallDir
    UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$InstallDir\uninstall.ps1`""
}
foreach ($k in $props.Keys) { Set-ItemProperty -Path $UninstallKey -Name $k -Value $props[$k] }
Set-ItemProperty -Path $UninstallKey -Name EstimatedSize -Value ([int]((Get-Item $exe).Length / 1KB)) -Type DWord
Set-ItemProperty -Path $UninstallKey -Name NoModify -Value 1 -Type DWord
Set-ItemProperty -Path $UninstallKey -Name NoRepair -Value 1 -Type DWord

if ($Autostart) {
    Set-ItemProperty -Path $RunKey -Name $AppName -Value "`"$exe`""
    Write-Host 'Start with Windows: enabled'
}

Write-Host "Installation complete (version $version)." -ForegroundColor Green
Start-Process $exe
