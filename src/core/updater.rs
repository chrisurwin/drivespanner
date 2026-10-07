use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub update_available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_name: String,
    pub release_url: String,
    pub release_notes: String,
    pub published_at: String,
    pub msi_download_url: Option<String>,
    pub last_checked: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: Option<String>,
    name: Option<String>,
    html_url: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    assets: Option<Vec<GitHubAsset>>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: Option<String>,
    browser_download_url: Option<String>,
}

pub struct UpdateChecker {
    status: Arc<RwLock<UpdateStatus>>,
    github_repo: Arc<RwLock<String>>,
}

impl UpdateChecker {
    pub fn new(github_repo: String) -> Self {
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        let status = Arc::new(RwLock::new(UpdateStatus {
            update_available: false,
            current_version: current_version.clone(),
            latest_version: current_version,
            release_name: String::new(),
            release_url: String::new(),
            release_notes: String::new(),
            published_at: String::new(),
            msi_download_url: None,
            last_checked: None,
            error: None,
        }));

        Self {
            status,
            github_repo: Arc::new(RwLock::new(github_repo)),
        }
    }

    pub async fn get_status(&self) -> UpdateStatus {
        self.status.read().await.clone()
    }

    pub async fn set_repo(&self, repo: String) {
        *self.github_repo.write().await = repo;
    }

    /// Queries GitHub Releases API using curl.exe
    pub async fn check_for_updates(&self) -> UpdateStatus {
        let repo = self.github_repo.read().await.clone();
        let current_ver = env!("CARGO_PKG_VERSION");
        info!("Checking for updates for {} (current: v{})...", repo, current_ver);

        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
        let url_clone = url.clone();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("curl.exe")
                .arg("-s")
                .arg("-H")
                .arg("User-Agent: PoolForge-Updater")
                .arg("-H")
                .arg("Accept: application/vnd.github.v3+json")
                .arg("--max-time")
                .arg("10")
                .arg(&url_clone)
                .output()
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        .and_then(|r| r);

        let now_str = now_utc_rfc3339();

        let mut st = self.status.write().await;
        st.last_checked = Some(now_str);

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                match serde_json::from_str::<GitHubRelease>(&stdout) {
                    Ok(rel) => {
                        let tag = rel.tag_name.unwrap_or_default();
                        let clean_tag = tag.trim_start_matches('v').trim_start_matches('V');
                        let is_newer = is_version_newer(clean_tag, current_ver);

                        let msi_url = rel.assets.and_then(|assets| {
                            assets.into_iter().find_map(|a| {
                                let name = a.name.unwrap_or_default();
                                if name.ends_with(".msi") {
                                    a.browser_download_url
                                } else {
                                    None
                                }
                            })
                        });

                        st.update_available = is_newer;
                        st.latest_version = clean_tag.to_string();
                        st.release_name = rel.name.unwrap_or_else(|| format!("PoolForge v{}", clean_tag));
                        st.release_url = rel.html_url.unwrap_or_default();
                        st.release_notes = rel.body.unwrap_or_default();
                        st.published_at = rel.published_at.unwrap_or_default();
                        st.msi_download_url = msi_url;
                        st.error = None;

                        if is_newer {
                            info!("Update available: v{} (current is v{})", clean_tag, current_ver);
                        } else {
                            info!("PoolForge is up-to-date (v{})", current_ver);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse GitHub release JSON: {}", e);
                        st.error = Some(format!("Failed to parse release: {}", e));
                    }
                }
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                warn!("GitHub release check failed: {}", err);
                st.error = Some(format!("GitHub API error: {}", err.trim()));
            }
            Err(e) => {
                warn!("curl execution failed for update check: {}", e);
                st.error = Some(format!("Network check error: {}", e));
            }
        }

        st.clone()
    }

    /// Spawns background loop to check for updates periodically
    pub fn start_background_loop(self: Arc<Self>, interval_hours: u64) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let check_interval = std::time::Duration::from_secs(interval_hours.max(1) * 3600);
            loop {
                tokio::time::sleep(check_interval).await;
                self.check_for_updates().await;
            }
        })
    }
}

/// Simple semver comparator: returns true if latest > current
fn is_version_newer(latest: &str, current: &str) -> bool {
    let parse_nums = |s: &str| -> Vec<u32> {
        s.split('.')
            .map(|part| part.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };

    let v_latest = parse_nums(latest);
    let v_curr = parse_nums(current);

    for (l, c) in v_latest.iter().zip(v_curr.iter()) {
        if l > c {
            return true;
        }
        if l < c {
            return false;
        }
    }

    v_latest.len() > v_curr.len()
}

/// Computes an ISO 8601 / RFC 3339 UTC timestamp without third-party crates
fn now_utc_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let mins = (rem_secs % 3600) / 60;
    let s = rem_secs % 60;

    let mut d = days as i64;
    let mut year = 1970;
    loop {
        let leap = if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) { 1 } else { 0 };
        let days_in_year = 365 + leap;
        if d >= days_in_year {
            d -= days_in_year;
            year += 1;
        } else {
            break;
        }
    }
    let leap = if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) { 1 } else { 0 };
    let month_days = [31, 28 + leap, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1;
    for &md in &month_days {
        if d >= md {
            d -= md;
            month += 1;
        } else {
            break;
        }
    }
    let day = d + 1;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hours, mins, s)
}
