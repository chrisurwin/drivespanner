use crate::config::SharedConfig;
use crate::core::disk::MemberDisk;
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BalancerState {
    Idle,
    Analyzing,
    Balancing,
    Paused,
    Completed,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancerStatus {
    pub state: BalancerState,
    pub current_file: Option<String>,
    pub current_file_bytes_moved: u64,
    pub current_file_total_bytes: u64,
    pub total_files_moved: u64,
    pub total_bytes_moved: u64,
    pub speed_mb_per_sec: f64,
    pub pool_imbalance_percent: f64,
    pub is_balanced: bool,
}

pub struct DriveBalancer {
    pub state: Arc<RwLock<BalancerState>>,
    pub cancel_flag: Arc<AtomicBool>,
    pub current_file: Arc<RwLock<Option<String>>>,
    pub current_file_bytes_moved: Arc<AtomicU64>,
    pub current_file_total_bytes: Arc<AtomicU64>,
    pub total_files_moved: Arc<AtomicU64>,
    pub total_bytes_moved: Arc<AtomicU64>,
    pub speed_mb_per_sec: Arc<RwLock<f64>>,
    pub pool_imbalance_percent: Arc<RwLock<f64>>,
}

impl DriveBalancer {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(BalancerState::Idle)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            current_file: Arc::new(RwLock::new(None)),
            current_file_bytes_moved: Arc::new(AtomicU64::new(0)),
            current_file_total_bytes: Arc::new(AtomicU64::new(0)),
            total_files_moved: Arc::new(AtomicU64::new(0)),
            total_bytes_moved: Arc::new(AtomicU64::new(0)),
            speed_mb_per_sec: Arc::new(RwLock::new(0.0)),
            pool_imbalance_percent: Arc::new(RwLock::new(0.0)),
        }
    }

    pub async fn stop(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        *self.state.write().await = BalancerState::Idle;
        *self.current_file.write().await = None;
        info!("Drive balancing cancellation signaled.");
    }

    pub async fn get_status(&self) -> BalancerStatus {
        let state = self.state.read().await.clone();
        let current_file = self.current_file.read().await.clone();
        let current_file_bytes_moved = self.current_file_bytes_moved.load(Ordering::SeqCst);
        let current_file_total_bytes = self.current_file_total_bytes.load(Ordering::SeqCst);
        let total_files_moved = self.total_files_moved.load(Ordering::SeqCst);
        let total_bytes_moved = self.total_bytes_moved.load(Ordering::SeqCst);
        let speed = *self.speed_mb_per_sec.read().await;
        let imbalance = *self.pool_imbalance_percent.read().await;

        BalancerStatus {
            state,
            current_file,
            current_file_bytes_moved,
            current_file_total_bytes,
            total_files_moved,
            total_bytes_moved,
            speed_mb_per_sec: speed,
            pool_imbalance_percent: imbalance,
            is_balanced: imbalance <= 10.0,
        }
    }

    pub fn calculate_imbalance(disks: &[MemberDisk]) -> f64 {
        let active: Vec<_> = disks
            .iter()
            .filter_map(|d| {
                let s = d.query_stats();
                if s.online && s.enabled && s.total_bytes > 0 {
                    Some(s.used_percent)
                } else {
                    None
                }
            })
            .collect();

        if active.len() < 2 {
            return 0.0;
        }

        let max_val = active.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_val = active.iter().cloned().fold(f64::INFINITY, f64::min);
        (max_val - min_val).max(0.0)
    }

    pub async fn run_balance(
        balancer: Arc<DriveBalancer>,
        disks_lock: Arc<std::sync::RwLock<Vec<MemberDisk>>>,
        config_lock: SharedConfig,
    ) {
        balancer.cancel_flag.store(false, Ordering::SeqCst);
        *balancer.state.write().await = BalancerState::Analyzing;
        info!("Starting drive balance evaluation...");

        let (threshold, _strategy) = {
            let cfg = config_lock.read().await;
            (
                cfg.balancer.imbalance_threshold_percent,
                cfg.balancer.strategy.clone(),
            )
        };

        let disks = disks_lock.read().unwrap().clone();
        let imbalance = Self::calculate_imbalance(&disks);
        *balancer.pool_imbalance_percent.write().await = imbalance;

        if imbalance <= threshold {
            info!(
                "Pool is balanced (imbalance: {:.1}%, threshold: {:.1}%). No files moved.",
                imbalance, threshold
            );
            *balancer.state.write().await = BalancerState::Completed;
            return;
        }

        info!(
            "Pool imbalance is {:.1}% (threshold: {:.1}%). Calculating migration plan...",
            imbalance, threshold
        );
        *balancer.state.write().await = BalancerState::Balancing;

        // Group disks into most-used (sources) and least-used (destinations)
        let mut disk_stats: Vec<_> = disks
            .iter()
            .map(|d| (d.clone(), d.query_stats()))
            .filter(|(_, s)| s.online && s.enabled && !s.read_only)
            .collect();

        if disk_stats.len() < 2 {
            *balancer.state.write().await = BalancerState::Completed;
            return;
        }

        disk_stats.sort_by(|a, b| b.1.used_percent.partial_cmp(&a.1.used_percent).unwrap());

        let source_disk = disk_stats.first().unwrap().0.clone();
        let dest_disk = disk_stats.last().unwrap().0.clone();

        info!(
            "Balancing source disk {} ({:.1}% used) -> dest disk {} ({:.1}% used)",
            source_disk.id, disk_stats.first().unwrap().1.used_percent,
            dest_disk.id, disk_stats.last().unwrap().1.used_percent
        );

        // Enumerate candidates from source disk
        if !source_disk.pooldata_path.exists() {
            *balancer.state.write().await = BalancerState::Completed;
            return;
        }

        let mut candidates = Vec::new();
        for entry in walkdir::WalkDir::new(&source_disk.pooldata_path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Ok(rel) = entry.path().strip_prefix(&source_disk.pooldata_path) {
                    let rel_str = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                    let dest_path = dest_disk.to_physical_path(&rel_str);
                    // Crucial: check that destination doesn't already have this file (maintain replica segregation)
                    if !dest_path.exists() {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        candidates.push((rel_str, size, entry.path().to_path_buf(), dest_path));
                    }
                }
            }
        }

        info!("Identified {} candidate files for rebalancing", candidates.len());

        let start_time = std::time::Instant::now();

        for (rel_path, file_size, src_path, dst_path) in candidates {
            if balancer.cancel_flag.load(Ordering::SeqCst) {
                info!("Balancing cancelled by user.");
                *balancer.state.write().await = BalancerState::Idle;
                *balancer.current_file.write().await = None;
                return;
            }

            *balancer.current_file.write().await = Some(rel_path.clone());
            balancer.current_file_total_bytes.store(file_size, Ordering::SeqCst);
            balancer.current_file_bytes_moved.store(0, Ordering::SeqCst);

            // Safe 3-step atomic move:
            // 1. Ensure target directory exists
            if let Some(parent) = dst_path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            // 2. Copy file to destination
            let copy_res = fs::copy(&src_path, &dst_path);
            match copy_res {
                Ok(bytes) if bytes == file_size => {
                    // 3. Remove source file only after successful copy
                    let _ = fs::remove_file(&src_path);

                    balancer.total_files_moved.fetch_add(1, Ordering::SeqCst);
                    let total_b = balancer.total_bytes_moved.fetch_add(bytes, Ordering::SeqCst) + bytes;
                    balancer.current_file_bytes_moved.store(bytes, Ordering::SeqCst);

                    let elapsed_sec = start_time.elapsed().as_secs_f64().max(0.001);
                    let speed = (total_b as f64 / (1024.0 * 1024.0)) / elapsed_sec;
                    *balancer.speed_mb_per_sec.write().await = speed;

                    // Clean empty directories on source disk
                    if let Some(src_parent) = src_path.parent() {
                        let _ = fs::remove_dir(src_parent); // Only succeeds if empty
                    }
                }
                Ok(_) => {
                    error!("Copy size mismatch for '{}'. Removing destination replica.", rel_path);
                    let _ = fs::remove_file(&dst_path);
                }
                Err(e) => {
                    error!("Failed to copy '{}' for balancing: {}", rel_path, e);
                }
            }

            // Check if current imbalance is now satisfied
            let current_disks = disks_lock.read().unwrap().clone();
            let cur_imb = Self::calculate_imbalance(&current_disks);
            *balancer.pool_imbalance_percent.write().await = cur_imb;
            if cur_imb <= threshold {
                info!("Pool has reached target balance of {:.1}%.", cur_imb);
                break;
            }
        }

        *balancer.current_file.write().await = None;
        *balancer.state.write().await = BalancerState::Completed;
        info!("Drive balancing run complete.");
    }
}
