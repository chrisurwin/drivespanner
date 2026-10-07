# PoolForge WiX MSI Build Automation Script
param (
    [string]$Version = "0.1.0",
    [string]$SourceDir = "",
    [string]$OutputDir = "target\installer"
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$projectRoot = Resolve-Path (Join-Path $scriptDir "..")

if ([string]::IsNullOrEmpty($SourceDir)) {
    # Check release target directory
    $possibleSources = @(
        (Join-Path $projectRoot "target\release"),
        (Join-Path $projectRoot "target\x86_64-pc-windows-gnu\release"),
        (Join-Path $projectRoot "target\x86_64-pc-windows-msvc\release")
    )

    foreach ($dir in $possibleSources) {
        if (Test-Path (Join-Path $dir "poolforge.exe")) {
            $SourceDir = $dir
            break
        }
    }
} else {
    if (-not [System.IO.Path]::IsPathRooted($SourceDir)) {
        $SourceDir = Join-Path $projectRoot $SourceDir
    }
}

if (-not (Test-Path (Join-Path $SourceDir "poolforge.exe"))) {
    Write-Error "Could not find poolforge.exe in '$SourceDir'. Run 'cargo build --release' first."
    exit 1
}

Write-Host "Using binary source directory: $SourceDir"

# Copy default config and helper script into staging area if not present
$stagingDir = Join-Path $projectRoot "target\wix_staging"
New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
Copy-Item (Join-Path $SourceDir "poolforge.exe") (Join-Path $stagingDir "poolforge.exe") -Force
Copy-Item (Join-Path $scriptDir "default_config.json") (Join-Path $stagingDir "config.json") -Force
Copy-Item (Join-Path $scriptDir "check_winfsp.ps1") (Join-Path $stagingDir "check_winfsp.ps1") -Force
Copy-Item (Join-Path $scriptDir "validate_drive.js") (Join-Path $stagingDir "validate_drive.js") -Force
Copy-Item (Join-Path $scriptDir "poolforge.ico") (Join-Path $stagingDir "poolforge.ico") -Force
Copy-Item (Join-Path $scriptDir "banner.bmp") (Join-Path $stagingDir "banner.bmp") -Force
Copy-Item (Join-Path $scriptDir "dialog.bmp") (Join-Path $stagingDir "dialog.bmp") -Force

# Locate WiX tools
$wixBins = @(
    "candle.exe",
    "${env:ProgramFiles(x86)}\WiX Toolset v3.11\bin\candle.exe",
    "${env:ProgramFiles}\WiX Toolset v3.11\bin\candle.exe",
    "${env:WIX}\bin\candle.exe"
)

$candlePath = $null
foreach ($c in $wixBins) {
    if (Get-Command $c -ErrorAction SilentlyContinue) {
        $candlePath = (Get-Command $c).Source
        break
    } elseif (Test-Path $c) {
        $candlePath = $c
        break
    }
}

if (-not $candlePath) {
    Write-Error "WiX Toolset (candle.exe / light.exe) was not found. Please install WiX Toolset v3.11 or run on GitHub Actions."
    exit 1
}

$wixDir = Split-Path -Parent $candlePath
$lightPath = Join-Path $wixDir "light.exe"

New-Item -ItemType Directory -Force -Path (Join-Path $projectRoot $OutputDir) | Out-Null
$wxsFile = Join-Path $scriptDir "poolforge.wxs"
$wixObj = Join-Path $projectRoot "target\poolforge.wixobj"
$outputMsi = Join-Path (Join-Path $projectRoot $OutputDir) "PoolForge-$Version-Setup.msi"

Write-Host "Compiling WiX source: $wxsFile..."
& $candlePath "-ext" "WixUIExtension" "-dVersion=$Version" "-dSourceDir=$stagingDir" "-out" $wixObj $wxsFile
if ($LASTEXITCODE -ne 0) {
    Write-Error "Candle compilation failed with code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host "Linking MSI package: $outputMsi..."
& $lightPath "-ext" "WixUIExtension" "-sval" "-out" $outputMsi $wixObj
if ($LASTEXITCODE -ne 0) {
    Write-Error "Light linking failed with code $LASTEXITCODE"
    exit $LASTEXITCODE
}

Write-Host "MSI generated successfully: $outputMsi" -ForegroundColor Green
