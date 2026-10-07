use crate::config::{MemberDriveConfig, PoolConfig, SharedConfig};
use crate::core::balancer::DriveBalancer;
use crate::core::disk::{scan_system_drives, AvailableDriveInfo, DiskStats, MemberDisk};
use crate::core::replication::{ReplicationHealth, ReplicationRulesEngine};
use crate::core::replicator::ReplicatorService;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, RwLock as SyncRwLock};
use tokio::sync::RwLock;
use log::{error, info};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolOverviewStats {
    pub pool_id: String,
    pub pool_name: String,
    pub mount_point: String,
    pub is_mounted: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
    pub member_drive_count: usize,
    pub total_files_count: u64,
    pub replicated_files_count: u64,
    pub under_replicated_count: u64,
    pub pool_imbalance_percent: f64,
    pub status_badge: String, // "Healthy", "Degraded", "Balancing", "Replicating"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolFolderEntry {
    pub name: String,
    pub relative_path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub target_replicas: usize,
    pub actual_replicas: usize,
    pub replica_disks: Vec<String>,
    pub health: ReplicationHealth,
}

pub struct StoragePool {
    pub config: SharedConfig,
    pub disks: Arc<SyncRwLock<Vec<MemberDisk>>>,
    pub replicator: Arc<ReplicatorService>,
    pub balancer: Arc<DriveBalancer>,
    pub updater: Arc<crate::core::updater::UpdateChecker>,
    pub is_mounted: Arc<RwLock<bool>>,
}

impl StoragePool {
    pub fn new(config: SharedConfig) -> (Self, tokio::task::JoinHandle<()>) {
        let disks = Arc::new(SyncRwLock::new(Vec::new()));
        let (replicator, rep_handle) = ReplicatorService::new(disks.clone(), config.clone());
        let balancer = Arc::new(DriveBalancer::new());
        let updater = Arc::new(crate::core::updater::UpdateChecker::new("chris/poolforge".to_string()));

        let pool = Self {
            config,
            disks,
            replicator: Arc::new(replicator),
            balancer,
            updater,
            is_mounted: Arc::new(RwLock::new(false)),
        };

        (pool, rep_handle)
    }

    pub async fn initialize_from_config(&self) -> std::io::Result<()> {
        let cfg = self.config.read().await;
        let mut loaded_disks = Vec::new();

        for drive_cfg in &cfg.member_drives {
            match MemberDisk::new(
                drive_cfg.id.clone(),
                &drive_cfg.drive_path,
                &drive_cfg.pooldata_name,
                drive_cfg.enabled,
                drive_cfg.read_only,
                drive_cfg.is_landing_zone,
            ) {
                Ok(disk) => {
                    info!(
                        "Loaded member disk {} at {} ({})",
                        disk.id,
                        disk.drive_path.display(),
                        disk.pooldata_path.display()
                    );
                    loaded_disks.push(disk);
                }
                Err(e) => {
                    error!(
                        "Failed to load member disk {}: {}",
                        drive_cfg.drive_path, e
                    );
                }
            }
        }

        *self.disks.write().unwrap() = loaded_disks;

        self.updater.set_repo(cfg.updates.github_repo.clone()).await;
        if cfg.updates.check_enabled {
            self.updater.clone().start_background_loop(cfg.updates.check_interval_hours);
        }

        Ok(())
    }

