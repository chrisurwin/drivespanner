use crate::core::placement::{PlacementEngine, PlacementPolicy};
use crate::core::pool::StoragePool;
use crate::vfs::fsp_bindings::*;
use libc::{c_char, c_int, c_void, size_t};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use log::{error, info};

// POSIX error constants
const ENOENT: c_int = 2;
const EIO: c_int = 5;
const EACCES: c_int = 13;
const ENOSPC: c_int = 28;

pub struct OpenFileHandle {
    pub file: File,
    pub rel_path: String,
    pub is_write: bool,
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(100);
static GLOBAL_HANDLES: Mutex<Option<HashMap<u64, OpenFileHandle>>> = Mutex::new(None);
static GLOBAL_POOL: Mutex<Option<Arc<StoragePool>>> = Mutex::new(None);
static TOKIO_HANDLE: Mutex<Option<tokio::runtime::Handle>> = Mutex::new(None);

#[derive(Clone, Debug)]
pub enum CachedNode {
    Directory,
    File {
        disk_id: String,
        size: i64,
        modified: i64,
    },
    Negative,
}

struct CacheEntry {
    node: CachedNode,
    instant: std::time::Instant,
}

static PATH_CACHE: Mutex<Option<HashMap<String, CacheEntry>>> = Mutex::new(None);

pub fn init_fs_globals(pool: Arc<StoragePool>, rt_handle: Option<tokio::runtime::Handle>) {
    *GLOBAL_HANDLES.lock().unwrap() = Some(HashMap::new());
    *GLOBAL_POOL.lock().unwrap() = Some(pool);
    *TOKIO_HANDLE.lock().unwrap() = rt_handle;
    *PATH_CACHE.lock().unwrap() = Some(HashMap::new());
}

pub fn path_to_inode(path: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.to_ascii_lowercase().bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if hash == 0 { 1 } else { hash }
}

pub fn normalize_cache_key(path: &str) -> String {
    let mut clean = path.replace('\\', "/").to_ascii_lowercase();
    if !clean.starts_with('/') {
        clean.insert(0, '/');
    }
    if clean.len() > 1 && clean.ends_with('/') {
        clean.pop();
    }
    clean
}

pub fn invalidate_path_cache(rel_path: &str) {
    let key = normalize_cache_key(rel_path);
    if let Ok(mut lock) = PATH_CACHE.lock() {
        if let Some(map) = lock.as_mut() {
            map.remove(&key);
            if let Some(parent) = std::path::Path::new(&key).parent() {
                let p_str = normalize_cache_key(&parent.to_string_lossy());
                map.remove(&p_str);
            }
        }
    }
}

fn get_pool() -> Option<Arc<StoragePool>> {
    GLOBAL_POOL.lock().unwrap().as_ref().cloned()
}

fn c_path_to_rel(path_ptr: *const c_char) -> String {
    if path_ptr.is_null() {
        return "/".to_string();
    }
    let c_str = unsafe { CStr::from_ptr(path_ptr) };
    let s = c_str.to_string_lossy();
    let mut clean = s.replace('\\', "/");
    if !clean.starts_with('/') {
        clean.insert(0, '/');
    }
    clean
}

pub unsafe extern "C" fn fs_getattr(path_ptr: *const c_char, stbuf: *mut FuseStat) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    if stbuf.is_null() {
        return -EIO;
    }

    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    if rel_path == "/" || rel_path.is_empty() {
        let mut st = FuseStat::default();
        st.st_ino = path_to_inode("/");
        st.st_mode = 0o040777; // S_IFDIR | 0777
        st.st_nlink = 2;
        *stbuf = st;
        return 0;
    }

    let cache_key = normalize_cache_key(&rel_path);

