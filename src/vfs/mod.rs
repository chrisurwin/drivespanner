pub mod fsp_bindings;
pub mod pool_fs;

use crate::core::pool::StoragePool;
use fsp_bindings::{FuseOperations, WinFspDll};
use pool_fs::{create_fuse_operations, init_fs_globals};
use std::ffi::CString;
use std::os::raw::c_char;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use log::{error, info};

static IS_MOUNTED: AtomicBool = AtomicBool::new(false);
static CURRENT_MOUNT: Mutex<Option<String>> = Mutex::new(None);
static WORKER_CHILD: Mutex<Option<Child>> = Mutex::new(None);

#[cfg(windows)]
fn notify_shell_drive_added(letter: char) {
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
        let dll_name = CString::new("shell32.dll").unwrap();
        let h = LoadLibraryA(dll_name.as_ptr() as *const u8);
        if !h.is_null() {
            let func_name = CString::new("SHChangeNotify").unwrap();
            if let Some(proc) = GetProcAddress(h, func_name.as_ptr() as *const u8) {
                type SHChangeNotifyFn = unsafe extern "system" fn(i32, u32, *const u16, *const u16);
                let func: SHChangeNotifyFn = std::mem::transmute(proc);
                let path: Vec<u16> = format!("{}:\\\0", letter).encode_utf16().collect();
                func(0x00000100 /* SHCNE_DRIVEADD */, 0x0005 /* SHCNF_PATHW */, path.as_ptr(), std::ptr::null());
            }
        }
    }
}

#[cfg(not(windows))]
fn notify_shell_drive_added(_letter: char) {}

#[cfg(windows)]
fn notify_shell_drive_removed(letter: char) {
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
        let dll_name = CString::new("shell32.dll").unwrap();
        let h = LoadLibraryA(dll_name.as_ptr() as *const u8);
        if !h.is_null() {
            let func_name = CString::new("SHChangeNotify").unwrap();
            if let Some(proc) = GetProcAddress(h, func_name.as_ptr() as *const u8) {
                type SHChangeNotifyFn = unsafe extern "system" fn(i32, u32, *const u16, *const u16);
                let func: SHChangeNotifyFn = std::mem::transmute(proc);
                let path: Vec<u16> = format!("{}:\\\0", letter).encode_utf16().collect();
                func(0x00000080 /* SHCNE_DRIVEREMOVED */, 0x0005 /* SHCNF_PATHW */, path.as_ptr(), std::ptr::null());
            }
        }
    }
}

#[cfg(not(windows))]
fn notify_shell_drive_removed(_letter: char) {}

pub struct PoolMounter;

impl PoolMounter {
    pub fn is_mounted() -> bool {
        if !IS_MOUNTED.load(Ordering::SeqCst) {
            return false;
        }

        // Verify child worker process is still alive if managed
        if let Ok(mut lock) = WORKER_CHILD.lock() {
            if let Some(ref mut child) = *lock {
                if let Ok(Some(_)) = child.try_wait() {
                    IS_MOUNTED.store(false, Ordering::SeqCst);
                    return false;
                }
            }
        }

        true
    }

    #[allow(dead_code)]
    pub fn current_mount_point() -> Option<String> {
        CURRENT_MOUNT.lock().unwrap().clone()
    }

