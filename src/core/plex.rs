use crate::core::disk::MemberDisk;
use crate::core::placement::{PlacementEngine, PlacementPolicy};
use crate::core::pool::StoragePool;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
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
    #[serde(default)]
    pub evicted_items_count: u64,
    #[serde(default)]
    pub last_evicted_summary: Option<String>,
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
            evicted_items_count: 0,
            last_evicted_summary: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedFolderOverview {
    pub id: String,
    pub path_pattern: String,
    pub replica_count: usize,
    pub enabled: bool,
    pub description: String,
    pub cached_files: u64,
    pub cached_bytes: u64,
    pub total_files: u64,
    pub total_bytes: u64,
    pub is_fully_cached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedSyncStats {
    pub files_copied: u64,
    pub bytes_copied: u64,
    pub files_already_cached: u64,
    pub total_pinned_folders: usize,
    pub message: String,
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

/// Returns the path to the persistent Plex cache state file in ProgramData
pub fn get_plex_cache_state_path() -> PathBuf {
    if let Ok(progdata) = std::env::var("ProgramData") {
        let dir = PathBuf::from(progdata).join("DriveSpanner");
        let _ = fs::create_dir_all(&dir);
        return dir.join("plex_cache.json");
    }
    PathBuf::from("plex_cache.json")
}

pub struct PlexCacheService {
    pub status: Arc<RwLock<PlexSyncStatus>>,
}

impl PlexCacheService {
    pub fn new() -> Self {
        let initial_status = Self::load_persisted_status().unwrap_or_default();
        Self {
            status: Arc::new(RwLock::new(initial_status)),
        }
    }

    pub fn load_persisted_status() -> Option<PlexSyncStatus> {
        let path = get_plex_cache_state_path();
        if path.is_file() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if let Ok(st) = serde_json::from_str::<PlexSyncStatus>(&contents) {
                    return Some(st);
                }
            }
        }
        None
    }

    pub fn save_persisted_status(status: &PlexSyncStatus) {
        let path = get_plex_cache_state_path();
        if let Ok(json) = serde_json::to_string_pretty(status) {
            let _ = fs::write(path, json);
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

        // 3. Evict Watched Episodes from SSD Cache back to HDD (if enabled)
        let (mut evicted_count, mut last_evicted_summary, previous_items) = {
            let st = self.status.read().await;
            (st.evicted_items_count, st.last_evicted_summary.clone(), st.items.clone())
        };

        let planned_rel_set: HashSet<String> = planned_items.iter().map(|i| i.rel_path.clone()).collect();
        let array_disks: Vec<MemberDisk> = disks.iter().filter(|d| !d.is_landing_zone).cloned().collect();

        // Items that were in SSD cache previously but are no longer in planned items (watched / finished)
        let watched_candidates: Vec<CachedMediaItem> = previous_items
            .into_iter()
            .filter(|item| !planned_rel_set.contains(&item.rel_path))
            .collect();

        let mut retry_eviction_items = Vec::new();

        if plex_cfg.evict_watched_episodes && !watched_candidates.is_empty() {
            info!("Evaluating {} previously cached items for watched episode eviction...", watched_candidates.len());

            for watched in watched_candidates {
                let cache_phys = cache_disk.to_physical_path(&watched.rel_path);

                if !cache_phys.is_file() {
                    // File is already absent from SSD cache
                    continue;
                }

                info!("Evicting watched media '{}' from SSD Cache and verifying HDD storage...", watched.title);

                // Step A: Ensure file is safely on HDD array
                let file_size = match fs::metadata(&cache_phys) {
                    Ok(m) => m.len(),
                    Err(_) => watched.size_bytes,
                };

                let existing_hdd = disks.iter().find(|d| !d.is_landing_zone && d.to_physical_path(&watched.rel_path).is_file());

                let mut hdd_ready = false;
                if let Some(hdd_disk) = existing_hdd {
                    let hdd_phys = hdd_disk.to_physical_path(&watched.rel_path);
                    if let Ok(m) = fs::metadata(&hdd_phys) {
                        if m.len() == file_size {
                            hdd_ready = true;
                        } else {
                            warn!("HDD copy size mismatch for '{}' (HDD: {} vs SSD: {}). Re-copying to HDD...", watched.title, m.len(), file_size);
                            if fs::copy(&cache_phys, &hdd_phys).is_ok() {
                                hdd_ready = true;
                            }
                        }
                    }
                } else if !array_disks.is_empty() {
                    // File is only on SSD landing zone; migrate it to array HDD
                    let excluded = HashSet::new();
                    if let Some(dest_disk) = PlacementEngine::select_primary_disk(&array_disks, &excluded, file_size, PlacementPolicy::MostFreeSpace) {
                        let dest_phys = dest_disk.to_physical_path(&watched.rel_path);
                        if let Some(parent) = dest_phys.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        match fs::copy(&cache_phys, &dest_phys) {
                            Ok(_) => {
                                info!("Safely transferred watched file '{}' to HDD disk ({})", watched.title, dest_disk.id);
                                hdd_ready = true;
                            }
                            Err(e) => {
                                error!("Failed to transfer watched file '{}' to HDD: {}", watched.title, e);
                            }
                        }
                    } else {
                        error!("No suitable HDD disk found with enough space for watched file '{}'", watched.title);
                    }
                }

                if !hdd_ready {
                    error!("Aborting eviction of '{}': could not guarantee file persistence on HDD array.", watched.title);
                    retry_eviction_items.push(watched);
                    continue;
                }

                // Step B: Remove file from SSD cache
                match fs::remove_file(&cache_phys) {
                    Ok(_) => {
                        crate::vfs::pool_fs::invalidate_path_cache(&watched.rel_path);
                        Self::cleanup_empty_parent_dirs(&cache_phys, &cache_disk.pooldata_path);
                        evicted_count = evicted_count.saturating_add(1);
                        last_evicted_summary = Some(format!("{} returned to HDD", watched.title));
                        info!("Successfully evicted watched episode '{}' from SSD cache (returned to HDD).", watched.title);
                    }
                    Err(e) => {
                        warn!("Could not delete '{}' from SSD cache (file in use?): {}. Will retry next sync.", watched.title, e);
                        retry_eviction_items.push(watched);
                    }
                }
            }
        }

        // 4. Perform Pre-caching to SSD Cache Drive
        let mut active_cached_items = retry_eviction_items;
        let mut total_cached_bytes: u64 = active_cached_items.iter().map(|i| i.size_bytes).sum();
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
            evicted_items_count: evicted_count,
            last_evicted_summary,
        };

        Self::save_persisted_status(&final_status);
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
        let (protected_set, pinned_rules, min_age_mins): (HashSet<String>, Vec<crate::config::FolderReplicationRule>, u64) = {
            let st = pool.plex.status.read().await;
            let p_set: HashSet<String> = st.items.iter().map(|i| i.rel_path.clone()).collect();
            let cfg = pool.config.read().await;
            let p_rules = cfg.replication.rules.iter().filter(|r| r.enabled && r.pin_to_cache).cloned().collect();
            let age = cfg.cache.min_file_age_minutes;
            (p_set, p_rules, age)
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

                    // Protect pinned fast cache folders from eviction, while ensuring HDD backup copy exists
                    let norm_rel = crate::core::replication::ReplicationRulesEngine::normalize_path(&rel_str);
                    let matching_pinned = pinned_rules.iter().find(|r| {
                        let pat = crate::core::replication::ReplicationRulesEngine::normalize_path(&r.path_pattern);
                        crate::core::replication::ReplicationRulesEngine::matches_pattern(&norm_rel, &pat)
                    });

                    if let Some(rule) = matching_pinned {
                        // Ensure at least one persistent copy exists on the HDD array for redundancy!
                        let (has_hdd_copy, array_disks) = {
                            let disks_guard = pool.disks.read().unwrap();
                            let has = disks_guard.iter().any(|d| !d.is_landing_zone && d.to_physical_path(&rel_str).is_file());
                            let arr: Vec<MemberDisk> = disks_guard.iter().filter(|d| !d.is_landing_zone).cloned().collect();
                            (has, arr)
                        };

                        if !has_hdd_copy && !array_disks.is_empty() {
                            let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                            let excluded = HashSet::new();

                            if let Some(dest_disk) = PlacementEngine::select_primary_disk(&array_disks, &excluded, file_size, PlacementPolicy::MostFreeSpace) {
                                let dest_phys = dest_disk.to_physical_path(&rel_str);
                                if let Some(parent) = dest_phys.parent() {
                                    let _ = fs::create_dir_all(parent);
                                }
                                if let Ok(_) = fs::copy(entry.path(), &dest_phys) {
                                    info!("Mover: replicated pinned fast cache file '{}' to HDD disk ({})", rel_str, dest_disk.id);
                                    if rule.replica_count > 1 {
                                        let rep = pool.replicator.clone();
                                        let rel_clone = rel_str.clone();
                                        tokio::spawn(async move {
                                            rep.queue_replication(rel_clone).await;
                                        });
                                    }
                                }
                            }
                        }
                        // PRESERVE ON SSD: Do not add to files_to_move, so file stays on SSD Cache
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

    /// Recursively removes empty parent directories up to, but not including, the base cache directory
    pub fn cleanup_empty_parent_dirs(file_path: &Path, base_dir: &Path) {
        let base_str = base_dir.to_string_lossy().trim_end_matches('\\').trim_end_matches('/').to_uppercase();
        let mut cur = match file_path.parent() {
            Some(p) => p.to_path_buf(),
            None => return,
        };

        loop {
            let cur_str = cur.to_string_lossy().trim_end_matches('\\').trim_end_matches('/').to_uppercase();
            if cur_str == base_str || !cur_str.starts_with(&base_str) {
                break;
            }
            if fs::remove_dir(&cur).is_err() {
                break;
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
    }

    /// Scans all folders marked with `pin_to_cache: true` across member HDDs,
    /// copies missing files onto the SSD Cache drive for zero-spinup instant access,
    /// and ensures the files are replicated according to their replica_count.
    pub async fn sync_pinned_fast_cache(pool: Arc<StoragePool>) -> Result<PinnedSyncStats, String> {
        let cache_disk_opt = {
            let disks = pool.disks.read().unwrap();
            disks.iter().find(|d| d.is_landing_zone).cloned()
        };

        let cache_disk = match cache_disk_opt {
            Some(cd) => cd,
            None => {
                return Err("No member drive is designated as SSD Cache (Landing Zone).".to_string());
            }
        };

        let pinned_rules: Vec<crate::config::FolderReplicationRule> = {
            let cfg = pool.config.read().await;
            cfg.replication.rules.iter().filter(|r| r.enabled && r.pin_to_cache).cloned().collect()
        };

        if pinned_rules.is_empty() {
            return Ok(PinnedSyncStats {
                files_copied: 0,
                bytes_copied: 0,
                files_already_cached: 0,
                total_pinned_folders: 0,
                message: "No folders are currently pinned to Fast SSD Cache.".to_string(),
            });
        }

        info!("Starting Fast Cache synchronization for {} pinned folder(s)...", pinned_rules.len());

        let disks = pool.disks.read().unwrap().clone();
        let hdd_disks: Vec<MemberDisk> = disks.iter().filter(|d| !d.is_landing_zone).cloned().collect();

        let mut files_copied = 0;
        let mut bytes_copied = 0;
        let mut files_already_cached = 0;

        for rule in &pinned_rules {
            let norm_pattern = crate::core::replication::ReplicationRulesEngine::normalize_path(&rule.path_pattern);
            for hdd in &hdd_disks {
                let hdd_folder_phys = hdd.to_physical_path(&norm_pattern);
                if !hdd_folder_phys.exists() {
                    continue;
                }

                for entry in walkdir::WalkDir::new(&hdd_folder_phys).into_iter().filter_map(|e| e.ok()) {
                    if entry.file_type().is_file() {
                        if let Ok(rel) = entry.path().strip_prefix(&hdd.pooldata_path) {
                            let rel_str = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                            let norm_rel = crate::core::replication::ReplicationRulesEngine::normalize_path(&rel_str);

                            if !crate::core::replication::ReplicationRulesEngine::matches_pattern(&norm_rel, &norm_pattern) {
                                continue;
                            }

                            let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                            let cache_phys = cache_disk.to_physical_path(&rel_str);

                            // Check if file already exists on SSD Cache
                            let needs_cache_copy = if cache_phys.is_file() {
                                match fs::metadata(&cache_phys) {
                                    Ok(m) => m.len() != file_size, // Re-copy if size differs
                                    Err(_) => true,
                                }
                            } else {
                                true
                            };

                            if needs_cache_copy {
                                // Check available SSD Cache space (preserve 2 GB safety margin)
                                let stats = cache_disk.query_stats();
                                if stats.free_bytes < file_size.saturating_add(2 * 1024 * 1024 * 1024) {
                                    warn!("Pausing Fast Cache sync for '{}': SSD Cache disk free space limit reached.", rel_str);
                                    break;
                                }

                                if let Some(parent) = cache_phys.parent() {
                                    let _ = fs::create_dir_all(parent);
                                }

                                match fs::copy(entry.path(), &cache_phys) {
                                    Ok(copied) => {
                                        files_copied += 1;
                                        bytes_copied += copied;
                                        crate::vfs::pool_fs::invalidate_path_cache(&rel_str);
                                        info!("Fast Cache: cached '{}' ({:.2} MB) on SSD Cache for instant access.", rel_str, copied as f64 / (1024.0 * 1024.0));
                                    }
                                    Err(e) => {
                                        error!("Fast Cache: failed to copy '{}' to SSD: {}", rel_str, e);
                                    }
                                }
                            } else {
                                files_already_cached += 1;
                            }

                            // Trigger replication on the HDD array if duplication is configured
                            if rule.replica_count > 1 {
                                let rep = pool.replicator.clone();
                                let rel_clone = rel_str.clone();
                                tokio::spawn(async move {
                                    rep.queue_replication(rel_clone).await;
                                });
                            }
                        }
                    }
                }
            }
        }

        let summary = format!(
            "Fast Cache sync complete: {} file(s) ({:.2} MB) populated to SSD Cache, {} file(s) already cached across {} pinned folder(s).",
            files_copied,
            bytes_copied as f64 / (1024.0 * 1024.0),
            files_already_cached,
            pinned_rules.len()
        );
        info!("{}", summary);

        Ok(PinnedSyncStats {
            files_copied,
            bytes_copied,
            files_already_cached,
            total_pinned_folders: pinned_rules.len(),
            message: summary,
        })
    }

    /// Computes aggregated statistics for all pinned fast cache folders across the pool
    pub async fn get_pinned_fast_cache_overview(pool: &StoragePool) -> Vec<PinnedFolderOverview> {
        let (pinned_rules, disks) = {
            let cfg = pool.config.read().await;
            let rules: Vec<_> = cfg.replication.rules.iter().filter(|r| r.enabled && r.pin_to_cache).cloned().collect();
            let d = pool.disks.read().unwrap().clone();
            (rules, d)
        };

        let cache_disk_opt = disks.iter().find(|d| d.is_landing_zone).cloned();
        let mut overviews = Vec::new();

        for rule in pinned_rules {
            let norm_pattern = crate::core::replication::ReplicationRulesEngine::normalize_path(&rule.path_pattern);

            let mut total_files_set = HashSet::new();
            let mut total_bytes = 0u64;
            let mut cached_files = 0u64;
            let mut cached_bytes = 0u64;

            // 1. Scan SSD cache disk for files in this folder
            if let Some(ref cd) = cache_disk_opt {
                let cache_folder_phys = cd.to_physical_path(&norm_pattern);
                if cache_folder_phys.exists() {
                    for entry in walkdir::WalkDir::new(&cache_folder_phys).into_iter().filter_map(|e| e.ok()) {
                        if entry.file_type().is_file() {
                            if let Ok(rel) = entry.path().strip_prefix(&cd.pooldata_path) {
                                let rel_str = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                                let norm_rel = crate::core::replication::ReplicationRulesEngine::normalize_path(&rel_str);
                                if crate::core::replication::ReplicationRulesEngine::matches_pattern(&norm_rel, &norm_pattern) {
                                    let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                    cached_files += 1;
                                    cached_bytes += sz;
                                    if total_files_set.insert(norm_rel) {
                                        total_bytes += sz;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 2. Scan HDD disks for total unique files in this folder across the pool
            for disk in disks.iter().filter(|d| !d.is_landing_zone) {
                let hdd_folder_phys = disk.to_physical_path(&norm_pattern);
                if hdd_folder_phys.exists() {
                    for entry in walkdir::WalkDir::new(&hdd_folder_phys).into_iter().filter_map(|e| e.ok()) {
                        if entry.file_type().is_file() {
                            if let Ok(rel) = entry.path().strip_prefix(&disk.pooldata_path) {
                                let rel_str = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                                let norm_rel = crate::core::replication::ReplicationRulesEngine::normalize_path(&rel_str);
                                if crate::core::replication::ReplicationRulesEngine::matches_pattern(&norm_rel, &norm_pattern) {
                                    let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                    if total_files_set.insert(norm_rel) {
                                        total_bytes += sz;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let total_files = total_files_set.len() as u64;
            let is_fully_cached = total_files > 0 && cached_files >= total_files;

            overviews.push(PinnedFolderOverview {
                id: rule.id,
                path_pattern: rule.path_pattern,
                replica_count: rule.replica_count,
                enabled: rule.enabled,
                description: rule.description,
                cached_files,
                cached_bytes,
                total_files,
                total_bytes,
                is_fully_cached,
            });
        }

        overviews
    }

    /// Spawns periodic background sync loop
    pub fn start_background_loop(self: Arc<Self>, pool: Arc<StoragePool>) {
        tokio::spawn(async move {
            // Initial sync 5 seconds after service start so cache is populated immediately
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            {
                let pool_clone = pool.clone();
                let this = self.clone();
                tokio::spawn(async move {
                    let enabled = pool_clone.config.read().await.cache.plex.enabled;
                    if enabled {
                        let _ = this.sync_and_precache(pool_clone.clone()).await;
                    }
                    let _ = Self::sync_pinned_fast_cache(pool_clone).await;
                });
            }

            loop {
                let (enabled, interval_mins) = {
                    let cfg = pool.config.read().await;
                    (cfg.cache.plex.enabled, cfg.cache.plex.sync_interval_minutes)
                };

                let interval_secs = (interval_mins * 60).max(60);
                tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;

                let pool_clone = pool.clone();
                let this = self.clone();
                tokio::spawn(async move {
                    if enabled {
                        let _ = this.sync_and_precache(pool_clone.clone()).await;
                    }
                    let _ = Self::sync_pinned_fast_cache(pool_clone).await;
                });
            }
        });
    }
}

fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    now.to_string()
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

    #[test]
    fn test_cleanup_empty_parent_dirs() {
        let temp_dir = std::env::temp_dir().join("drivespanner_test_cleanup_dirs");
        let base_dir = temp_dir.join("CacheBase");
        let season1_dir = base_dir.join("TV").join("Show").join("Season 1");
        let season2_dir = base_dir.join("TV").join("Show").join("Season 2");
        fs::create_dir_all(&season1_dir).unwrap();
        fs::create_dir_all(&season2_dir).unwrap();

        let ep1 = season1_dir.join("S01E01.mkv");
        let ep2 = season2_dir.join("S02E01.mkv");
        File::create(&ep1).unwrap();
        File::create(&ep2).unwrap();

        // Evict S01E01
        fs::remove_file(&ep1).unwrap();
        PlexCacheService::cleanup_empty_parent_dirs(&ep1, &base_dir);

        // Season 1 should be gone, but Season 2 and Show should remain
        assert!(!season1_dir.exists(), "Season 1 folder should be cleaned up as empty");
        assert!(season2_dir.exists(), "Season 2 folder should still exist");
        assert!(base_dir.join("TV").join("Show").exists(), "Show folder should still exist because Season 2 has files");
        assert!(base_dir.exists(), "Base cache directory must never be removed");

        // Evict S02E01
        fs::remove_file(&ep2).unwrap();
        PlexCacheService::cleanup_empty_parent_dirs(&ep2, &base_dir);

        // Now Season 2, Show, and TV should all be cleaned up, but base_dir still exists
        assert!(!season2_dir.exists(), "Season 2 folder should be cleaned up as empty");
        assert!(!base_dir.join("TV").join("Show").exists(), "Show folder should be cleaned up");
        assert!(!base_dir.join("TV").exists(), "TV folder should be cleaned up");
        assert!(base_dir.exists(), "Base cache directory must remain intact");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_plex_sync_status_persistence_roundtrip() {
        let status = PlexSyncStatus {
            connected: true,
            last_sync_time: Some("1700000000-sync".to_string()),
            error_message: None,
            items: vec![CachedMediaItem {
                title: "Severance - S01E01".to_string(),
                media_type: "episode".to_string(),
                grandparent_title: Some("Severance".to_string()),
                season: Some(1),
                episode: Some(1),
                rel_path: "/TV/Severance/S01E01.mkv".to_string(),
                size_bytes: 1048576,
                source: "Continue Watching".to_string(),
                cached_at: 1700000000,
            }],
            total_cached_bytes: 1048576,
            cache_drive_id: Some("ssd-1".to_string()),
            cache_drive_path: Some("C:\\".to_string()),
            evicted_items_count: 3,
            last_evicted_summary: Some("Severance - S01E01 returned to HDD".to_string()),
        };

        let json = serde_json::to_string_pretty(&status).unwrap();
        let loaded: PlexSyncStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.connected, true);
        assert_eq!(loaded.evicted_items_count, 3);
        assert_eq!(loaded.last_evicted_summary, Some("Severance - S01E01 returned to HDD".to_string()));
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].rel_path, "/TV/Severance/S01E01.mkv");
    }

    #[test]
    fn test_pinned_fast_cache_overview_and_rule_matching() {
        use crate::core::replication::ReplicationRulesEngine;
        use crate::config::FolderReplicationRule;

        // Verify pattern matching
        let music_rule = "/Music";
        assert!(ReplicationRulesEngine::matches_pattern("/Music/Track01.flac", music_rule));
        assert!(ReplicationRulesEngine::matches_pattern("/Music/Rock/Queen/Bohemian.mp3", music_rule));
        assert!(!ReplicationRulesEngine::matches_pattern("/Videos/Concert.mkv", music_rule));

        // Verify FolderReplicationRule serialization with pin_to_cache
        let rule = FolderReplicationRule {
            id: "rule-music-1".to_string(),
            path_pattern: "/Music".to_string(),
            replica_count: 2,
            enabled: true,
            description: "Music pinned for instant playback".to_string(),
            pin_to_cache: true,
        };

        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("\"pin_to_cache\":true"));

        let loaded: FolderReplicationRule = serde_json::from_str(&json).unwrap();
        assert!(loaded.pin_to_cache);
        assert_eq!(loaded.replica_count, 2);

        // Verify backwards compatibility when pin_to_cache is missing in JSON
        let legacy_json = r#"{"id":"rule-1","path_pattern":"/Photos","replica_count":3,"enabled":true,"description":"Photos"}"#;
        let legacy_rule: FolderReplicationRule = serde_json::from_str(legacy_json).unwrap();
        assert!(!legacy_rule.pin_to_cache); // defaults to false

        // Verify PinnedFolderOverview serialization
        let overview = PinnedFolderOverview {
            id: "rule-music-1".to_string(),
            path_pattern: "/Music".to_string(),
            replica_count: 2,
            enabled: true,
            description: "Music library".to_string(),
            cached_files: 150,
            cached_bytes: 1048576000,
            total_files: 150,
            total_bytes: 1048576000,
            is_fully_cached: true,
        };
        let overview_json = serde_json::to_string(&overview).unwrap();
        let loaded_ov: PinnedFolderOverview = serde_json::from_str(&overview_json).unwrap();
        assert_eq!(loaded_ov.cached_files, 150);
        assert!(loaded_ov.is_fully_cached);
    }
}