    // Spindown protection: check memory cache first!
    // Prevents waking up sleeping USB DAS disks for repeated stats and non-existent files.
    {
        if let Ok(lock) = PATH_CACHE.lock() {
            if let Some(map) = lock.as_ref() {
                if let Some(entry) = map.get(&cache_key) {
                    match &entry.node {
                        CachedNode::Negative => {
                            if entry.instant.elapsed().as_secs() < 180 {
                                return -ENOENT;
                            }
                        }
                        CachedNode::Directory => {
                            if entry.instant.elapsed().as_secs() < 60 {
                                let mut st = FuseStat::default();
                                st.st_ino = path_to_inode(&rel_path);
                                st.st_mode = 0o040777;
                                st.st_nlink = 2;
                                *stbuf = st;
                                return 0;
                            }
                        }
                        CachedNode::File { size, modified, .. } => {
                            if entry.instant.elapsed().as_secs() < 60 {
                                let mut st = FuseStat::default();
                                st.st_ino = path_to_inode(&rel_path);
                                st.st_mode = 0o100666;
                                st.st_nlink = 1;
                                st.st_size = *size;
                                st.st_blksize = 4096;
                                st.st_blocks = ((*size + 511) / 512).max(1);
                                st.st_mtim.tv_sec = *modified;
                                st.st_ctim.tv_sec = *modified;
                                st.st_atim.tv_sec = *modified;
                                st.st_birthtim.tv_sec = *modified;
                                *stbuf = st;
                                return 0;
                            }
                        }
                    }
                }
            }
        }
    }

    // Check across member disks in parallel to avoid sequential spin-up delays
    let disks = pool.disks.read().unwrap().clone();

    struct ProbeResult {
        is_dir: bool,
        meta: Option<std::fs::Metadata>,
        disk_id: String,
    }

