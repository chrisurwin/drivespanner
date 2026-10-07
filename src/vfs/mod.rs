pub mod fsp_bindings;
pub mod pool_fs;

use crate::core::pool::StoragePool;
use fsp_bindings::{FuseOperations, WinFspDll};
use pool_fs::{create_fuse_operations, init_fs_globals};
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use log::info;

static IS_MOUNTED: AtomicBool = AtomicBool::new(false);

pub struct PoolMounter;

impl PoolMounter {
    /// Mounts the pooled storage as a native Windows drive letter using WinFsp
    pub fn start_mount(
        pool: Arc<StoragePool>,
        mount_point: String,
    ) -> Result<(), String> {
        if IS_MOUNTED.load(Ordering::SeqCst) {
            return Err("Pool is already mounted".to_string());
        }

        let winfsp = WinFspDll::load().map_err(|e| format!("WinFsp load error: {}", e))?;
        let tokio_handle = tokio::runtime::Handle::try_current().ok();
        init_fs_globals(pool.clone(), tokio_handle.clone());

        let pool_for_thread = pool.clone();
        let mount_point_clone = mount_point.clone();

        thread::spawn(move || {
            info!("Initializing WinFsp virtual mount on {}", mount_point_clone);

            let clean_mount = mount_point_clone.trim().trim_end_matches('\\').to_string();

            let opt_str = "FileSystemName=PoolForge,FileInfoTimeout=5000,DirInfoTimeout=5000,VolumeInfoTimeout=10000,ThreadCount=16".to_string();
            let args_strings = vec![
                "poolforge".to_string(),
                "-f".to_string(),
                "-m".to_string(),
                clean_mount.clone(),
                "-o".to_string(),
                opt_str,
            ];

            let c_args: Vec<CString> = args_strings
                .into_iter()
                .map(|s| CString::new(s).unwrap())
                .collect();

            let c_argv: Vec<*const c_char> = c_args.iter().map(|c| c.as_ptr()).collect();

            let ops = create_fuse_operations();
            let opsize = std::mem::size_of::<FuseOperations>();

            IS_MOUNTED.store(true, Ordering::SeqCst);
            if let Some(ref handle) = tokio_handle {
                let p = pool_for_thread.clone();
                handle.spawn(async move {
                    *p.is_mounted.write().await = true;
                });
            }

            info!("WinFsp filesystem dispatcher running on {}", clean_mount);
            let res = unsafe {
                (winfsp.fuse_main_real)(
                    c_argv.len() as i32,
                    c_argv.as_ptr(),
                    &ops,
                    opsize,
                    std::ptr::null_mut(),
                )
            };

            info!("WinFsp filesystem dispatcher exited with code {}", res);
            IS_MOUNTED.store(false, Ordering::SeqCst);
            if let Some(ref handle) = tokio_handle {
                let p = pool_for_thread.clone();
                handle.spawn(async move {
                    *p.is_mounted.write().await = false;
                });
            }
        });

        Ok(())
    }
}