    /// Dedicated worker entry point executed in an independent child process.
    /// Runs the WinFsp FUSE dispatcher in console mode to avoid Service Control Manager collisions.
    pub fn run_worker(mount_point: &str) {
        let clean_mount = mount_point.trim().trim_end_matches(['\\', '/']).to_string();
        let letter = clean_mount
            .chars()
            .find(|c| c.is_ascii_alphabetic())
            .unwrap_or('V')
            .to_ascii_uppercase();
        let target_mount = format!("{}:", letter);

        info!("Starting PoolForge mount worker on {}", target_mount);

        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                error!("Mount worker failed to create Tokio runtime: {}", e);
                std::process::exit(1);
            }
        };
        let _guard = rt.enter();

        let config_path = crate::config::PoolConfig::get_config_path();
        let cfg = crate::config::PoolConfig::load_or_default(&config_path);
        let shared_cfg = Arc::new(tokio::sync::RwLock::new(cfg));
        let (pool, _rep_handle) = StoragePool::new(shared_cfg);
        let pool_arc = Arc::new(pool);

        if let Err(e) = rt.block_on(pool_arc.initialize_from_config()) {
            error!("Mount worker failed to initialize pool: {}", e);
        }

        let winfsp = match WinFspDll::load() {
            Ok(w) => w,
            Err(e) => {
                error!("Mount worker failed to load WinFsp: {}", e);
                std::process::exit(1);
            }
        };

        init_fs_globals(pool_arc, Some(rt.handle().clone()));

        let opt_str = "FileSystemName=PoolForge,volname=PoolForge,FileInfoTimeout=5000,DirInfoTimeout=5000,VolumeInfoTimeout=10000,ThreadCount=16".to_string();
        let args_strings = vec![
            "poolforge".to_string(),
            target_mount.clone(),
            "-f".to_string(),
            "-o".to_string(),
            opt_str,
        ];

        let c_args: Vec<CString> = args_strings
            .into_iter()
            .map(|s| CString::new(s).unwrap())
            .collect();

        let mut c_argv: Vec<*const c_char> = c_args.iter().map(|c| c.as_ptr()).collect();
        c_argv.push(std::ptr::null());

        let ops = create_fuse_operations();
        let opsize = std::mem::size_of::<FuseOperations>();

        info!("Mount worker dispatching WinFsp FUSE on {}", target_mount);

        let res = unsafe {
            (winfsp.fuse_main_real)(
                (c_argv.len() - 1) as i32,
                c_argv.as_ptr(),
                &ops,
                opsize,
                std::ptr::null_mut(),
            )
        };

        info!("Mount worker WinFsp dispatcher exited with code {}", res);
        std::process::exit(res);
    }

    /// Mounts the pooled storage as a native Windows drive letter using a dedicated worker process
    pub fn start_mount(
        pool: Arc<StoragePool>,
        mount_point: String,
    ) -> Result<(), String> {
        if Self::is_mounted() {
            return Err("Pool is already mounted".to_string());
        }

        let clean_mount = mount_point.trim().trim_end_matches(['\\', '/']).to_string();
        let letter = clean_mount
            .chars()
            .find(|c| c.is_ascii_alphabetic())
            .unwrap_or('V')
            .to_ascii_uppercase();
        let target_mount = format!("{}:", letter);

        // Pre-flight check: ensure the target drive letter is not already assigned to a member disk
        {
            let disks = pool.disks.read().unwrap();
            for d in disks.iter() {
                let d_letter = d.drive_path.to_string_lossy().chars().next().unwrap_or(' ').to_ascii_uppercase();
                if d_letter == letter {
                    return Err(format!(
                        "Drive letter {}: is a physical member disk in the storage pool and cannot be used as the virtual pool mount point.",
                        target_mount
                    ));
                }
            }
        }

        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Failed to locate poolforge executable: {}", e))?;

        info!("Starting mount worker on {} with {:?}", target_mount, exe_path);

        // Clean up any stale worker handle
        {
            let mut lock = WORKER_CHILD.lock().unwrap();
            if let Some(ref mut c) = *lock {
                let _ = c.kill();
                let _ = c.wait();
            }
            *lock = None;
        }

        let mut child = Command::new(&exe_path)
            .args(&["mount-worker", &target_mount])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn mount worker process: {}", e))?;

        // Wait up to 3.5 seconds to confirm the worker process started successfully and didn't crash
        let root_path = format!("{}:\\", letter);
        let mut mounted = false;

        for _ in 0..35 {
            std::thread::sleep(std::time::Duration::from_millis(100));

            // Check if worker exited prematurely with error
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("Mount worker process exited unexpectedly with {}", status));
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = child.kill();
                    return Err(format!("Failed to query mount worker status: {}", e));
                }
            }

            if Path::new(&root_path).exists() {
                mounted = true;
                break;
            }
        }

        if !mounted {
            let is_alive = matches!(child.try_wait(), Ok(None));
            if !is_alive {
                return Err(format!("Mount worker exited before {} could be mounted", target_mount));
            }
        }

        {
            let mut lock = WORKER_CHILD.lock().unwrap();
            *lock = Some(child);
        }

        IS_MOUNTED.store(true, Ordering::SeqCst);
        *CURRENT_MOUNT.lock().unwrap() = Some(target_mount.clone());

        let tokio_handle = tokio::runtime::Handle::try_current().ok();
        if let Some(ref handle) = tokio_handle {
            let p = pool.clone();
            handle.spawn(async move {
                *p.is_mounted.write().await = true;
            });
        }

        notify_shell_drive_added(letter);

        // Spawn monitor thread to automatically update IS_MOUNTED if the worker process ever exits
        let pool_monitor = pool.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let exited = {
                    let mut lock = WORKER_CHILD.lock().unwrap();
                    if let Some(ref mut c) = *lock {
                        matches!(c.try_wait(), Ok(Some(_)) | Err(_))
                    } else {
                        true
                    }
                };

                if exited {
                    IS_MOUNTED.store(false, Ordering::SeqCst);
                    *CURRENT_MOUNT.lock().unwrap() = None;
                    let tokio_handle = tokio::runtime::Handle::try_current().ok();
                    if let Some(ref handle) = tokio_handle {
                        let p = pool_monitor.clone();
                        handle.spawn(async move {
                            *p.is_mounted.write().await = false;
                        });
                    }
                    info!("Mount worker exited; virtual drive unmounted.");
                    break;
                }
            }
        });

        info!("Virtual storage pool successfully mounted on {}", target_mount);
        Ok(())
    }

    /// Gracefully unmounts the virtual pool by terminating the mount worker
    pub fn stop_mount(pool: Arc<StoragePool>) -> Result<(), String> {
        let child_opt = WORKER_CHILD.lock().unwrap().take();
        if let Some(mut child) = child_opt {
            info!("Stopping mount worker process (PID: {})...", child.id());
            let _ = child.kill();
            let _ = child.wait();
            info!("Mount worker process terminated.");
        }

        let current = CURRENT_MOUNT.lock().unwrap().take();
        if let Some(ref mount) = current {
            let letter = mount.chars().next().unwrap_or('V').to_ascii_uppercase();
            notify_shell_drive_removed(letter);
        }

        IS_MOUNTED.store(false, Ordering::SeqCst);

        let tokio_handle = tokio::runtime::Handle::try_current().ok();
        if let Some(ref handle) = tokio_handle {
            let p = pool.clone();
            handle.spawn(async move {
                *p.is_mounted.write().await = false;
            });
        }

        Ok(())
    }
}