    let probe_results: Vec<Option<ProbeResult>> = std::thread::scope(|s| {
        let mut handles = Vec::new();
        for disk in disks.iter() {
            let disk_ref = disk;
            let rel_ref = &rel_path;
            handles.push(s.spawn(move || {
                let phys = disk_ref.to_physical_path(rel_ref);
                if phys.is_dir() {
                    Some(ProbeResult {
                        is_dir: true,
                        meta: None,
                        disk_id: disk_ref.id.clone(),
                    })
                } else if phys.is_file() {
                    if let Ok(meta) = std::fs::metadata(&phys) {
                        Some(ProbeResult {
                            is_dir: false,
                            meta: Some(meta),
                            disk_id: disk_ref.id.clone(),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap_or(None)).collect()
    });

    let found_dir = probe_results.iter().flatten().find(|r| r.is_dir);
    let found_file = probe_results.iter().flatten().find(|r| !r.is_dir && r.meta.is_some());

    if let Some(_dir_res) = found_dir {
        if let Ok(mut lock) = PATH_CACHE.lock() {
            if let Some(map) = lock.as_mut() {
                map.insert(
                    cache_key,
                    CacheEntry {
                        node: CachedNode::Directory,
                        instant: std::time::Instant::now(),
                    },
                );
            }
        }
        let mut st = FuseStat::default();
        st.st_ino = path_to_inode(&rel_path);
        st.st_mode = 0o040777;
        st.st_nlink = 2;
        *stbuf = st;
        return 0;
    }

    if let Some(file_res) = found_file {
        if let Some(meta) = &file_res.meta {
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            if let Ok(mut lock) = PATH_CACHE.lock() {
                if let Some(map) = lock.as_mut() {
                    map.insert(
                        cache_key,
                        CacheEntry {
                            node: CachedNode::File {
                                disk_id: file_res.disk_id.clone(),
                                size: meta.len() as i64,
                                modified,
                            },
                            instant: std::time::Instant::now(),
                        },
                    );
                }
            }

            let mut st = FuseStat::default();
            st.st_ino = path_to_inode(&rel_path);
            st.st_mode = 0o100666; // S_IFREG | 0666
            st.st_nlink = 1;
            st.st_size = meta.len() as i64;
            st.st_blksize = 4096;
            st.st_blocks = ((st.st_size + 511) / 512).max(1);
            st.st_mtim.tv_sec = modified;
            st.st_ctim.tv_sec = modified;
            st.st_atim.tv_sec = modified;
            st.st_birthtim.tv_sec = modified;
            *stbuf = st;
            return 0;
        }
    }

    // Negative lookup: cache for 180s so Explorer probes (e.g. desktop.ini) don't scan all disks repeatedly
    if let Ok(mut lock) = PATH_CACHE.lock() {
        if let Some(map) = lock.as_mut() {
            map.insert(
                cache_key,
                CacheEntry {
                    node: CachedNode::Negative,
                    instant: std::time::Instant::now(),
                },
            );
        }
    }

    -ENOENT
}

pub unsafe extern "C" fn fs_readdir(
    path_ptr: *const c_char,
    buf: *mut c_void,
    filler: FuseFillDirFn,
    _off: i64,
    _fi: *mut FuseFileInfo,
) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap().clone();

    // Standard . and ..
    let dot = CString::new(".").unwrap();
    let dotdot = CString::new("..").unwrap();
    let mut dot_st = FuseStat::default();
    dot_st.st_ino = path_to_inode(&format!("{}/.", rel_path));
    dot_st.st_mode = 0o040777;
    let mut dotdot_st = FuseStat::default();
    dotdot_st.st_ino = path_to_inode(&format!("{}/..", rel_path));
    dotdot_st.st_mode = 0o040777;
    filler(buf, dot.as_ptr(), &dot_st, 0);
    filler(buf, dotdot.as_ptr(), &dotdot_st, 0);

    struct DirEntryInfo {
        name_str: String,
        is_dir: bool,
        file_size: i64,
        modified: i64,
        disk_id: String,
    }

    // Scan all member disks in parallel so sleeping disks spin up simultaneously
    let scanned_per_disk: Vec<Vec<DirEntryInfo>> = std::thread::scope(|s| {
        let mut handles = Vec::new();
        for disk in disks.iter() {
            let disk_ref = disk;
            let rel_path_ref = &rel_path;
            handles.push(s.spawn(move || {
                let phys_dir = disk_ref.to_physical_path(rel_path_ref);
                let mut list = Vec::new();
                if phys_dir.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(phys_dir) {
                        for entry in entries.filter_map(|e| e.ok()) {
                            let name_str = entry.file_name().to_string_lossy().to_string();
                            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                            let meta_opt = entry.metadata().ok();
                            let file_size = meta_opt.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                            let modified = meta_opt
                                .as_ref()
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0);

                            list.push(DirEntryInfo {
                                name_str,
                                is_dir,
                                file_size,
                                modified,
                                disk_id: disk_ref.id.clone(),
                            });
                        }
                    }
                }
                list
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });

    let mut entries_to_cache = Vec::new();
    let mut seen_names_lower = HashSet::new();

    for item in scanned_per_disk.into_iter().flatten() {
        let lower = item.name_str.to_ascii_lowercase();
        let child_rel = if rel_path == "/" {
            format!("/{}", item.name_str)
        } else {
            format!("{}/{}", rel_path.trim_end_matches('/'), item.name_str)
        };
        let child_key = normalize_cache_key(&child_rel);

        entries_to_cache.push((
            child_key,
            CacheEntry {
                node: if item.is_dir {
                    CachedNode::Directory
                } else {
                    CachedNode::File {
                        disk_id: item.disk_id,
                        size: item.file_size,
                        modified: item.modified,
                    }
                },
                instant: std::time::Instant::now(),
            },
        ));

        if seen_names_lower.insert(lower) {
            let mut st = FuseStat::default();
            st.st_ino = path_to_inode(&child_rel);
            st.st_mode = if item.is_dir { 0o040777 } else { 0o100666 };
            st.st_size = item.file_size;
            if let Ok(c_name) = CString::new(item.name_str) {
                filler(buf, c_name.as_ptr(), &st, 0);
            }
        }
    }

    // Batch insert into PATH_CACHE with a single lock acquisition
    if let Ok(mut lock) = PATH_CACHE.lock() {
        if let Some(map) = lock.as_mut() {
            for (k, v) in entries_to_cache {
                map.insert(k, v);
            }
        }
    }

    0
}

pub unsafe extern "C" fn fs_open(path_ptr: *const c_char, fi: *mut FuseFileInfo) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    let mut target_file: Option<(PathBuf, bool)> = None;

    // Check if open for write
    let is_write = if !fi.is_null() {
        let flags = (*fi).flags;
        (flags & (libc::O_WRONLY | libc::O_RDWR)) != 0
    } else {
        false
    };

    // Spindown protection: If we know which disk holds this file, open ONLY that disk!
    // The other 3 drives in the USB DAS enclosure will NEVER receive any I/O and remain spun down.
    let cached_disk_id = {
        let key = normalize_cache_key(&rel_path);
        let lock = PATH_CACHE.lock().unwrap();
        lock.as_ref().and_then(|m| m.get(&key)).and_then(|e| {
            if let CachedNode::File { ref disk_id, .. } = e.node {
                Some(disk_id.clone())
            } else {
                None
            }
        })
    };

    if let Some(ref target_id) = cached_disk_id {
        if let Some(disk) = disks.iter().find(|d| &d.id == target_id) {
            let phys = disk.to_physical_path(&rel_path);
            if phys.is_file() {
                target_file = Some((phys, is_write));
            }
        }
    }

    if target_file.is_none() {
        // Fallback: search disks
        for disk in disks.iter() {
            let phys = disk.to_physical_path(&rel_path);
            if phys.is_file() {
                target_file = Some((phys, is_write));
                break;
            }
        }
    }

    match target_file {
        Some((phys_path, is_write)) => {
            let mut opts = OpenOptions::new();
            opts.read(true);
            if is_write {
                opts.write(true);
            }
            match opts.open(&phys_path) {
                Ok(file) => {
                    let fh = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
                    let mut lock = GLOBAL_HANDLES.lock().unwrap();
                    if let Some(map) = lock.as_mut() {
                        map.insert(
                            fh,
                            OpenFileHandle {
                                file,
                                rel_path: rel_path.clone(),
                                is_write,
                            },
                        );
                    }
                    if !fi.is_null() {
                        (*fi).fh = fh;
                    }
                    info!("fs_open succeeded for '{}' (is_write={}) -> fh {}", rel_path, is_write, fh);
                    0
                }
                Err(e) => {
                    error!("fs_open failed for '{}': {}", rel_path, e);
                    -EACCES
                }
            }
        }
        None => -ENOENT,
    }
}

pub unsafe extern "C" fn fs_create(
    path_ptr: *const c_char,
    _mode: u32,
    fi: *mut FuseFileInfo,
) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    let excluded = HashSet::new();

    // Placement policy: pick disk with most free space
    let target_disk = PlacementEngine::select_primary_disk(
        &disks,
        &excluded,
        1024 * 1024,
        PlacementPolicy::MostFreeSpace,
    );

    match target_disk {
        Some(disk) => {
            let phys = disk.to_physical_path(&rel_path);
            if let Some(parent) = phys.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&phys) {
                Ok(file) => {
                    let fh = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
                    let mut lock = GLOBAL_HANDLES.lock().unwrap();
                    if let Some(map) = lock.as_mut() {
                        map.insert(
                            fh,
                            OpenFileHandle {
                                file,
                                rel_path: rel_path.clone(),
                                is_write: true,
                            },
                        );
                    }
                    if !fi.is_null() {
                        (*fi).fh = fh;
                    }
                    invalidate_path_cache(&rel_path);
                    info!("Created file '{}' on primary disk {}", rel_path, disk.id);
                    0
                }
                Err(e) => {
                    error!("fs_create failed for '{}': {}", rel_path, e);
                    -EACCES
                }
            }
        }
        None => -ENOSPC,
    }
}

pub unsafe extern "C" fn fs_read(
    _path_ptr: *const c_char,
    buf: *mut c_char,
    size: size_t,
    off: i64,
    fi: *mut FuseFileInfo,
) -> c_int {
    if fi.is_null() || buf.is_null() {
        return -EIO;
    }
    let fh = (*fi).fh;

    let mut lock = GLOBAL_HANDLES.lock().unwrap();
    let handle = match lock.as_mut().and_then(|m| m.get_mut(&fh)) {
        Some(h) => h,
        None => return -EIO,
    };

    if let Err(_) = handle.file.seek(SeekFrom::Start(off as u64)) {
        return -EIO;
    }

    let slice = std::slice::from_raw_parts_mut(buf as *mut u8, size);
    match handle.file.read(slice) {
        Ok(bytes_read) => bytes_read as c_int,
        Err(e) => {
            error!("fs_read error: {}", e);
            -EIO
        }
    }
}

pub unsafe extern "C" fn fs_write(
    _path_ptr: *const c_char,
    buf: *const c_char,
    size: size_t,
    off: i64,
    fi: *mut FuseFileInfo,
) -> c_int {
    if fi.is_null() || buf.is_null() {
        return -EIO;
    }
    let fh = (*fi).fh;

    let mut lock = GLOBAL_HANDLES.lock().unwrap();
    let handle = match lock.as_mut().and_then(|m| m.get_mut(&fh)) {
        Some(h) => h,
        None => return -EIO,
    };

    if let Err(_) = handle.file.seek(SeekFrom::Start(off as u64)) {
        return -EIO;
    }

    let slice = std::slice::from_raw_parts(buf as *const u8, size);
    match handle.file.write(slice) {
        Ok(bytes_written) => {
            handle.is_write = true;
            bytes_written as c_int
        }
        Err(e) => {
            error!("fs_write error: {}", e);
            -EIO
        }
    }
}

pub unsafe extern "C" fn fs_release(
    _path_ptr: *const c_char,
    fi: *mut FuseFileInfo,
) -> c_int {
    if fi.is_null() {
        return 0;
    }
    let fh = (*fi).fh;

    let closed = {
        let mut lock = GLOBAL_HANDLES.lock().unwrap();
        lock.as_mut().and_then(|m| m.remove(&fh))
    };

    if let Some(handle) = closed {
        // If file was written or created, queue background replication immediately!
        if handle.is_write {
            if let Some(pool) = get_pool() {
                let rep = pool.replicator.clone();
                let rel = handle.rel_path.clone();
                if let Some(rt) = TOKIO_HANDLE.lock().unwrap().as_ref() {
                    rt.spawn(async move {
                        rep.queue_replication(rel).await;
                    });
                }
            }
        }
    }

    0
}

pub unsafe extern "C" fn fs_unlink(path_ptr: *const c_char) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    invalidate_path_cache(&rel_path);
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    let mut deleted_any = false;

    for disk in disks.iter() {
        let phys = disk.to_physical_path(&rel_path);
        if phys.is_file() {
            if let Ok(_) = std::fs::remove_file(&phys) {
                deleted_any = true;
            }
        }
    }

    if deleted_any { 0 } else { -ENOENT }
}

pub unsafe extern "C" fn fs_mkdir(path_ptr: *const c_char, _mode: u32) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    invalidate_path_cache(&rel_path);
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    for disk in disks.iter() {
        let phys = disk.to_physical_path(&rel_path);
        let _ = std::fs::create_dir_all(&phys);
    }

    0
}

pub unsafe extern "C" fn fs_rmdir(path_ptr: *const c_char) -> c_int {
    let rel_path = c_path_to_rel(path_ptr);
    invalidate_path_cache(&rel_path);
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    let mut removed_any = false;

    for disk in disks.iter() {
        let phys = disk.to_physical_path(&rel_path);
        if phys.is_dir() {
            if let Ok(_) = std::fs::remove_dir(&phys) {
                removed_any = true;
            }
        }
    }

    if removed_any { 0 } else { -ENOENT }
}

pub unsafe extern "C" fn fs_rename(
    oldpath_ptr: *const c_char,
    newpath_ptr: *const c_char,
) -> c_int {
    let old_rel = c_path_to_rel(oldpath_ptr);
    let new_rel = c_path_to_rel(newpath_ptr);
    invalidate_path_cache(&old_rel);
    invalidate_path_cache(&new_rel);

    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    let mut renamed_any = false;

    for disk in disks.iter() {
        let old_phys = disk.to_physical_path(&old_rel);
        let new_phys = disk.to_physical_path(&new_rel);

        if old_phys.exists() {
            if let Some(parent) = new_phys.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(_) = std::fs::rename(&old_phys, &new_phys) {
                renamed_any = true;
            }
        }
    }

    if renamed_any { 0 } else { -ENOENT }
}

pub unsafe extern "C" fn fs_statfs(
    _path_ptr: *const c_char,
    stbuf: *mut FuseStatVfs,
) -> c_int {
    if stbuf.is_null() {
        return -EIO;
    }
    let pool = match get_pool() {
        Some(p) => p,
        None => return -EIO,
    };

    let disks = pool.disks.read().unwrap();
    let mut total_bytes: u64 = 0;
    let mut free_bytes: u64 = 0;

    for disk in disks.iter() {
        let stats = disk.query_stats();
        if stats.online && stats.enabled {
            total_bytes = total_bytes.saturating_add(stats.total_bytes);
            free_bytes = free_bytes.saturating_add(stats.free_bytes);
        }
    }

    let block_size: u64 = 4096;
    let total_blocks = total_bytes / block_size;
    let free_blocks = free_bytes / block_size;

    let mut vfs = FuseStatVfs::default();
    vfs.f_bsize = block_size;
    vfs.f_frsize = block_size;
    vfs.f_blocks = total_blocks;
    vfs.f_bfree = free_blocks;
    vfs.f_bavail = free_blocks;
    vfs.f_namemax = 255;

    *stbuf = vfs;
    0
}

pub unsafe extern "C" fn fs_fgetattr(
    _path_ptr: *const c_char,
    stbuf: *mut FuseStat,
    fi: *mut FuseFileInfo,
) -> c_int {
    if stbuf.is_null() || fi.is_null() {
        return -EIO;
    }
    let fh = (*fi).fh;
    let (rel_path, meta_res) = {
        let lock = GLOBAL_HANDLES.lock().unwrap();
        match lock.as_ref().and_then(|m| m.get(&fh)) {
            Some(h) => (h.rel_path.clone(), h.file.metadata()),
            None => return -EIO,
        }
    };

    if let Ok(meta) = meta_res {
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut st = FuseStat::default();
        st.st_ino = path_to_inode(&rel_path);
        st.st_mode = 0o100666;
        st.st_nlink = 1;
        st.st_size = meta.len() as i64;
        st.st_blksize = 4096;
        st.st_blocks = ((st.st_size + 511) / 512).max(1);
        st.st_mtim.tv_sec = modified;
        st.st_ctim.tv_sec = modified;
        st.st_atim.tv_sec = modified;
        st.st_birthtim.tv_sec = modified;
        *stbuf = st;
        0
    } else {
        -EIO
    }
}

pub unsafe extern "C" fn fs_access(path_ptr: *const c_char, _mask: c_int) -> c_int {
    let mut st = FuseStat::default();
    fs_getattr(path_ptr, &mut st)
}

pub fn create_fuse_operations() -> FuseOperations {
    let mut ops = FuseOperations::default();
    ops.getattr = Some(fs_getattr);
    ops.fgetattr = Some(fs_fgetattr);
    ops.access = Some(fs_access);
    ops.readdir = Some(fs_readdir);
    ops.open = Some(fs_open);
    ops.create = Some(fs_create);
    ops.read = Some(fs_read);
    ops.write = Some(fs_write);
    ops.release = Some(fs_release);
    ops.unlink = Some(fs_unlink);
    ops.mkdir = Some(fs_mkdir);
    ops.rmdir = Some(fs_rmdir);
    ops.rename = Some(fs_rename);
    ops.statfs = Some(fs_statfs);
    ops
}
