use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub pool_id: String,
    pub pool_name: String,
    pub mount_point: String, // Drive letter like "V:" or "X:"
    pub member_drives: Vec<MemberDriveConfig>,
    pub replication: ReplicationConfig,
    pub balancer: BalancerConfig,
    pub api: ApiConfig,
    #[serde(default)]
    pub updates: UpdateConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberDriveConfig {
    pub id: String,
    pub drive_path: String, // e.g. "D:\\"
    #[serde(alias = "poolpart_name")]
    pub pooldata_name: String, // e.g. "PoolData.4e019924-..."
    pub enabled: bool,
    pub read_only: bool,
    pub is_landing_zone: bool, // Fast SSD cache / landing disk
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    pub default_replicas: usize, // e.g. 1 (no dup) or 2 (2x dup)
    pub rules: Vec<FolderReplicationRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderReplicationRule {
    pub id: String,
    pub path_pattern: String, // e.g. "/Documents", "/Photos", "/Backups"
    pub replica_count: usize, // e.g. 2, 3
    pub enabled: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BalanceStrategy {
    EqualizePercentage, // Balance drives so they have equal % used
    EqualizeFreeSpace,  // Balance drives so they have equal free GB
    SequentialFill,     // Fill disk 1 to threshold, then disk 2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancerConfig {
    pub strategy: BalanceStrategy,
    pub auto_balance: bool,
    pub imbalance_threshold_percent: f64, // e.g. 10.0%
    pub schedule_interval_minutes: u64,   // e.g. 60 min
    pub rate_limit_mb_per_sec: u64,       // 0 for unlimited
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_host")]
    pub host: String, // "127.0.0.1" or "0.0.0.0"
    #[serde(default = "default_port")]
    pub port: u16,    // default 8989
    #[serde(default = "default_true")]
    pub auth_enabled: bool,
    #[serde(default)]
    pub password_hash: String,
    #[serde(default)]
    pub session_secret: String,
    #[serde(default)]
    pub session_token: String,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8989
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    #[serde(default = "default_true")]
    pub check_enabled: bool,
    #[serde(default = "default_github_repo")]
    pub github_repo: String,
    #[serde(default = "default_check_interval")]
    pub check_interval_hours: u64,
}

fn default_true() -> bool {
    true
}

fn default_github_repo() -> String {
    "chris/poolforge".to_string()
}

fn default_check_interval() -> u64 {
    6
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_enabled: true,
            github_repo: default_github_repo(),
            check_interval_hours: default_check_interval(),
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        let pool_id = Uuid::new_v4().to_string();
        Self {
            pool_id: pool_id.clone(),
            pool_name: "PoolForge Storage".to_string(),
            mount_point: "V:".to_string(),
            member_drives: Vec::new(),
            replication: ReplicationConfig {
                default_replicas: 1,
                rules: vec![
                    FolderReplicationRule {
                        id: Uuid::new_v4().to_string(),
                        path_pattern: "/Documents".to_string(),
                        replica_count: 2,
                        enabled: true,
                        description: "Important documents duplicated across 2 disks".to_string(),
                    },
                    FolderReplicationRule {
                        id: Uuid::new_v4().to_string(),
                        path_pattern: "/Family Photos".to_string(),
                        replica_count: 3,
                        enabled: true,
                        description: "Family photos replicated across 3 disks".to_string(),
                    },
                ],
            },
            balancer: BalancerConfig {
                strategy: BalanceStrategy::EqualizePercentage,
                auto_balance: false,
                imbalance_threshold_percent: 10.0,
                schedule_interval_minutes: 60,
                rate_limit_mb_per_sec: 0,
            },
            api: ApiConfig {
                host: "0.0.0.0".to_string(),
                port: 8989,
                auth_enabled: true,
                password_hash: String::new(),
                session_secret: Uuid::new_v4().to_string(),
                session_token: String::new(),
            },
            updates: UpdateConfig::default(),
        }
    }
}

pub type SharedConfig = Arc<RwLock<PoolConfig>>;

impl PoolConfig {
    pub fn get_config_path() -> PathBuf {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        let prog_files_path = std::env::var("ProgramFiles")
            .ok()
            .map(|pf| PathBuf::from(pf).join("PoolForge").join("config.json"));

        let prog_data_path = std::env::var("ProgramData")
            .ok()
            .map(|pd| PathBuf::from(pd).join("PoolForge").join("config.json"));

        // 1. Check next to executable if not System32
        if let Some(ref dir) = exe_dir {
            let p = dir.join("config.json");
            let is_system32 = dir.to_string_lossy().to_lowercase().contains("system32");
            if p.exists() && !is_system32 {
                return p;
            }
        }

        // 2. Check Program Files
        if let Some(ref p) = prog_files_path {
            if p.exists() {
                return p.clone();
            }
        }

        // 3. Check ProgramData
        if let Some(ref p) = prog_data_path {
            if p.exists() {
                return p.clone();
            }
        }

        // 4. Check local current working directory if not System32
        let current_dir = std::env::current_dir().unwrap_or_default();
        let is_current_system32 = current_dir.to_string_lossy().to_lowercase().contains("system32");
        let local_path = PathBuf::from("config.json");
        if local_path.exists() && !is_current_system32 {
            return local_path;
        }

        // 5. Automatic Migration: If C:\Windows\System32\config.json exists, migrate it!
        let system32_config = PathBuf::from("C:\\Windows\\System32\\config.json");
        if system32_config.exists() {
            let dest = prog_files_path
                .clone()
                .or_else(|| prog_data_path.clone())
                .or_else(|| {
                    exe_dir.as_ref().and_then(|d| {
                        if !d.to_string_lossy().to_lowercase().contains("system32") {
                            Some(d.join("config.json"))
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_else(|| PathBuf::from("config.json"));

            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(_) = fs::copy(&system32_config, &dest) {
                log::info!(
                    "Successfully migrated config from {} to {}",
                    system32_config.display(),
                    dest.display()
                );
                let _ = fs::remove_file(&system32_config);
                return dest;
            }
        }

        // 6. Default target for new installations
        if let Some(ref dir) = exe_dir {
            let is_system32 = dir.to_string_lossy().to_lowercase().contains("system32");
            if !is_system32 {
                return dir.join("config.json");
            }
        }

        if let Some(p) = prog_files_path {
            return p;
        }

        PathBuf::from("config.json")
    }

    pub fn load_or_default<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(cfg) = serde_json::from_str::<PoolConfig>(&content) {
                    return cfg;
                }
            }
        }
        let cfg = PoolConfig::default();
        let _ = cfg.save_to(path);
        cfg
    }

    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}
