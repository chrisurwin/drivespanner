use libc::{c_char, c_int, c_void, size_t, uint32_t, uint64_t};
use std::ffi::CString;
use std::mem;
use std::sync::atomic::{AtomicPtr, Ordering};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

pub type FuseFillDirFn = unsafe extern "C" fn(
    buf: *mut c_void,
    name: *const c_char,
    stbuf: *const FuseStat,
    off: i64,
) -> c_int;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FuseTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FuseStat {
    pub st_dev: u32,
    pub _pad0: u32,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u16,
    pub _pad1: u16,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u32,
    pub _pad2: u32,
    pub st_size: i64,
    pub st_atim: FuseTimespec,
    pub st_mtim: FuseTimespec,
    pub st_ctim: FuseTimespec,
    pub st_blksize: i32,
    pub _pad3: i32,
    pub st_blocks: i64,
    pub st_birthtim: FuseTimespec,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FuseFileInfo {
    pub flags: i32,
    pub fh_old: u32,
    pub writepage: i32,
    pub bitfields: u32,
    pub fh: u64,
    pub lock_owner: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FuseStatVfs {
    pub f_bsize: u64,
    pub f_frsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_favail: u64,
    pub f_fsid: u64,
    pub f_flag: u64,
    pub f_namemax: u64,
}

#[repr(C)]
#[derive(Default)]
pub struct FuseOperations {
    pub getattr: Option<unsafe extern "C" fn(path: *const c_char, stbuf: *mut FuseStat) -> c_int>,
    pub readlink: Option<unsafe extern "C" fn(path: *const c_char, buf: *mut c_char, size: size_t) -> c_int>,
    pub getdir: *const c_void,
    pub mknod: Option<unsafe extern "C" fn(path: *const c_char, mode: u32, dev: u64) -> c_int>,
    pub mkdir: Option<unsafe extern "C" fn(path: *const c_char, mode: u32) -> c_int>,
    pub unlink: Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,
    pub rmdir: Option<unsafe extern "C" fn(path: *const c_char) -> c_int>,
    pub symlink: Option<unsafe extern "C" fn(dstpath: *const c_char, srcpath: *const c_char) -> c_int>,
    pub rename: Option<unsafe extern "C" fn(oldpath: *const c_char, newpath: *const c_char) -> c_int>,
    pub link: *const c_void,
    pub chmod: Option<unsafe extern "C" fn(path: *const c_char, mode: u32) -> c_int>,
    pub chown: Option<unsafe extern "C" fn(path: *const c_char, uid: u32, gid: u32) -> c_int>,
    pub truncate: Option<unsafe extern "C" fn(path: *const c_char, size: i64) -> c_int>,
    pub utime: *const c_void,
    pub open: Option<unsafe extern "C" fn(path: *const c_char, fi: *mut FuseFileInfo) -> c_int>,
    pub read: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            buf: *mut c_char,
            size: size_t,
            off: i64,
            fi: *mut FuseFileInfo,
        ) -> c_int,
    >,
    pub write: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            buf: *const c_char,
            size: size_t,
            off: i64,
            fi: *mut FuseFileInfo,
        ) -> c_int,
    >,
    pub statfs: Option<unsafe extern "C" fn(path: *const c_char, stbuf: *mut FuseStatVfs) -> c_int>,
    pub flush: Option<unsafe extern "C" fn(path: *const c_char, fi: *mut FuseFileInfo) -> c_int>,
    pub release: Option<unsafe extern "C" fn(path: *const c_char, fi: *mut FuseFileInfo) -> c_int>,
    pub fsync: Option<unsafe extern "C" fn(path: *const c_char, datasync: c_int, fi: *mut FuseFileInfo) -> c_int>,
    pub setxattr: *const c_void,
    pub getxattr: *const c_void,
    pub listxattr: *const c_void,
    pub removexattr: *const c_void,
    pub opendir: Option<unsafe extern "C" fn(path: *const c_char, fi: *mut FuseFileInfo) -> c_int>,
    pub readdir: Option<
        unsafe extern "C" fn(
            path: *const c_char,
            buf: *mut c_void,
            filler: FuseFillDirFn,
            off: i64,
            fi: *mut FuseFileInfo,
        ) -> c_int,
    >,
    pub releasedir: Option<unsafe extern "C" fn(path: *const c_char, fi: *mut FuseFileInfo) -> c_int>,
    pub fsyncdir: *const c_void,
    pub init: Option<unsafe extern "C" fn(conn: *mut c_void) -> *mut c_void>,
    pub destroy: Option<unsafe extern "C" fn(data: *mut c_void)>,
    pub access: Option<unsafe extern "C" fn(path: *const c_char, mask: c_int) -> c_int>,
    pub create: Option<unsafe extern "C" fn(path: *const c_char, mode: u32, fi: *mut FuseFileInfo) -> c_int>,
    pub ftruncate: Option<unsafe extern "C" fn(path: *const c_char, off: i64, fi: *mut FuseFileInfo) -> c_int>,
    pub fgetattr: Option<unsafe extern "C" fn(path: *const c_char, stbuf: *mut FuseStat, fi: *mut FuseFileInfo) -> c_int>,
    pub lock: *const c_void,
    pub utimens: Option<unsafe extern "C" fn(path: *const c_char, tv: *const FuseTimespec) -> c_int>,
    pub bmap: *const c_void,
    pub flags: u32,
    pub ioctl: *const c_void,
}