    pub async fn get_overview_stats(&self) -> PoolOverviewStats {
        let cfg = self.config.read().await;
        let is_mounted = *self.is_mounted.read().await;
        let rep_status = self.replicator.get_status().await;
        let bal_status = self.balancer.get_status().await;

        let (total_bytes, free_bytes, total_files, member_count, imbalance) = {
            let disks = self.disks.read().unwrap();
            let mut total: u64 = 0;
            let mut free: u64 = 0;
            let mut files: u64 = 0;

            for disk in disks.iter() {
                let stats = disk.query_stats();
                if stats.online && stats.enabled {
                    total = total.saturating_add(stats.total_bytes);
                    free = free.saturating_add(stats.free_bytes);
                    files += stats.file_count;
                }
            }
            let imb = DriveBalancer::calculate_imbalance(&disks);
            (total, free, files, disks.len(), imb)
        };

        let used_bytes = total_bytes.saturating_sub(free_bytes);
        let used_percent = if total_bytes > 0 {
            (used_bytes as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        let status_badge = if bal_status.state == crate::core::balancer::BalancerState::Balancing {
            "Balancing".to_string()
        } else if rep_status.queue_size > 0 || rep_status.is_scrubbing {
            "Replicating".to_string()
        } else if rep_status.under_replicated_count > 0 {
            "Degraded".to_string()
        } else {
            "Healthy".to_string()
        };

        PoolOverviewStats {
            pool_id: cfg.pool_id.clone(),
            pool_name: cfg.pool_name.clone(),
            mount_point: cfg.mount_point.clone(),
            is_mounted,
            total_bytes,
            free_bytes,
            used_bytes,
            used_percent,
            member_drive_count: member_count,
            total_files_count: total_files,
            replicated_files_count: total_files.saturating_sub(rep_status.under_replicated_count),
            under_replicated_count: rep_status.under_replicated_count,
            pool_imbalance_percent: imbalance,
            status_badge,
        }
    }

    pub async fn get_member_disks_stats(&self) -> Vec<DiskStats> {
        let disks = self.disks.read().unwrap();
        disks.iter().map(|d| d.query_stats()).collect()
    }

    pub async fn get_available_system_drives(&self) -> Vec<AvailableDriveInfo> {
        let disks = self.disks.read().unwrap();
        let existing: Vec<String> = disks.iter().map(|d| d.drive_path.to_string_lossy().to_string()).collect();
        scan_system_drives(&existing)
    }

    pub async fn add_member_drive(
        &self,
        drive_path: String,
        is_landing_zone: bool,
    ) -> std::io::Result<()> {
        let pool_id = { self.config.read().await.pool_id.clone() };
        let disk_id = Uuid::new_v4().to_string();
        let pooldata_name = format!("PoolData.{}", pool_id);

        let disk = MemberDisk::new(
            disk_id.clone(),
            &drive_path,
            &pooldata_name,
            true,
            false,
            is_landing_zone,
        )?;

        // Update config
        {
            let mut cfg = self.config.write().await;
            cfg.member_drives.push(MemberDriveConfig {
                id: disk_id,
                drive_path: drive_path.clone(),
                pooldata_name,
                enabled: true,
                read_only: false,
                is_landing_zone,
            });
            let _ = cfg.save_to(PoolConfig::get_config_path());
        }

        self.disks.write().unwrap().push(disk);
        info!("Added new member disk {} to pool", drive_path);
        Ok(())
    }

    pub async fn remove_member_drive(&self, disk_id: &str) -> std::io::Result<()> {
        // Remove from memory
        self.disks.write().unwrap().retain(|d| d.id != disk_id);

        // Update config
        {
            let mut cfg = self.config.write().await;
            cfg.member_drives.retain(|d| d.id != disk_id);
            let _ = cfg.save_to(PoolConfig::get_config_path());
        }

        info!("Removed member disk {} from pool", disk_id);
        Ok(())
    }

    /// Merges folder contents across all member disks, presenting a single unified listing.
    /// Deduplicates files so that replicas appear only ONCE in the list!
    pub async fn list_folder(&self, relative_path: &str) -> std::io::Result<Vec<PoolFolderEntry>> {
        let (default_replicas, rules) = {
            let cfg = self.config.read().await;
            (cfg.replication.default_replicas, cfg.replication.rules.clone())
        };
        let disks = self.disks.read().unwrap().clone();

        let mut map: HashMap<String, PoolFolderEntry> = HashMap::new();

        for disk in disks.iter() {
            if !disk.pooldata_path.exists() {
                continue;
            }
            let phys_dir = disk.to_physical_path(relative_path);
            if !phys_dir.is_dir() {
                continue;
            }

            if let Ok(entries) = fs::read_dir(phys_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let file_type = entry.file_type().ok();
                    let is_dir = file_type.map(|t| t.is_dir()).unwrap_or(false);
                    let meta = entry.metadata().ok();
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let modified = meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    let entry_rel_path = if relative_path == "/" || relative_path.is_empty() {
                        format!("/{}", file_name)
                    } else {
                        format!("{}/{}", relative_path.trim_end_matches('/'), file_name)
                    };

                    let target_replicas = if is_dir {
                        1
                    } else {
                        ReplicationRulesEngine::get_target_replicas(
                            &entry_rel_path,
                            default_replicas,
                            &rules,
                        )
                    };

                    let existing = map.entry(file_name.clone()).or_insert_with(|| PoolFolderEntry {
                        name: file_name,
                        relative_path: entry_rel_path,
                        is_dir,
                        size,
                        modified,
                        target_replicas,
                        actual_replicas: 0,
                        replica_disks: Vec::new(),
                        health: ReplicationHealth::Healthy,
                    });

                    existing.actual_replicas += 1;
                    existing.replica_disks.push(disk.id.clone());

                    if !is_dir {
                        existing.health = if existing.actual_replicas < existing.target_replicas {
                            ReplicationHealth::UnderReplicated
                        } else if existing.actual_replicas > existing.target_replicas {
                            ReplicationHealth::OverReplicated
                        } else {
                            ReplicationHealth::Healthy
                        };
                    }
                }
            }
        }

        let mut results: Vec<PoolFolderEntry> = map.into_values().collect();
        // Sort: directories first, then alphabetically
        results.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(results)
    }
}
