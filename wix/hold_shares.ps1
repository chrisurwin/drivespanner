# DriveSpanner SMB Network Share Preservation & Connection Holding Script
param(
    [Parameter(Position=0)]
    [ValidateSet("prepare", "resume", "status")]
    [string]$Action = "status"
)

$ErrorActionPreference = "Continue"

function Get-PoolMountLetter {
    $searchPaths = @(
        (Join-Path $PSScriptRoot "config.json"),
        "${env:ProgramFiles}\DriveSpanner\config.json",
        "${env:ProgramData}\DriveSpanner\config.json",
        "C:\Program Files\DriveSpanner\config.json"
    )

    foreach ($p in $searchPaths) {
        if (Test-Path $p) {
            try {
                $raw = Get-Content -Path $p -Raw -ErrorAction SilentlyContinue
                if ($raw) {
                    $json = $raw | ConvertFrom-Json -ErrorAction SilentlyContinue
                    if ($json -and $json.mount_point) {
                        return $json.mount_point.TrimEnd('\').ToUpper()
                    }
                }
            } catch {}
        }
    }
    return "V:"
}

$stateDir = "${env:ProgramData}\DriveSpanner"
$stateFile = Join-Path $stateDir "upgrade_shares.json"

switch ($Action) {
    "prepare" {
        Write-Host "Preparing DriveSpanner for upgrade..." -ForegroundColor Cyan
        $mountLetter = Get-PoolMountLetter
        Write-Host "Target pool mount letter: $mountLetter"

        # 1. Discover active SMB shares mapped to this mount point
        $sharesToSave = @()
        try {
            $allShares = Get-SmbShare -ErrorAction SilentlyContinue
            if ($allShares) {
                foreach ($s in $allShares) {
                    if ($s.Path -and ($s.Path.ToUpper().StartsWith($mountLetter))) {
                        Write-Host "Found active SMB share: '$($s.Name)' -> '$($s.Path)'" -ForegroundColor Yellow
                        $sharesToSave += [PSCustomObject]@{
                            name = $s.Name
                            path = $s.Path
                            description = if ($s.Description) { $s.Description } else { "Restored DriveSpanner Share" }
                        }
                    }
                }
            }
        } catch {
            Write-Warning "Get-SmbShare error: $_"
        }

        # 2. Persist shares state
        if ($sharesToSave.Count -gt 0) {
            if (-not (Test-Path $stateDir)) {
                New-Item -ItemType Directory -Path $stateDir -Force | Out-Null
            }
            $sharesToSave | ConvertTo-Json -Depth 3 | Set-Content -Path $stateFile -Force
            Write-Host "Saved $($sharesToSave.Count) SMB share(s) to $stateFile" -ForegroundColor Green

            # 3. Put LanmanServer in paused wait state so active client connections hold rather than abort
            Write-Host "Holding network connections in wait state (pausing LanmanServer)..." -ForegroundColor Cyan
            & net.exe pause LanmanServer 2>&1 | Out-Null
        } else {
            Write-Host "No active network shares mapped to $mountLetter."
        }

        # 4. Gracefully stop services and unmount pool
        Write-Host "Stopping DriveSpannerService..."
        & net.exe stop DriveSpannerService 2>&1 | Out-Null
        & net.exe stop PoolForgeService 2>&1 | Out-Null
        & sc.exe delete PoolForgeService 2>&1 | Out-Null

        # Terminate any lingering drivespanner worker processes
        & taskkill.exe /F /IM drivespanner.exe 2>&1 | Out-Null
        Write-Host "Upgrade preparation complete." -ForegroundColor Green
        exit 0
    }

    "resume" {
        Write-Host "Resuming DriveSpanner after upgrade..." -ForegroundColor Cyan
        $mountLetter = Get-PoolMountLetter

        # 1. Start the Windows Service
        Write-Host "Starting DriveSpannerService..."
        & net.exe start DriveSpannerService 2>&1 | Out-Null

        # 2. Wait for the virtual drive to mount
        $mountRoot = "$mountLetter\"
        Write-Host "Waiting for virtual drive $mountRoot to become ready..."
        $ready = $false
        for ($i = 0; $i -lt 30; $i++) {
            if (Test-Path $mountRoot) {
                $ready = $true
                break
            }
            Start-Sleep -Milliseconds 500
        }

        if ($ready) {
            Write-Host "Virtual drive $mountRoot is online!" -ForegroundColor Green
        } else {
            Write-Warning "Virtual drive $mountRoot not detected within 15 seconds."
        }

        # 3. Restore any SMB shares from saved state
        if (Test-Path $stateFile) {
            try {
                $saved = Get-Content -Path $stateFile -Raw | ConvertFrom-Json
                if ($saved) {
                    if ($saved -isnot [System.Array]) { $saved = @($saved) }
                    foreach ($sh in $saved) {
                        $existing = Get-SmbShare -Name $sh.name -ErrorAction SilentlyContinue
                        if (-not $existing) {
                            Write-Host "Restoring dropped SMB share '$($sh.name)' -> '$($sh.path)'..." -ForegroundColor Yellow
                            try {
                                New-SmbShare -Name $sh.name -Path $sh.path -Description $sh.description -FullAccess "Everyone" -ErrorAction Stop | Out-Null
                                Write-Host "Share '$($sh.name)' successfully restored." -ForegroundColor Green
                            } catch {
                                & net.exe share "$($sh.name)=$($sh.path)" /GRANT:Everyone,FULL 2>&1 | Out-Null
                            }
                        } else {
                            Write-Host "Share '$($sh.name)' is already present." -ForegroundColor Green
                        }
                    }
                }
            } catch {
                Write-Warning "Error restoring shares: $_"
            }
            Remove-Item -Path $stateFile -Force -ErrorAction SilentlyContinue
        }

        # 4. Resume LanmanServer to release held client connections
        Write-Host "Releasing network connections from wait state (resuming LanmanServer)..." -ForegroundColor Cyan
        & net.exe continue LanmanServer 2>&1 | Out-Null

        Write-Host "DriveSpanner upgrade completed successfully. Network connections resumed." -ForegroundColor Green
        exit 0
    }

    "status" {
        $mountLetter = Get-PoolMountLetter
        Write-Host "DriveSpanner Mount: $mountLetter"
        $shares = Get-SmbShare -ErrorAction SilentlyContinue | Where-Object { $_.Path -and ($_.Path.ToUpper().StartsWith($mountLetter)) }
        if ($shares) {
            Write-Host "Active Shares on $mountLetter:" -ForegroundColor Green
            $shares | Format-Table -AutoSize
        } else {
            Write-Host "No active shares on $mountLetter."
        }
    }
}
