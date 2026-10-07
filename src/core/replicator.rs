use crate::config::SharedConfig;
use crate::core::disk::MemberDisk;
use crate::core::placement::PlacementEngine;
use crate::core::replication::ReplicationRulesEngine;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock as SyncRwLock};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{mpsc, RwLock};
use log::{error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationJobStatus {
    pub active_job: Option<String>,
    pub queue_size: usize,
    pub completed_count: u64,
    pub replicated_bytes: u64,
    pub is_scrubbing: bool,
    pub last_scrub_time: Option<String>,
    pub under_replicated_count: u64,
}

pub struct ReplicatorService {
    queue_tx: mpsc::Sender<String>,
    active_job: Arc<RwLock<Option<String>>>,
    queue_size: Arc<AtomicU64>,
    completed_count: Arc<AtomicU64>,
    replicated_bytes: Arc<AtomicU64>,
    is_scrubbing: Arc<AtomicBool>,
    under_replicated_count: Arc<AtomicU64>,
}

impl ReplicatorService {
    pub fn new(
        disks: Arc<SyncRwLock<Vec<MemberDisk>>>,
        config: SharedConfig,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<String>(10000);
        let active_job = Arc::new(RwLock::new(None));
        let queue_size = Arc::new(AtomicU64::new(0));
        let completed_count = Arc::new(AtomicU64::new(0));
        let replicated_bytes = Arc::new(AtomicU64::new(0));
        let is_scrubbing = Arc::new(AtomicBool::new(false));
        let under_replicated_count = Arc::new(AtomicU64::new(0));

        let active_job_clone = active_job.clone();
        let queue_size_clone = queue_size.clone();
        let completed_count_clone = completed_count.clone();
        let replicated_bytes_clone = replicated_bytes.clone();
        let disks_clone = disks.clone();
        let config_clone = config.clone();

        let handle = tokio::spawn(async move {
            info!("Replication background worker started.");
            while let Some(rel_path) = rx.recv().await {
                queue_size_clone.fetch_sub(1, Ordering::SeqCst);
                *active_job_clone.write().await = Some(rel_path.clone());

                let res = Self::replicate_file_internal(
                    &rel_path,
                    &disks_clone,
                    &config_clone,
                    &replicated_bytes_clone,
                )
                .await;

                if let Err(e) = res {
                    error!("Error replicating file '{}': {}", rel_path, e);
                } else {
                    completed_count_clone.fetch_add(1, Ordering::SeqCst);
                }

                *active_job_clone.write().await = None;
            }
            info!("Replication worker channel closed.");
        });

        (
            Self {
                queue_tx: tx,
                active_job,
                queue_size,
                completed_count,
                replicated_bytes,
                is_scrubbing,
                under_replicated_count,
            },
            handle,
        )
    }

    pub async fn queue_replication(&self, relative_path: String) {
        self.queue_size.fetch_add(1, Ordering::SeqCst);
        let _ = self.queue_tx.send(relative_path).await;
    }

    pub async fn get_status(&self) -> ReplicationJobStatus {
        let active_job = self.active_job.read().await.clone();
        ReplicationJobStatus {
            active_job,
            queue_size: self.queue_size.load(Ordering::SeqCst) as usize,
            completed_count: self.completed_count.load(Ordering::SeqCst),
            replicated_bytes: self.replicated_bytes.load(Ordering::SeqCst),
            is_scrubbing: self.is_scrubbing.load(Ordering::SeqCst),
            last_scrub_time: None,
            under_replicated_count: self.under_replicated_count.load(Ordering::SeqCst),
        }
    }

    pub async fn replicate_file_internal(
        rel_path: &str,
        disks_lock: &Arc<SyncRwLock<Vec<MemberDisk>>>,
        config_lock: &SharedConfig,
        bytes_counter: &Arc<AtomicU64>,
    ) -> std::io::Result<()> {
        let (default_replicas, rules) = {
            let cfg = config_lock.read().await;
            (cfg.replication.default_replicas, cfg.replication.rules.clone())
        };

        let target_replicas =
            ReplicationRulesEngine::get_target_replicas(rel_path, default_replicas, &rules);

        let disks = disks_lock.read().unwrap().clone();
        if disks.is_empty() {
            return Ok(());
        }

        // Find existing copies on member disks
        let mut existing_disks: Vec<(MemberDisk, PathBuf, u64)> = Vec::new();
        let mut existing_disk_ids = HashSet::new();

        for disk in &disks {
            let full_path = disk.to_physical_path(rel_path);
            if full_path.is_file() {
                if let Ok(meta) = fs::metadata(&full_path) {
                    existing_disks.push((disk.clone(), full_path, meta.len()));
                    existing_disk_ids.insert(disk.id.clone());
                }
            }
        }

        if existing_disks.is_empty() {
            // File does not exist anywhere on disks
            return Ok(());
        }

        let actual_replicas = existing_disks.len();
        if actual_replicas >= target_replicas {
            // Already satisfied
            return Ok(());
        }

        let needed = target_replicas - actual_replicas;
        let (source_disk, source_file_path, file_size) = &existing_disks[0];

        // Select candidate destination disks
        let destinations = PlacementEngine::select_replica_disks(
            &disks,
            &existing_disk_ids,
            needed,
            *file_size,
        );

        if destinations.is_empty() {
            warn!(
                "Not enough available disks to satisfy replica count {} for '{}'. Currently on {} disks.",
                target_replicas, rel_path, actual_replicas
            );
            return Ok(());
        }

        for dest_disk in destinations {
            let dest_full_path = dest_disk.to_physical_path(rel_path);
            if let Some(parent) = dest_full_path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            info!(
                "Replicating '{}' ({} bytes) from disk {} to disk {}",
                rel_path, file_size, source_disk.id, dest_disk.id
            );

            // Safe copy
            fs::copy(source_file_path, &dest_full_path)?;
            bytes_counter.fetch_add(*file_size, Ordering::SeqCst);
        }

        Ok(())
    }

    /// Performs a full health scrub across the pool, identifying all files
    /// and queuing any under-replicated files for duplication.
    pub async fn run_scrub(
        disks_lock: Arc<SyncRwLock<Vec<MemberDisk>>>,
        config_lock: SharedConfig,
        service: Arc<ReplicatorService>,
    ) -> (u64, u64) {
        service.is_scrubbing.store(true, Ordering::SeqCst);
        info!("Starting storage pool scrub...");

        let disks = disks_lock.read().unwrap().clone();
        let (default_replicas, rules) = {
            let cfg = config_lock.read().await;
            (cfg.replication.default_replicas, cfg.replication.rules.clone())
        };

        // Map of relative path -> Set of disk IDs containing that file
        let mut file_map: HashMap<String, (u64, HashSet<String>)> = HashMap::new();

        for disk in &disks {
            if !disk.pooldata_path.exists() {
                continue;
            }

            for entry in walkdir::WalkDir::new(&disk.pooldata_path)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    if let Ok(rel) = entry.path().strip_prefix(&disk.pooldata_path) {
                        let rel_str = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        let entry = file_map.entry(rel_str).or_insert((size, HashSet::new()));
                        entry.1.insert(disk.id.clone());
                    }
                }
            }
        }

        let mut scanned_count = 0u64;
        let mut under_rep = 0u64;

        for (rel_path, (_, disk_ids)) in &file_map {
            scanned_count += 1;
            let target = ReplicationRulesEngine::get_target_replicas(rel_path, default_replicas, &rules);
            if disk_ids.len() < target {
                under_rep += 1;
                service.queue_replication(rel_path.clone()).await;
            }
        }

        service.under_replicated_count.store(under_rep, Ordering::SeqCst);
        service.is_scrubbing.store(false, Ordering::SeqCst);
        info!(
            "Scrub complete: {} files scanned, {} under-replicated files queued for recovery.",
            scanned_count, under_rep
        );

        (scanned_count, under_rep)
    }
}
