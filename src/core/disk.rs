use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};

const DRIVE_UNKNOWN: u32 = 0;
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_CDROM: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskStats {
    pub id: String,
    pub drive_path: String,       // e.g. "D:\\"
    #[serde(alias = "poolpart_path")]
    pub pooldata_path: String,    // e.g. "D:\\PoolData.4e019924-..."
    pub volume_name: String,      // e.g. "DATA"
    pub filesystem: String,       // e.g. "NTFS"
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
    pub enabled: bool,
    pub read_only: bool,
    pub is_landing_zone: bool,
    pub file_count: u64,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableDriveInfo {
    pub drive_letter: String, // e.g. "D:"
    pub volume_name: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
    pub is_already_member: bool,
    pub is_system_drive: bool,
}

#[derive(Debug, Clone)]
pub struct MemberDisk {
    pub id: String,
    pub drive_path: PathBuf,
    pub pooldata_path: PathBuf,
    pub enabled: bool,
    pub read_only: bool,
    pub is_landing_zone: bool,
    pub cached_stats: Arc<Mutex<Option<(Instant, DiskStats)>>>,
    pub approx_file_count: Arc<AtomicU64>,
}

impl MemberDisk {
    pub fn new(
        id: String,
        drive_path_str: &str,
        pooldata_name: &str,
        enabled: bool,
        read_only: bool,
        is_landing_zone: bool,
    ) -> std::io::Result<Self> {
        let mut clean_drive = drive_path_str.trim().to_string();
        if !clean_drive.ends_with('\\') && !clean_drive.ends_with('/') {
            clean_drive.push('\\');
        }
        let drive_path = PathBuf::from(&clean_drive);
        let pooldata_path = drive_path.join(pooldata_name);

        if !pooldata_path.exists() {
            fs::create_dir_all(&pooldata_path)?;
        }

        Ok(Self {
            id,
            drive_path,
            pooldata_path,
            enabled,
            read_only,
            is_landing_zone,
            cached_stats: Arc::new(Mutex::new(None)),
            approx_file_count: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn to_physical_path(&self, relative_pool_path: &str) -> PathBuf {
        let clean = relative_pool_path
            .trim_start_matches('/')
            .trim_start_matches('\\')
            .replace('/', "\\");
        self.pooldata_path.join(clean)
    }

    pub fn query_stats(&self) -> DiskStats {
        // Spindown protection: If stats were queried recently (< 60s),
        // return cached snapshot without sending I/O to sleeping USB DAS disks!
        {
            let cache = self.cached_stats.lock().unwrap();
            if let Some((instant, ref stats)) = *cache {
                if instant.elapsed().as_secs() < 60 {
                    return stats.clone();
                }
            }
        }

        let (total_bytes, free_bytes, volume_name, filesystem, online) =
            query_windows_volume_info(&self.drive_path);
        let used_bytes = total_bytes.saturating_sub(free_bytes);
        let used_percent = if total_bytes > 0 {
            (used_bytes as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        let file_count = self.approx_file_count.load(Ordering::Relaxed);

        let stats = DiskStats {
            id: self.id.clone(),
            drive_path: self.drive_path.to_string_lossy().to_string(),
            pooldata_path: self.pooldata_path.to_string_lossy().to_string(),
            volume_name,
            filesystem,
            total_bytes,
            free_bytes,
            used_bytes,
            used_percent,
            enabled: self.enabled,
            read_only: self.read_only,
            is_landing_zone: self.is_landing_zone,
            file_count,
            online,
        };

        *self.cached_stats.lock().unwrap() = Some((Instant::now(), stats.clone()));
        stats
    }
}

pub fn to_wide_null(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

pub fn query_windows_volume_info(path: &Path) -> (u64, u64, String, String, bool) {
    let path_str = path.to_string_lossy();
    let wide_path = to_wide_null(&path_str);

    let mut free_bytes_available: u64 = 0;
    let mut total_number_of_bytes: u64 = 0;
    let mut total_number_of_free_bytes: u64 = 0;

    let res = unsafe {
        GetDiskFreeSpaceExW(
            wide_path.as_ptr(),
            &mut free_bytes_available,
            &mut total_number_of_bytes,
            &mut total_number_of_free_bytes,
        )
    };

    if res == 0 {
        return (0, 0, "Unknown".to_string(), "Unknown".to_string(), false);
    }

    let mut volume_name_buffer = [0u16; 260];
    let mut fs_name_buffer = [0u16; 260];
    let mut serial_number: u32 = 0;
    let mut max_component_len: u32 = 0;
    let mut flags: u32 = 0;

    let vol_res = unsafe {
        GetVolumeInformationW(
            wide_path.as_ptr(),
            volume_name_buffer.as_mut_ptr(),
            volume_name_buffer.len() as u32,
            &mut serial_number,
            &mut max_component_len,
            &mut flags,
            fs_name_buffer.as_mut_ptr(),
            fs_name_buffer.len() as u32,
        )
    };

    let volume_name = if vol_res != 0 {
        let len = volume_name_buffer
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(volume_name_buffer.len());
        String::from_utf16_lossy(&volume_name_buffer[..len])
    } else {
        "Local Disk".to_string()
    };

    let filesystem = if vol_res != 0 {
        let len = fs_name_buffer
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(fs_name_buffer.len());
        String::from_utf16_lossy(&fs_name_buffer[..len])
    } else {
        "NTFS".to_string()
    };

    (
        total_number_of_bytes,
        total_number_of_free_bytes,
        if volume_name.is_empty() { "Local Disk".to_string() } else { volume_name },
        filesystem,
        true,
    )
}

#[derive(Clone, Debug)]
struct RawVolumeData {
    letter_char: char,
    drive_root: String,
    volume_name: String,
    filesystem: String,
    total_bytes: u64,
    free_bytes: u64,
    used_bytes: u64,
    used_percent: f64,
    #[allow(dead_code)]
    online: bool,
}

static RAW_SYSTEM_VOLUMES_CACHE: Mutex<Option<(Instant, Vec<RawVolumeData>)>> =
    Mutex::new(None);

pub fn invalidate_system_drives_cache() {
    let mut cache = RAW_SYSTEM_VOLUMES_CACHE.lock().unwrap();
    *cache = None;
}

fn scan_raw_system_volumes() -> Vec<RawVolumeData> {
    let mut volumes = Vec::new();
    let drive_mask = unsafe { GetLogicalDrives() };

    for i in 0..26 {
        if (drive_mask & (1 << i)) == 0 {
            continue;
        }

        let letter_char = (b'A' + i as u8) as char;
        let drive_root = format!("{}:\\", letter_char);
        let wide_path = to_wide_null(&drive_root);

        let drive_type = unsafe { GetDriveTypeW(wide_path.as_ptr()) };
        // Skip optical/CDROM drives, unknown drives, and legacy floppy drives
        if drive_type == DRIVE_CDROM || drive_type == DRIVE_UNKNOWN || drive_type == DRIVE_NO_ROOT_DIR {
            continue;
        }
        if (letter_char == 'A' || letter_char == 'B') && drive_type == DRIVE_REMOVABLE {
            continue;
        }

        let path = Path::new(&drive_root);
        let (total_bytes, free_bytes, volume_name, filesystem, online) =
            query_windows_volume_info(path);

        // Skip virtual pool filesystem to prevent recursion/looping
        if filesystem.contains("PoolForge") || volume_name.contains("PoolForge") {
            continue;
        }

        if online && total_bytes > 0 {
            let used_bytes = total_bytes.saturating_sub(free_bytes);
            let used_percent = if total_bytes > 0 {
                (used_bytes as f64 / total_bytes as f64) * 100.0
            } else {
                0.0
            };

            volumes.push(RawVolumeData {
                letter_char,
                drive_root,
                volume_name,
                filesystem,
                total_bytes,
                free_bytes,
                used_bytes,
                used_percent,
                online,
            });
        }
    }

    volumes
}

/// Discovers candidate Windows drives that can be added to the storage pool
pub fn scan_system_drives(existing_member_paths: &[String]) -> Vec<AvailableDriveInfo> {
    let raw_volumes = {
        let mut cache = RAW_SYSTEM_VOLUMES_CACHE.lock().unwrap();
        if let Some((instant, ref list)) = *cache {
            if instant.elapsed().as_secs() < 15 {
                list.clone()
            } else {
                let fresh = scan_raw_system_volumes();
                *cache = Some((Instant::now(), fresh.clone()));
                fresh
            }
        } else {
            let fresh = scan_raw_system_volumes();
            *cache = Some((Instant::now(), fresh.clone()));
            fresh
        }
    };

    let existing_letters: Vec<char> = existing_member_paths
        .iter()
        .filter_map(|p| p.trim().chars().next().map(|c| c.to_ascii_uppercase()))
        .collect();

    raw_volumes
        .into_iter()
        .map(|v| {
            let is_already_member = existing_letters.contains(&v.letter_char)
                || existing_member_paths.iter().any(|m| {
                    m.eq_ignore_ascii_case(&v.drive_root)
                        || m.eq_ignore_ascii_case(&format!("{}:", v.letter_char))
                });

            let is_system_drive = v.letter_char.to_ascii_uppercase() == 'C';

            AvailableDriveInfo {
                drive_letter: format!("{}:", v.letter_char),
                volume_name: v.volume_name,
                filesystem: v.filesystem,
                total_bytes: v.total_bytes,
                free_bytes: v.free_bytes,
                used_bytes: v.used_bytes,
                used_percent: v.used_percent,
                is_already_member,
                is_system_drive,
            }
        })
        .collect()
}
