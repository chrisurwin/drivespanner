use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbShareInfo {
    pub name: String,
    pub path: String,
    pub description: String,
}

/// Returns the path to the persistent upgrade shares state file in ProgramData
pub fn get_upgrade_shares_path() -> PathBuf {
    if let Ok(progdata) = std::env::var("ProgramData") {
        let dir = PathBuf::from(progdata).join("DriveSpanner");
        let _ = std::fs::create_dir_all(&dir);
        return dir.join("upgrade_shares.json");
    }
    PathBuf::from("upgrade_shares.json")
}

/// Discovers active Windows SMB shares mapped to the specified mount letter (e.g. "V:" or "V:\")
pub fn discover_shares_for_mount(mount_letter: &str) -> Vec<SmbShareInfo> {
    let clean_letter = mount_letter.trim_end_matches('\\').to_uppercase();
    info!("Discovering active SMB shares pointing to mount point '{}'...", clean_letter);

    let ps_cmd = format!(
        "Get-SmbShare -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and ($_.Path.ToUpper().StartsWith('{0}') -or $_.Path.ToUpper().StartsWith('{0}:')) }} | Select-Object Name, Path, Description | ConvertTo-Json -Compress",
        clean_letter
    );

    let output = Command::new("powershell.exe")
        .args(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_cmd])
        .output();

    let mut results = Vec::new();

    if let Ok(out) = output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !stdout.is_empty() {
                // Could be single object or array
                if stdout.starts_with('[') {
                    if let Ok(shares) = serde_json::from_str::<Vec<SmbShareInfo>>(&stdout) {
                        results = shares;
                    }
                } else if stdout.starts_with('{') {
                    if let Ok(share) = serde_json::from_str::<SmbShareInfo>(&stdout) {
                        results.push(share);
                    }
                }
            }
        }
    }

    // Fallback: If PowerShell didn't return shares, query net.exe share
    if results.is_empty() {
        if let Ok(out) = Command::new("net.exe").arg("share").output() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let name = parts[0];
                    let resource = parts[1];
                    if resource.to_uppercase().starts_with(&clean_letter) {
                        results.push(SmbShareInfo {
                            name: name.to_string(),
                            path: resource.to_string(),
                            description: "Restored DriveSpanner Pool Share".to_string(),
                        });
                    }
                }
            }
        }
    }

    info!("Discovered {} SMB share(s) mapped to {}", results.len(), clean_letter);
    results
}

/// Saves discovered SMB shares to a persistent JSON state file
pub fn save_upgrade_shares(shares: &[SmbShareInfo]) -> Result<(), String> {
    let path = get_upgrade_shares_path();
    let json = serde_json::to_string_pretty(shares).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    info!("Saved {} share definition(s) to {}", shares.len(), path.display());
    Ok(())
}

/// Pauses the Windows Server Service (LanmanServer) to place active SMB connections into a wait state
pub fn pause_lanman_server() -> Result<(), String> {
    info!("Pausing LanmanServer (holding active SMB connections in wait state)...");
    let output = Command::new("net.exe")
        .args(&["pause", "LanmanServer"])
        .output()
        .map_err(|e| format!("Failed to invoke net.exe pause LanmanServer: {}", e))?;

    if output.status.success() {
        info!("LanmanServer successfully paused. Client connections are held in pending state.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Notice pausing LanmanServer: {}", stderr);
        Ok(())
    }
}

/// Resumes the Windows Server Service (LanmanServer) so held client connections resume transparently
pub fn resume_lanman_server() -> Result<(), String> {
    info!("Resuming LanmanServer (releasing held client connections)...");
    let output = Command::new("net.exe")
        .args(&["continue", "LanmanServer"])
        .output()
        .map_err(|e| format!("Failed to invoke net.exe continue LanmanServer: {}", e))?;

    if output.status.success() {
        info!("LanmanServer successfully resumed. Client connections unpaused.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Notice resuming LanmanServer: {}", stderr);
        Ok(())
    }
}

/// Restores any previously saved SMB shares that may have been dropped during unmount/upgrade
pub fn restore_upgrade_shares() -> usize {
    let path = get_upgrade_shares_path();
    if !path.exists() {
        info!("No saved SMB share state found at {}. Nothing to restore.", path.display());
        return 0;
    }

    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read {}: {}", path.display(), e);
            return 0;
        }
    };

    let shares: Vec<SmbShareInfo> = match serde_json::from_str(&data) {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to parse {}: {}", path.display(), e);
            return 0;
        }
    };

    let mut restored = 0;
    for share in &shares {
        info!("Verifying/restoring SMB share '{}' pointing to '{}'...", share.name, share.path);

        // Check if share currently exists
        let check_cmd = format!("(Get-SmbShare -Name '{}' -ErrorAction SilentlyContinue) -ne $null", share.name);
        let exists = Command::new("powershell.exe")
            .args(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &check_cmd])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "True")
            .unwrap_or(false);

        if !exists {
            info!("Re-creating missing SMB share '{}' -> '{}'...", share.name, share.path);
            let recreate_cmd = format!(
                "New-SmbShare -Name '{0}' -Path '{1}' -Description '{2}' -FullAccess 'Everyone' -ErrorAction SilentlyContinue",
                share.name, share.path, share.description
            );
            let res = Command::new("powershell.exe")
                .args(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &recreate_cmd])
                .output();

            if let Ok(out) = res {
                if out.status.success() {
                    info!("Successfully restored SMB share '{}' via New-SmbShare.", share.name);
                    restored += 1;
                    continue;
                }
            }

            // Fallback: net share
            let net_res = Command::new("net.exe")
                .args(&["share", &format!("{}={}", share.name, share.path), "/GRANT:Everyone,FULL"])
                .output();

            if let Ok(out) = net_res {
                if out.status.success() {
                    info!("Successfully restored SMB share '{}' via net.exe share.", share.name);
                    restored += 1;
                } else {
                    error!("Failed to recreate SMB share '{}': {}", share.name, String::from_utf8_lossy(&out.stderr));
                }
            }
        } else {
            info!("SMB share '{}' is already present and valid.", share.name);
            restored += 1;
        }
    }

    // Clean up state file once restoration is complete
    let _ = std::fs::remove_file(&path);
    restored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smb_share_info_serde() {
        let shares = vec![
            SmbShareInfo {
                name: "PoolShare".to_string(),
                path: "V:\\".to_string(),
                description: "Main Pool Storage".to_string(),
            },
            SmbShareInfo {
                name: "Media".to_string(),
                path: "V:\\Media".to_string(),
                description: "Media Library".to_string(),
            },
        ];

        let json = serde_json::to_string_pretty(&shares).unwrap();
        let decoded: Vec<SmbShareInfo> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].name, "PoolShare");
        assert_eq!(decoded[0].path, "V:\\");
        assert_eq!(decoded[1].name, "Media");
        assert_eq!(decoded[1].path, "V:\\Media");
    }
}