pub type FuseMainRealFn = unsafe extern "C" fn(
    argc: c_int,
    argv: *const *const c_char,
    ops: *const FuseOperations,
    opsize: size_t,
    data: *mut c_void,
) -> c_int;

pub type FuseExitFn = unsafe extern "C" fn(f: *mut c_void);
pub type FuseGetContextFn = unsafe extern "C" fn() -> *mut c_void;

pub struct WinFspDll {
    pub handle: *mut c_void,
    pub fuse_main_real: FuseMainRealFn,
    pub fuse_exit: Option<FuseExitFn>,
}

unsafe impl Send for WinFspDll {}
unsafe impl Sync for WinFspDll {}

static WINFSP_INSTANCE: AtomicPtr<WinFspDll> = AtomicPtr::new(std::ptr::null_mut());

impl WinFspDll {
    pub fn load() -> Result<&'static WinFspDll, String> {
        let current = WINFSP_INSTANCE.load(Ordering::SeqCst);
        if !current.is_null() {
            return Ok(unsafe { &*current });
        }

        let search_paths = [
            "winfsp-x64.dll",
            r"C:\Program Files (x86)\WinFsp\bin\winfsp-x64.dll",
            r"C:\Program Files\WinFsp\bin\winfsp-x64.dll",
        ];

        let mut module = std::ptr::null_mut();
        for path in &search_paths {
            let c_path = CString::new(*path).unwrap();
            let h = unsafe { LoadLibraryA(c_path.as_ptr() as *const u8) };
            if !h.is_null() {
                module = h;
                break;
            }
        }

        if module.is_null() {
            return Err("WinFsp DLL (winfsp-x64.dll) could not be loaded. Please ensure WinFsp is installed.".to_string());
        }

        let fuse_main_sym = CString::new("fuse_main_real").unwrap();
        let fuse_main_proc = unsafe { GetProcAddress(module, fuse_main_sym.as_ptr() as *const u8) };
        if fuse_main_proc.is_none() {
            return Err("Symbol 'fuse_main_real' not found in winfsp-x64.dll".to_string());
        }

        let fuse_main_real: FuseMainRealFn = unsafe { mem::transmute(fuse_main_proc.unwrap()) };

        let fuse_exit_sym = CString::new("fuse_exit").unwrap();
        let fuse_exit_proc = unsafe { GetProcAddress(module, fuse_exit_sym.as_ptr() as *const u8) };
        let fuse_exit: Option<FuseExitFn> = fuse_exit_proc.map(|p| unsafe { mem::transmute(p) });

        let boxed = Box::new(WinFspDll {
            handle: module,
            fuse_main_real,
            fuse_exit,
        });

        let raw = Box::into_raw(boxed);
        WINFSP_INSTANCE.store(raw, Ordering::SeqCst);
        Ok(unsafe { &*raw })
    }
}
