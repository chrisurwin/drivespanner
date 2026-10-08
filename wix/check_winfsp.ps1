# DriveSpanner WinFsp Prerequisite Installer
param (
    [switch]$Silent = $false
)

$ErrorActionPreference = "Stop"

function Test-WinFspInstalled {
    $paths = @(
        "HKLM:\SOFTWARE\WinFsp",
        "HKLM:\SOFTWARE\WOW6432Node\WinFsp"
    )
    foreach ($p in $paths) {
        if (Test-Path $p) {
            return $true
        }
    }

    $dllPaths = @(
        "${env:ProgramFiles(x86)}\WinFsp\bin\winfsp-x64.dll",
        "${env:ProgramFiles}\WinFsp\bin\winfsp-x64.dll"
    )
    foreach ($d in $dllPaths) {
        if (Test-Path $d) {
            return $true
        }
    }

    return $false
}

Write-Host "Checking for WinFsp prerequisite..."
if (Test-WinFspInstalled) {
    Write-Host "WinFsp is already installed." -ForegroundColor Green
    exit 0
}

Write-Host "WinFsp is not detected on this system." -ForegroundColor Yellow

$shouldInstall = $true

if (-not $Silent) {
    Add-Type -AssemblyName PresentationFramework
    $msg = "DriveSpanner requires the WinFsp kernel driver for virtual filesystem mounting.`n`nWould you like to automatically download and install WinFsp now?"
    $title = "DriveSpanner Prerequisites - WinFsp Required"
    $result = [System.Windows.MessageBox]::Show($msg, $title, [System.Windows.MessageBoxButton]::YesNo, [System.Windows.MessageBoxImage]::Question)

    if ($result -ne [System.Windows.MessageBoxResult]::Yes) {
        Write-Warning "WinFsp installation declined by user."
        exit 1
    }
}

$winfspUrl = "https://github.com/winfsp/winfsp/releases/download/v2.0/winfsp-2.0.23075.msi"
$tempMsi = Join-Path $env:TEMP "winfsp-prereq.msi"

Write-Host "Downloading WinFsp installer from $winfspUrl..."
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri $winfspUrl -OutFile $tempMsi -UseBasicParsing

Write-Host "Installing WinFsp silently..."
$process = Start-Process msiexec.exe -ArgumentList "/i `"$tempMsi`" /passive /norestart" -Wait -PassThru

if ($process.ExitCode -eq 0 -or $process.ExitCode -eq 3010) {
    Write-Host "WinFsp installed successfully!" -ForegroundColor Green
    Remove-Item $tempMsi -Force -ErrorAction SilentlyContinue
    exit 0
} else {
    Write-Error "WinFsp installation failed with exit code: $($process.ExitCode)"
    exit 1
}
