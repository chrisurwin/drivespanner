use log::info;
use std::process::Command;

pub const SERVICE_NAME: &str = "PoolForgeService";
pub const SERVICE_DISPLAY_NAME: &str = "PoolForge Storage Service";

pub struct ServiceManager;

impl ServiceManager {
    pub fn install() -> Result<(), String> {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Failed to get executable path: {}", e))?;
        let exe_str = exe_path.to_string_lossy();

        let bin_path_arg = format!("\"{}\" run", exe_str);

        info!("Registering Windows Service '{}'...", SERVICE_NAME);
        let output = Command::new("sc.exe")
            .args(&[
                "create",
                SERVICE_NAME,
                &format!("binPath={}", bin_path_arg),
                "start=auto",
                &format!("DisplayName={}", SERVICE_DISPLAY_NAME),
            ])
            .output()
            .map_err(|e| format!("Failed to run sc.exe: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            info!("Service installed successfully: {}", stdout.trim());
            let _ = Command::new("sc.exe")
                .args(&[
                    "description",
                    SERVICE_NAME,
                    "PoolForge - High-performance storage pooling and replication engine for Windows.",
                ])
                .output();
            Ok(())
        } else {
            Err(format!("sc.exe error: {} {}", stdout.trim(), stderr.trim()))
        }
    }

    pub fn uninstall() -> Result<(), String> {
        info!("Stopping Windows Service '{}' if running...", SERVICE_NAME);
        let _ = Command::new("sc.exe").args(&["stop", SERVICE_NAME]).output();

        info!("Deleting Windows Service '{}'...", SERVICE_NAME);
        let output = Command::new("sc.exe")
            .args(&["delete", SERVICE_NAME])
            .output()
            .map_err(|e| format!("Failed to run sc.exe: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() {
            info!("Service uninstalled successfully.");
            Ok(())
        } else {
            Err(format!("sc.exe delete failed: {}", stdout.trim()))
        }
    }

    pub fn start() -> Result<(), String> {
        info!("Starting Windows Service '{}'...", SERVICE_NAME);
        let output = Command::new("net.exe")
            .args(&["start", SERVICE_NAME])
            .output()
            .map_err(|e| format!("Failed to start service: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            info!("Service started: {}", stdout.trim());
            Ok(())
        } else {
            Err(format!("Failed to start: {} {}", stdout.trim(), stderr.trim()))
        }
    }

    pub fn stop() -> Result<(), String> {
        info!("Stopping Windows Service '{}'...", SERVICE_NAME);
        let output = Command::new("net.exe")
            .args(&["stop", SERVICE_NAME])
            .output()
            .map_err(|e| format!("Failed to stop service: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            info!("Service stopped: {}", stdout.trim());
            Ok(())
        } else {
            Err(format!("Failed to stop: {} {}", stdout.trim(), stderr.trim()))
        }
    }
}
