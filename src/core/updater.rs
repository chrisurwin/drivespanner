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
    pub fn new(mut github_repo: String) -> Self {
        if github_repo.is_empty() || github_repo.contains("poolforge") {
            github_repo = "chrisurwin/drivespanner".to_string();
        }
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

    pub async fn set_repo(&self, mut repo: String) {
        if repo.is_empty() || repo.contains("poolforge") {
            repo = "chrisurwin/drivespanner".to_string();
        }
        *self.github_repo.write().await = repo;
    }

    /// Queries GitHub Releases API using curl.exe
    pub async fn check_for_updates(&self) -> UpdateStatus {
        let mut repo = self.github_repo.read().await.clone();
        if repo.is_empty() || repo.contains("poolforge") {
            repo = "chrisurwin/drivespanner".to_string();
            *self.github_repo.write().await = repo.clone();
        }
        let current_ver = env!("CARGO_PKG_VERSION");
        info!("Checking for updates for {} (current: v{})...", repo, current_ver);

        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
        let url_clone = url.clone();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("curl.exe")
                .arg("-s")
                .arg("-S")
                .arg("-L")
                .arg("-H")
                .arg("User-Agent: DriveSpanner-Updater")
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
                    Ok(rel) if rel.tag_name.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false) => {
                        let tag = rel.tag_name.unwrap();
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
                        st.release_name = rel.name.unwrap_or_else(|| format!("DriveSpanner v{}", clean_tag));
                        st.release_url = rel.html_url.unwrap_or_default();
                        st.release_notes = rel.body.unwrap_or_default();
                        st.published_at = rel.published_at.unwrap_or_default();
                        st.msi_download_url = msi_url;
                        st.error = None;

                        if is_newer {
                            info!("Update available: v{} (current is v{})", clean_tag, current_ver);
                        } else {
                            info!("DriveSpanner is up-to-date (v{})", current_ver);
                        }
                    }
                    Ok(_) => {
                        // Check if GitHub returned an API notice/error message (e.g. rate limit or not found)
                        #[derive(Deserialize)]
                        struct GitHubApiMessage {
                            message: Option<String>,
                        }
                        if let Ok(api_err) = serde_json::from_str::<GitHubApiMessage>(&stdout) {
                            if let Some(msg) = api_err.message {
                                warn!("GitHub API notice for {}: {}", repo, msg);
                                st.error = Some(format!("GitHub API notice: {}", msg));
                            } else {
                                st.error = Some("No releases found on GitHub repository".to_string());
                            }
                        } else {
                            st.error = Some("No valid release tag found in GitHub response".to_string());
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
            // Check shortly after startup so update status is immediately populated
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            self.check_for_updates().await;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_version_newer() {
        assert!(is_version_newer("1.3.1", "1.3.0"));
        assert!(is_version_newer("1.4.0", "1.3.0"));
        assert!(is_version_newer("2.0.0", "1.3.0"));
        assert!(is_version_newer("1.3.0.1", "1.3.0"));

        assert!(!is_version_newer("1.3.0", "1.3.0"));
        assert!(!is_version_newer("1.2.9", "1.3.0"));
        assert!(!is_version_newer("0.1.2", "1.3.0"));
        assert!(!is_version_newer("", "1.3.0"));
    }

    #[tokio::test]
    async fn test_repo_poolforge_auto_migration() {
        let updater = UpdateChecker::new("chrisurwin/poolforge".to_string());
        assert_eq!(*updater.github_repo.read().await, "chrisurwin/drivespanner");

        updater.set_repo("someother/poolforge".to_string()).await;
        assert_eq!(*updater.github_repo.read().await, "chrisurwin/drivespanner");

        updater.set_repo("".to_string()).await;
        assert_eq!(*updater.github_repo.read().await, "chrisurwin/drivespanner");
    }

    #[test]
    fn test_github_release_parsing() {
        let sample_json = r#"{
            "tag_name": "v1.4.0",
            "name": "DriveSpanner v1.4.0",
            "html_url": "https://github.com/chrisurwin/drivespanner/releases/tag/v1.4.0",
            "body": "Fixed update checking and added Fast Cache.",
            "published_at": "2026-10-14T10:00:00Z",
            "assets": [
                {
                    "name": "DriveSpanner-1.4.0-Setup.msi",
                    "browser_download_url": "https://github.com/chrisurwin/drivespanner/releases/download/v1.4.0/DriveSpanner-1.4.0-Setup.msi"
                }
            ]
        }"#;

        let rel: GitHubRelease = serde_json::from_str(sample_json).unwrap();
        assert_eq!(rel.tag_name, Some("v1.4.0".to_string()));
        let msi = rel.assets.and_then(|a| a.into_iter().find_map(|x| if x.name.unwrap_or_default().ends_with(".msi") { x.browser_download_url } else { None }));
        assert_eq!(msi, Some("https://github.com/chrisurwin/drivespanner/releases/download/v1.4.0/DriveSpanner-1.4.0-Setup.msi".to_string()));
    }
}
