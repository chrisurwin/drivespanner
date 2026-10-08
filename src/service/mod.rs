use log::{error, info};
use std::ffi::OsString;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

pub const SERVICE_NAME: &str = "PoolForgeService";
pub const SERVICE_DISPLAY_NAME: &str = "PoolForge Storage Service";

pub fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

define_windows_service!(ffi_service_main, poolforge_service_main);

fn poolforge_service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service_impl() {
        error!("PoolForge Windows Service encountered fatal error: {}", e);
    }
}

fn run_service_impl() -> Result<(), Box<dyn std::error::Error>> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let status_handle = service_control_handler::register(SERVICE_NAME, move |control_event| {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                info!("Service Control Manager requested service stop.");
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })?;

    // Report RUNNING status to SCM immediately so SCM never hits timeout 1053!
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    info!("PoolForge Windows Service registered and reported RUNNING to SCM.");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let (tokio_stop_tx, tokio_stop_rx) = tokio::sync::oneshot::channel::<()>();

    std::thread::spawn(move || {
        if let Ok(()) = stop_rx.recv() {
            let _ = tokio_stop_tx.send(());
        }
    });

    rt.block_on(async {
        if let Err(e) = crate::async_main(Some(tokio_stop_rx)).await {
            error!("PoolForge daemon error: {}", e);
        }
    });

    // Report STOPPED status to SCM
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });

    info!("PoolForge Windows Service stopped cleanly.");
    Ok(())
}

pub struct ServiceManager;

impl ServiceManager {
    pub fn install() -> Result<(), String> {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Failed to get executable path: {}", e))?;
        let exe_str = exe_path.to_string_lossy();

        let bin_path_arg = format!("\"{}\" service run", exe_str);

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
