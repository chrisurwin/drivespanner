use crate::core::disk::MemberDisk;
use crate::core::placement::{PlacementEngine, PlacementPolicy};
use crate::core::pool::StoragePool;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMediaItem {
    pub title: String,
    pub media_type: String, // "episode" | "movie"
    pub grandparent_title: Option<String>, // TV show title
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub rel_path: String,
    pub size_bytes: u64,
    pub source: String, // "Continue Watching" | "Next Episode Pre-cache"
    pub cached_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlexSyncStatus {
    pub connected: bool,
    pub last_sync_time: Option<String>,
    pub error_message: Option<String>,
    pub items: Vec<CachedMediaItem>,
    pub total_cached_bytes: u64,
    pub cache_drive_id: Option<String>,
    pub cache_drive_path: Option<String>,
}

impl Default for PlexSyncStatus {
    fn default() -> Self {
        Self {
            connected: false,
            last_sync_time: None,
            error_message: None,
            items: Vec::new(),
            total_cached_bytes: 0,
            cache_drive_id: None,
            cache_drive_path: None,
        }
    }
}

// Plex JSON API Deserialization Structs
#[derive(Debug, Deserialize)]
struct PlexResponse {
    #[serde(rename = "MediaContainer")]
    media_container: Option<PlexMediaContainer>,
}

#[derive(Debug, Deserialize)]
struct PlexMediaContainer {
    #[serde(rename = "Metadata")]
    metadata: Option<Vec<PlexMetadataItem>>,
}

#[derive(Debug, Deserialize)]
struct PlexMetadataItem {
    #[serde(rename = "ratingKey")]
    #[allow(dead_code)]
    rating_key: Option<String>,
    #[serde(rename = "type")]
    item_type: Option<String>,
    title: Option<String>,
    #[serde(rename = "grandparentTitle")]
    grandparent_title: Option<String>,
    #[serde(rename = "parentIndex")]
    parent_index: Option<u32>,
    index: Option<u32>,
    #[serde(rename = "Media")]
    media: Option<Vec<PlexMediaVariant>>,
}

#[derive(Debug, Deserialize)]
struct PlexMediaVariant {
    #[serde(rename = "Part")]
    parts: Option<Vec<PlexMediaPart>>,
}

#[derive(Debug, Deserialize)]
struct PlexMediaPart {
    file: Option<String>,
    #[allow(dead_code)]
    size: Option<u64>,
}

pub struct PlexCacheService {
    pub status: Arc<RwLock<PlexSyncStatus>>,
}

impl PlexCacheService {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(PlexSyncStatus::default())),
        }
    }

    pub async fn get_status(&self) -> PlexSyncStatus {
        self.status.read().await.clone()
    }

    /// Queries Plex Media Server for "Continue Watching" items, detects next episodes,
    /// and pre-caches them onto the SSD Cache drive.
    pub async fn sync_and_precache(&self, pool: Arc<StoragePool>) -> Result<PlexSyncStatus, String> {
        let (plex_cfg, mount_letter) = {
            let cfg = pool.config.read().await;
            (cfg.cache.plex.clone(), cfg.mount_point.clone())
        };

        let clean_mount = mount_letter.trim_end_matches('\\').to_uppercase();

        // 1. Identify SSD Cache drive in pool
        let cache_disk_opt = {
            let disks = pool.disks.read().unwrap();
            disks.iter().find(|d| d.is_landing_zone).cloned()
        };

        let cache_disk = match cache_disk_opt {
            Some(cd) => cd,
            None => {
                let err = "No member drive is marked as SSD Cache (Landing Zone). Add an SSD cache drive first.".to_string();
                let mut st = self.status.write().await;
                st.connected = false;
                st.error_message = Some(err.clone());
                return Err(err);
            }
        };

        info!("Starting Plex Continue Watching cache sync against {}...", plex_cfg.server_url);

        // 2. Query Plex OnDeck / Continue Watching endpoint
        let endpoint_url = format!("{}/library/onDeck", plex_cfg.server_url.trim_end_matches('/'));
        let mut curl = Command::new("curl.exe");
        curl.arg("-s")
            .arg("--max-time")
            .arg("10")
            .arg("-H")
            .arg("Accept: application/json");

        if !plex_cfg.token.trim().is_empty() {
            curl.arg("-H").arg(format!("X-Plex-Token: {}", plex_cfg.token.trim()));
        }

        curl.arg(&endpoint_url);

        let output = match tokio::task::spawn_blocking(move || curl.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                let err = format!("Failed to execute curl to connect to Plex: {}", e);
                let mut st = self.status.write().await;
                st.connected = false;
                st.error_message = Some(err.clone());
                return Err(err);
            }
            Err(e) => {
                let err = format!("Task join error during Plex query: {}", e);
                let mut st = self.status.write().await;
                st.connected = false;
                st.error_message = Some(err.clone());
                return Err(err);
            }
        };

        if !output.status.success() {
            let err = format!("Plex connection error (exit code {:?}): {}", output.status.code(), String::from_utf8_lossy(&output.stderr));
            let mut st = self.status.write().await;
            st.connected = false;
            st.error_message = Some(err.clone());
            return Err(err);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let plex_data: PlexResponse = match serde_json::from_str(&stdout) {
            Ok(d) => d,
            Err(e) => {
                let snippet = if stdout.len() > 120 { &stdout[..120] } else { &stdout };
                let err = format!("Failed to parse Plex response JSON: {}. Snippet: {}", e, snippet);
                let mut st = self.status.write().await;
                st.connected = false;
                st.error_message = Some(err.clone());
                return Err(err);
            }
        };

        let metadata_items = plex_data
            .media_container
            .and_then(|mc| mc.metadata)
            .unwrap_or_default();

        info!("Retrieved {} OnDeck media items from Plex.", metadata_items.len());

        let mut planned_items: Vec<CachedMediaItem> = Vec::new();
        let disks = pool.disks.read().unwrap().clone();

        for item in metadata_items {
            let item_type = item.item_type.unwrap_or_else(|| "video".to_string());
            let title = item.title.unwrap_or_else(|| "Unknown Title".to_string());
            let grandparent = item.grandparent_title;
            let season = item.parent_index;
            let episode = item.index;

            // Extract file path from first media part
            let part_file = item
                .media
                .and_then(|m| m.into_iter().next())
                .and_then(|v| v.parts)
                .and_then(|p| p.into_iter().next())
                .and_then(|p| p.file);

            if let Some(raw_path) = part_file {
                if let Some(rel_path) = Self::normalize_pool_rel_path(&raw_path, &clean_mount, &disks) {
                    let file_size = Self::get_file_size_on_disks(&rel_path, &disks);
                    let display_title = match &grandparent {
                        Some(gp) => format!("{} - S{:02}E{:02}: {}", gp, season.unwrap_or(1), episode.unwrap_or(1), title),
                        None => title.clone(),
                    };

                    planned_items.push(CachedMediaItem {
                        title: display_title,
                        media_type: item_type.clone(),
                        grandparent_title: grandparent.clone(),
                        season,
                        episode,
                        rel_path: rel_path.clone(),
                        size_bytes: file_size,
                        source: "Continue Watching".to_string(),
                        cached_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
                    });

                    // Precache Next Episode if applicable
                    if plex_cfg.precache_next_episode && item_type == "episode" && season.is_some() && episode.is_some() {
                        let cur_s = season.unwrap();
                        let cur_e = episode.unwrap();
                        if let Some((next_rel, next_size, next_s, next_e)) = Self::find_next_episode_file(&rel_path, cur_s, cur_e, &disks) {
                            let next_title = match &grandparent {
                                Some(gp) => format!("{} - S{:02}E{:02} (Next Episode)", gp, next_s, next_e),
                                None => format!("Next Episode (S{:02}E{:02})", next_s, next_e),
                            };

                            planned_items.push(CachedMediaItem {
                                title: next_title,
                                media_type: "episode".to_string(),
                                grandparent_title: grandparent.clone(),
                                season: Some(next_s),
                                episode: Some(next_e),
                                rel_path: next_rel,
                                size_bytes: next_size,
                                source: "Next Episode Pre-cache".to_string(),
                                cached_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
                            });
                        }
                    }
                }
            }
        }

        // 3. Perform Pre-caching to SSD Cache Drive
        let mut active_cached_items = Vec::new();
        let mut total_cached_bytes: u64 = 0;
        let max_quota_bytes = plex_cfg.max_cache_gb.saturating_mul(1024 * 1024 * 1024);

        for mut item in planned_items {
            let cache_phys = cache_disk.to_physical_path(&item.rel_path);

            if cache_phys.is_file() {
                // Already present on SSD Cache
                if let Ok(meta) = fs::metadata(&cache_phys) {
                    item.size_bytes = meta.len();
                    total_cached_bytes = total_cached_bytes.saturating_add(item.size_bytes);
                }
                active_cached_items.push(item);
                continue;
            }

            // Check if adding this file exceeds max quota
            if max_quota_bytes > 0 && total_cached_bytes.saturating_add(item.size_bytes) > max_quota_bytes {
                warn!("Skipping precache of '{}': cache quota limit ({} GB) reached.", item.title, plex_cfg.max_cache_gb);
                continue;
            }

            // Find source file on one of the HDD member disks
            let source_hdd_opt = disks.iter().find(|d| !d.is_landing_zone && d.to_physical_path(&item.rel_path).is_file());

            if let Some(source_disk) = source_hdd_opt {
                let src_phys = source_disk.to_physical_path(&item.rel_path);
                info!("Pre-caching '{}' from HDD ({}) to SSD Cache...", item.title, source_disk.id);

                if let Some(parent) = cache_phys.parent() {
                    let _ = fs::create_dir_all(parent);
                }

                match fs::copy(&src_phys, &cache_phys) {
                    Ok(copied_bytes) => {
                        item.size_bytes = copied_bytes;
                        total_cached_bytes = total_cached_bytes.saturating_add(copied_bytes);
                        // Invalidate VFS cache so future opens immediately point to the SSD Cache copy!
                        crate::vfs::pool_fs::invalidate_path_cache(&item.rel_path);
                        info!("Successfully precached {} ({} MB) to SSD Cache.", item.title, copied_bytes / (1024 * 1024));
                        active_cached_items.push(item);
                    }
                    Err(e) => {
                        error!("Failed to copy file to SSD cache for '{}': {}", item.title, e);
                    }
                }
            } else {
                warn!("Source file for '{}' not found on any HDD disk: {}", item.title, item.rel_path);
            }
        }

        let now_str = chrono_like_now();
        let final_status = PlexSyncStatus {
            connected: true,
            last_sync_time: Some(now_str),
            error_message: None,
            items: active_cached_items,
            total_cached_bytes,
            cache_drive_id: Some(cache_disk.id.clone()),
            cache_drive_path: Some(cache_disk.drive_path.to_string_lossy().to_string()),
        };

        *self.status.write().await = final_status.clone();
        Ok(final_status)
    }

    /// Translates raw Plex file path (e.g. "V:\TV\Show\ep1.mkv") into pool relative path "/TV/Show/ep1.mkv"
    fn normalize_pool_rel_path(raw: &str, mount_letter: &str, disks: &[MemberDisk]) -> Option<String> {
        let clean_raw = raw.replace('\\', "/");
        let mount_prefix = format!("{}:", mount_letter.trim_end_matches(':'));

        // Case 1: Starts with pool mount letter "V:/TV/..."
        if clean_raw.to_uppercase().starts_with(&mount_prefix) {
            let sub = &clean_raw[mount_prefix.len()..];
            let formatted = if !sub.starts_with('/') { format!("/{}", sub) } else { sub.to_string() };
            return Some(formatted);
        }

        // Case 2: Direct path to a member drive "D:/PoolData.xyz/TV/..."
        for disk in disks {
            let pooldata_str = disk.pooldata_path.to_string_lossy().replace('\\', "/");
            if clean_raw.to_uppercase().starts_with(&pooldata_str.to_uppercase()) {
                let sub = &clean_raw[pooldata_str.len()..];
                let formatted = if !sub.starts_with('/') { format!("/{}", sub) } else { sub.to_string() };
                return Some(formatted);
            }
        }

        // Case 3: Already a relative path "/TV/..."
        if clean_raw.starts_with('/') {
            return Some(clean_raw);
        }

        None
    }

    fn get_file_size_on_disks(rel_path: &str, disks: &[MemberDisk]) -> u64 {
        for disk in disks {
            let p = disk.to_physical_path(rel_path);
            if let Ok(meta) = fs::metadata(p) {
                return meta.len();
            }
        }
        0
    }

    /// Looks for the next episode file (e.g. S01E04 after S01E03) in the same directory across member disks
    fn find_next_episode_file(cur_rel: &str, cur_s: u32, cur_e: u32, disks: &[MemberDisk]) -> Option<(String, u64, u32, u32)> {
        let cur_path = Path::new(cur_rel);
        let parent_dir = cur_path.parent()?.to_string_lossy().replace('\\', "/");

        let next_e = cur_e + 1;
        let patterns = vec![
            format!("s{:02}e{:02}", cur_s, next_e),
            format!("s{}e{}", cur_s, next_e),
            format!("{}x{:02}", cur_s, next_e),
            format!("{}x{}", cur_s, next_e),
        ];

        // Scan parent directory across disks
        for disk in disks {
            let phys_dir = disk.to_physical_path(&parent_dir);
            if phys_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(phys_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                        for pat in &patterns {
                            if name.contains(pat) {
                                let filename = entry.file_name().to_string_lossy().to_string();
                                let rel = if parent_dir == "/" || parent_dir.is_empty() {
                                    format!("/{}", filename)
                                } else {
                                    format!("{}/{}", parent_dir.trim_end_matches('/'), filename)
                                };
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                return Some((rel, size, cur_s, next_e));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Flushes newly written files off the SSD Cache to HDD storage array, while protecting Plex items!
    pub async fn run_cache_mover(pool: Arc<StoragePool>) -> Result<usize, String> {
        let min_age_mins = pool.config.read().await.cache.min_file_age_minutes;
        let protected_set: HashSet<String> = {
            let st = pool.plex.status.read().await;
            st.items.iter().map(|i| i.rel_path.clone()).collect()
        };

        let cache_disk_opt = {
            let disks = pool.disks.read().unwrap();
            disks.iter().find(|d| d.is_landing_zone).cloned()
        };

        let cache_disk = match cache_disk_opt {
            Some(cd) => cd,
            None => return Ok(0),
        };

        if !cache_disk.pooldata_path.exists() {
            return Ok(0);
        }

        info!("Running SSD Cache Mover: evaluating files to flush to persistent array HDDs...");
        let now_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let min_age_secs = min_age_mins * 60;

        let mut files_to_move = Vec::new();
        for entry in walkdir::WalkDir::new(&cache_disk.pooldata_path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                if let Ok(rel) = entry.path().strip_prefix(&cache_disk.pooldata_path) {
                    let rel_str = format!("/{}", rel.to_string_lossy().replace('\\', "/"));

                    // Protect active Plex continue watching files from being evicted from SSD
                    if protected_set.contains(&rel_str) {
                        continue;
                    }

                    if let Ok(meta) = entry.metadata() {
                        let modified = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);

                        if now_epoch.saturating_sub(modified) >= min_age_secs {
                            files_to_move.push((rel_str, meta.len(), entry.path().to_path_buf()));
                        }
                    }
                }
            }
        }

        info!("Identified {} files on SSD Cache eligible to move to array HDDs.", files_to_move.len());
        let disks = pool.disks.read().unwrap().clone();
        let array_disks: Vec<MemberDisk> = disks.iter().filter(|d| !d.is_landing_zone).cloned().collect();

        if array_disks.is_empty() {
            return Err("No persistent array HDD disks available to receive cached files.".to_string());
        }

        let mut moved_count = 0;
        let excluded = HashSet::new();

        for (rel_path, file_size, cache_phys) in files_to_move {
            if let Some(dest_disk) = PlacementEngine::select_primary_disk(&array_disks, &excluded, file_size, PlacementPolicy::MostFreeSpace) {
                let dest_phys = dest_disk.to_physical_path(&rel_path);

                if let Some(parent) = dest_phys.parent() {
                    let _ = fs::create_dir_all(parent);
                }

                match fs::copy(&cache_phys, &dest_phys) {
                    Ok(_) => {
                        // Delete file from SSD Cache once securely written to array HDD
                        let _ = fs::remove_file(&cache_phys);
                        crate::vfs::pool_fs::invalidate_path_cache(&rel_path);
                        info!("Mover: flushed cached file '{}' from SSD to HDD ({})", rel_path, dest_disk.id);
                        moved_count += 1;
                    }
                    Err(e) => {
                        error!("Mover error copying '{}' to HDD: {}", rel_path, e);
                    }
                }
            }
        }

        Ok(moved_count)
    }

    /// Spawns periodic background sync loop
    pub fn start_background_loop(self: Arc<Self>, pool: Arc<StoragePool>) {
        tokio::spawn(async move {
            loop {
                let (enabled, interval_mins) = {
                    let cfg = pool.config.read().await;
                    (cfg.cache.plex.enabled, cfg.cache.plex.sync_interval_minutes)
                };

                let interval_secs = (interval_mins * 60).max(60);
                tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;

                if enabled {
                    let pool_clone = pool.clone();
                    let this = self.clone();
                    if let Err(e) = this.sync_and_precache(pool_clone).await {
                        warn!("Background Plex cache sync notice: {}", e);
                    }
                }
            }
        });
    }
}

fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    format!("{}-sync", now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::path::PathBuf;

    #[test]
    fn test_normalize_pool_rel_path_mount_letter() {
        let disks = vec![];
        let normalized = PlexCacheService::normalize_pool_rel_path("V:\\Movies\\Inception (2010)\\Inception.mkv", "V:", &disks);
        assert_eq!(normalized, Some("/Movies/Inception (2010)/Inception.mkv".to_string()));

        let normalized_slash = PlexCacheService::normalize_pool_rel_path("V:/TV/Lost/Season 1/S01E01.mkv", "V:", &disks);
        assert_eq!(normalized_slash, Some("/TV/Lost/Season 1/S01E01.mkv".to_string()));
    }

    #[test]
    fn test_normalize_pool_rel_path_pooldata_prefix() {
        let temp_dir = std::env::temp_dir().join("drivespanner_test_norm");
        let _ = fs::create_dir_all(&temp_dir);
        let disk = MemberDisk {
            id: "disk-1".to_string(),
            drive_path: PathBuf::from("D:\\"),
            pooldata_path: temp_dir.clone(),
            enabled: true,
            read_only: false,
            is_landing_zone: false,
            cached_stats: std::sync::Arc::new(std::sync::Mutex::new(None)),
            approx_file_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        let target_file = temp_dir.join("Anime").join("CowboyBebop.mkv");
        let target_str = target_file.to_string_lossy().to_string();

        let normalized = PlexCacheService::normalize_pool_rel_path(&target_str, "V:", &[disk]);
        assert_eq!(normalized, Some("/Anime/CowboyBebop.mkv".to_string()));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_find_next_episode_file_detection() {
        let temp_dir = std::env::temp_dir().join("drivespanner_test_episodes");
        let series_dir = temp_dir.join("TV").join("Severance").join("Season 01");
        fs::create_dir_all(&series_dir).unwrap();

        // Create episode 4 and episode 5
        let ep4 = series_dir.join("Severance.S01E04.The.You.You.Are.mkv");
        let ep5 = series_dir.join("Severance.S01E05.The.Grim.Barbarity.mkv");
        File::create(&ep4).unwrap();
        File::create(&ep5).unwrap();

        let disk = MemberDisk {
            id: "disk-tv".to_string(),
            drive_path: PathBuf::from("D:\\"),
            pooldata_path: temp_dir.clone(),
            enabled: true,
            read_only: false,
            is_landing_zone: false,
            cached_stats: std::sync::Arc::new(std::sync::Mutex::new(None)),
            approx_file_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };

        let cur_rel = "/TV/Severance/Season 01/Severance.S01E04.The.You.You.Are.mkv";
        let next_found = PlexCacheService::find_next_episode_file(cur_rel, 1, 4, &[disk]);

        assert!(next_found.is_some());
        let (next_rel, _size, s, e) = next_found.unwrap();
        assert_eq!(s, 1);
        assert_eq!(e, 5);
        assert!(next_rel.contains("S01E05"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
