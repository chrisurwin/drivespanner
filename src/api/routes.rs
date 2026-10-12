use crate::config::{FolderReplicationRule, PoolConfig};
use crate::core::balancer::DriveBalancer;
use crate::core::pool::StoragePool;
use crate::core::replicator::ReplicatorService;
use crate::vfs::PoolMounter;
use axum::{
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Thread-safe in-memory rate limiter for sensitive authentication endpoints
#[derive(Debug)]
pub struct RateLimiter {
    attempts: Mutex<HashMap<String, (usize, Instant)>>,
    max_attempts: usize,
    window_duration: Duration,
}

impl RateLimiter {
    pub fn new(max_attempts: usize, window_secs: u64) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            window_duration: Duration::from_secs(window_secs),
        }
    }

    /// Checks if a client is allowed to make an attempt.
    /// Returns `Ok(())` if allowed, or `Err(remaining_secs)` if throttled.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();

        // Evict expired entries if table grows large
        if map.len() > 1000 {
            let win = self.window_duration;
            map.retain(|_, (_, start)| now.duration_since(*start) < win);
        }

        if let Some((count, start)) = map.get(key) {
            let elapsed = now.duration_since(*start);
            if elapsed < self.window_duration {
                if *count >= self.max_attempts {
                    let remaining = self.window_duration.saturating_sub(elapsed).as_secs().max(1);
                    return Err(remaining);
                }
            } else {
                map.remove(key);
            }
        }
        Ok(())
    }

    /// Records an attempt (e.g. failed login or setup call)
    pub fn record_attempt(&self, key: &str) {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();

        if let Some((count, start)) = map.get_mut(key) {
            if now.duration_since(*start) < self.window_duration {
                *count += 1;
                return;
            }
        }
        map.insert(key.to_string(), (1, now));
    }

    /// Resets the counter upon a successful authentication
    pub fn reset(&self, key: &str) {
        let mut map = self.attempts.lock().unwrap();
        map.remove(key);
    }
}

pub fn get_client_ip_key(headers: &HeaderMap) -> String {
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(s) = forwarded.to_str() {
            if let Some(first) = s.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(s) = real_ip.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<StoragePool>,
    pub rate_limiter: Arc<RateLimiter>,
}

impl AppState {
    pub fn new(pool: Arc<StoragePool>) -> Self {
        Self {
            pool,
            rate_limiter: Arc::new(RateLimiter::new(5, 60)), // 5 attempts per 60 seconds
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddDriveRequest {
    pub drive_path: String,
    pub is_landing_zone: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveDriveRequest {
    pub disk_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddRuleRequest {
    pub path_pattern: String,
    pub replica_count: usize,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateDefaultReplicationRequest {
    pub default_replicas: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MountRequest {
    pub drive_letter: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthSetupRequest {
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthLoginRequest {
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateCacheConfigRequest {
    pub enabled: Option<bool>,
    pub move_threshold_percent: Option<f64>,
    pub plex_enabled: Option<bool>,
    pub plex_precache_next_episode: Option<bool>,
    pub plex_server_url: Option<String>,
    pub plex_token: Option<String>,
    pub plex_max_cache_gb: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub message: String,
    pub data: Option<T>,
}

use tower_http::cors::{Any, CorsLayer};

pub fn create_router(app_state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Auth routes (unprotected)
        .route("/api/auth/status", get(get_auth_status))
        .route("/api/auth/setup", post(auth_setup))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/logout", post(auth_logout))
        // Updates routes
        .route("/api/updates", get(get_updates))
        .route("/api/updates/check", post(check_updates))
        // Cache & Plex routes
        .route("/api/cache/status", get(get_cache_status))
        .route("/api/cache/config", post(set_cache_config))
        .route("/api/cache/sync", post(sync_cache))
        .route("/api/cache/move", post(run_cache_mover_endpoint))
        // Protected API routes
        .route("/api/status", get(get_status))
        .route("/api/drives", get(get_drives))
        .route("/api/drives/add", post(add_drive))
        .route("/api/drives/remove", post(remove_drive))
        .route("/api/rules", get(get_rules))
        .route("/api/rules", post(add_rule))
        .route("/api/rules/default", post(set_default_replication))
        .route("/api/rules/:id", delete(delete_rule))
        .route("/api/browse", get(browse_folder))
        .route("/api/scrub/start", post(start_scrub))
        .route("/api/balance/start", post(start_balance))
        .route("/api/balance/stop", post(stop_balance))
        .route("/api/mount", post(mount_pool))
        .route("/api/unmount", post(unmount_pool))
        .route("/api/mount/letter", post(set_mount_letter))
        // Web UI static files
        .route("/", get(serve_index))
        .route("/app.js", get(serve_js))
        .route("/style.css", get(serve_css))
        .route("/logo.png", get(serve_logo))
        .route("/favicon.ico", get(serve_favicon))
        .layer(cors)
        .with_state(app_state)
}

fn is_authenticated(headers: &HeaderMap, cfg: &PoolConfig) -> bool {
    if !cfg.api.auth_enabled {
        return true;
    }
    if cfg.api.password_hash.is_empty() {
        return false;
    }
    if cfg.api.session_token.is_empty() {
        return false;
    }

    // Check session token expiration (7-day TTL if creation timestamp is recorded)
    if cfg.api.session_token_created_at > 0 {
        if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
            const SEVEN_DAYS_SECS: u64 = 7 * 24 * 3600;
            if now.as_secs().saturating_sub(cfg.api.session_token_created_at) > SEVEN_DAYS_SECS {
                return false;
            }
        }
    }

    if let Some(auth_val) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = auth_val.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                if crate::api::auth::constant_time_eq_str(token.trim(), &cfg.api.session_token) {
                    return true;
                }
            }
        }
    }

    if let Some(cookie_val) = headers.get(header::COOKIE) {
        if let Ok(s) = cookie_val.to_str() {
            for part in s.split(';') {
                let part = part.trim();
                if let Some(token) = part.strip_prefix("session=") {
                    if crate::api::auth::constant_time_eq_str(token.trim(), &cfg.api.session_token) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

fn check_auth(headers: &HeaderMap, cfg: &PoolConfig) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if !is_authenticated(headers, cfg) {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "success": false,
                "error": "Authentication required",
                "auth_required": true
            })),
        ))
    } else {
        Ok(())
    }
}

// Auth Handlers
async fn get_auth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let cfg = state.pool.config.read().await;
    let auth_enabled = cfg.api.auth_enabled;
    let setup_required = auth_enabled && cfg.api.password_hash.is_empty();
    let authenticated = if !auth_enabled {
        true
    } else if setup_required {
        false
    } else {
        is_authenticated(&headers, &cfg)
    };

    Json(serde_json::json!({
        "auth_enabled": auth_enabled,
        "setup_required": setup_required,
        "authenticated": authenticated,
    }))
}

async fn auth_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AuthSetupRequest>,
) -> Response {
    let client_key = get_client_ip_key(&headers);
    if let Err(retry_after) = state.rate_limiter.check(&client_key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "success": false,
                "message": format!("Too many requests. Please wait {} seconds.", retry_after),
                "retry_after_secs": retry_after
            })),
        ).into_response();
    }

    let mut cfg = state.pool.config.write().await;
    if !cfg.api.password_hash.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": "Administrator password is already configured. Please log in."
            })),
        ).into_response();
    }

    let pwd = payload.password.trim();
    if pwd.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": "Password must be at least 8 characters long."
            })),
        ).into_response();
    }

    let hashed = crate::api::auth::hash_password(pwd, None);
    let secret = Uuid::new_v4().to_string();
    let token = crate::api::auth::generate_session_token(&secret);
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    cfg.api.password_hash = hashed;
    cfg.api.session_secret = secret;
    cfg.api.session_token = token.clone();
    cfg.api.session_token_created_at = now_secs;
    let config_path = PoolConfig::get_config_path();
    if let Err(e) = cfg.save_to(&config_path) {
        log::error!("Failed to save administrator password to {}: {}", config_path.display(), e);
    } else {
        log::info!("Administrator password configured and saved to {}", config_path.display());
    }

    (
        StatusCode::OK,
        [(header::SET_COOKIE, format!("session={}; Path=/; HttpOnly; SameSite=Strict", token))],
        Json(serde_json::json!({
            "success": true,
            "message": "Admin password configured successfully.",
            "token": token
        })),
    ).into_response()
}

async fn auth_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AuthLoginRequest>,
) -> Response {
    let client_key = get_client_ip_key(&headers);
    if let Err(retry_after) = state.rate_limiter.check(&client_key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "success": false,
                "message": format!("Too many failed login attempts. Please wait {} seconds before trying again.", retry_after),
                "retry_after_secs": retry_after
            })),
        ).into_response();
    }

    let mut cfg = state.pool.config.write().await;
    if !cfg.api.auth_enabled {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "token": "no-auth-required"
            })),
        ).into_response();
    }

    if cfg.api.password_hash.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": "Initial setup required."
            })),
        ).into_response();
    }

    if crate::api::auth::verify_password(&payload.password, &cfg.api.password_hash) {
        state.rate_limiter.reset(&client_key);

        let token = crate::api::auth::generate_session_token(&cfg.api.session_secret);
        let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        cfg.api.session_token = token.clone();
        cfg.api.session_token_created_at = now_secs;
        let _ = cfg.save_to(PoolConfig::get_config_path());

        (
            StatusCode::OK,
            [(header::SET_COOKIE, format!("session={}; Path=/; HttpOnly; SameSite=Strict", token))],
            Json(serde_json::json!({
                "success": true,
                "token": token
            })),
        ).into_response()
    } else {
        state.rate_limiter.record_attempt(&client_key);

        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "success": false,
                "message": "Invalid password."
            })),
        ).into_response()
    }
}

async fn auth_logout(State(state): State<AppState>) -> Response {
    let mut cfg = state.pool.config.write().await;
    cfg.api.session_token = String::new();
    cfg.api.session_token_created_at = 0;
    let _ = cfg.save_to(PoolConfig::get_config_path());

    (
        StatusCode::OK,
        [(header::SET_COOKIE, "session=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict".to_string())],
        Json(serde_json::json!({
            "success": true,
            "message": "Logged out successfully."
        })),
    ).into_response()
}

// Updates Handlers
async fn get_updates(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.pool.updater.get_status().await;
    Json(status)
}

async fn check_updates(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.pool.updater.check_for_updates().await;
    Json(status)
}

// Cache & Plex Handlers
async fn get_cache_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    let cache_cfg = cfg.cache.clone();
    drop(cfg);

    let plex_status = state.pool.plex.get_status().await;

    let cache_disk_stats = {
        let disks = state.pool.disks.read().unwrap();
        disks.iter().find(|d| d.is_landing_zone).map(|d| d.query_stats())
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "cache_disk": cache_disk_stats,
        "config": cache_cfg,
        "plex_status": plex_status,
    })))
}

async fn set_cache_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateCacheConfigRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = state.pool.config.write().await;
    check_auth(&headers, &cfg)?;

    if let Some(en) = payload.enabled {
        cfg.cache.enabled = en;
    }
    if let Some(thresh) = payload.move_threshold_percent {
        cfg.cache.move_threshold_percent = thresh.clamp(10.0, 99.0);
    }
    if let Some(pe) = payload.plex_enabled {
        cfg.cache.plex.enabled = pe;
    }
    if let Some(pne) = payload.plex_precache_next_episode {
        cfg.cache.plex.precache_next_episode = pne;
    }
    if let Some(url) = payload.plex_server_url {
        cfg.cache.plex.server_url = url;
    }
    if let Some(tok) = payload.plex_token {
        cfg.cache.plex.token = tok;
    }
    if let Some(quota) = payload.plex_max_cache_gb {
        cfg.cache.plex.max_cache_gb = quota;
    }

    let config_path = PoolConfig::get_config_path();
    if let Err(e) = cfg.save_to(&config_path) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("Failed to save cache configuration: {}", e)
            })),
        ));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Cache and Plex settings saved successfully.",
        "config": cfg.cache.clone(),
    })))
}

async fn sync_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    match state.pool.plex.sync_and_precache(state.pool.clone()).await {
        Ok(sync_status) => Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("Plex sync complete: {} item(s) cached ({:.2} GB).", sync_status.items.len(), sync_status.total_cached_bytes as f64 / 1_073_741_824.0),
            "status": sync_status,
        }))),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": format!("Plex cache sync failed: {}", e)
            })),
        )),
    }
}

async fn run_cache_mover_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    match crate::core::plex::PlexCacheService::run_cache_mover(state.pool.clone()).await {
        Ok(count) => Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("SSD Cache Mover complete: {} file(s) flushed to HDD storage array.", count),
            "moved_count": count,
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("Cache mover error: {}", e)
            })),
        )),
    }
}

// Protected Pool Handlers
async fn get_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    let overview = state.pool.get_overview_stats().await;
    let replication = state.pool.replicator.get_status().await;
    let balancer = state.pool.balancer.get_status().await;

    Ok(Json(serde_json::json!({
        "overview": overview,
        "replication": replication,
        "balancer": balancer,
    })))
}

async fn get_drives(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    let member_stats = state.pool.get_member_disks_stats().await;
    let available = state.pool.get_available_system_drives().await;

    Ok(Json(serde_json::json!({
        "member_drives": member_stats,
        "available_system_drives": available,
    })))
}

async fn add_drive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AddDriveRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    let is_lz = payload.is_landing_zone.unwrap_or(false);
    match state.pool.add_member_drive(payload.drive_path.clone(), is_lz).await {
        Ok(_) => Ok(Json(ApiResponse {
            success: true,
            message: format!("Drive {} successfully added to pool", payload.drive_path),
            data: None::<()>,
        })),
        Err(e) => Ok(Json(ApiResponse {
            success: false,
            message: format!("Failed to add drive: {}", e),
            data: None::<()>,
        })),
    }
}

async fn remove_drive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<RemoveDriveRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    match state.pool.remove_member_drive(&payload.disk_id).await {
        Ok(_) => Ok(Json(ApiResponse {
            success: true,
            message: "Member drive removed from pool".to_string(),
            data: None::<()>,
        })),
        Err(e) => Ok(Json(ApiResponse {
            success: false,
            message: format!("Failed to remove drive: {}", e),
            data: None::<()>,
        })),
    }
}

async fn get_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    Ok(Json(serde_json::json!({
        "default_replicas": cfg.replication.default_replicas,
        "rules": cfg.replication.rules,
    })))
}

async fn add_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AddRuleRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = state.pool.config.write().await;
    check_auth(&headers, &cfg)?;

    let id = Uuid::new_v4().to_string();
    let desc = payload.description.unwrap_or_else(|| {
        format!("Replicate {} copies for {}", payload.replica_count, payload.path_pattern)
    });

    cfg.replication.rules.push(FolderReplicationRule {
        id,
        path_pattern: payload.path_pattern,
        replica_count: payload.replica_count,
        enabled: true,
        description: desc,
    });
    let _ = cfg.save_to(PoolConfig::get_config_path());

    Ok(Json(ApiResponse {
        success: true,
        message: "Replication rule saved".to_string(),
        data: None::<()>,
    }))
}

async fn set_default_replication(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateDefaultReplicationRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = state.pool.config.write().await;
    check_auth(&headers, &cfg)?;

    cfg.replication.default_replicas = payload.default_replicas.max(1);
    let _ = cfg.save_to(PoolConfig::get_config_path());

    Ok(Json(ApiResponse {
        success: true,
        message: format!("Default replication set to {}x", payload.default_replicas),
        data: None::<()>,
    }))
}

async fn delete_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(rule_id): AxumPath<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = state.pool.config.write().await;
    check_auth(&headers, &cfg)?;

    cfg.replication.rules.retain(|r| r.id != rule_id);
    let _ = cfg.save_to(PoolConfig::get_config_path());

    Ok(Json(ApiResponse {
        success: true,
        message: "Rule deleted".to_string(),
        data: None::<()>,
    }))
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
}

async fn browse_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BrowseQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    let folder_path = query.path.unwrap_or_else(|| "/".to_string());
    match state.pool.list_folder(&folder_path).await {
        Ok(entries) => Ok(Json(serde_json::json!({
            "current_path": folder_path,
            "entries": entries,
        }))),
        Err(e) => Ok(Json(serde_json::json!({
            "error": format!("Failed to read folder: {}", e),
            "entries": [],
        }))),
    }
}

async fn start_scrub(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    let pool = state.pool.clone();
    tokio::spawn(async move {
        ReplicatorService::run_scrub(pool.disks.clone(), pool.config.clone(), pool.replicator.clone()).await;
    });

    Ok(Json(ApiResponse {
        success: true,
        message: "Pool scrub started in background".to_string(),
        data: None::<()>,
    }))
}

async fn start_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    let pool = state.pool.clone();
    tokio::spawn(async move {
        DriveBalancer::run_balance(pool.balancer.clone(), pool.disks.clone(), pool.config.clone()).await;
    });

    Ok(Json(ApiResponse {
        success: true,
        message: "Drive balancing started in background".to_string(),
        data: None::<()>,
    }))
}

async fn stop_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    state.pool.balancer.stop().await;
    Ok(Json(ApiResponse {
        success: true,
        message: "Drive balancing stopped".to_string(),
        data: None::<()>,
    }))
}

async fn mount_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MountRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    let configured_letter = cfg.mount_point.clone();
    drop(cfg);

    let raw_letter = payload
        .drive_letter
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(configured_letter);

    let clean = raw_letter.trim().trim_end_matches(['\\', '/']);
    let letter_char = clean
        .chars()
        .find(|c| c.is_ascii_alphabetic())
        .unwrap_or('V')
        .to_ascii_uppercase();
    let final_letter = format!("{}:", letter_char);

    // If letter changed, persist to configuration
    {
        let mut cfg = state.pool.config.write().await;
        if cfg.mount_point != final_letter {
            cfg.mount_point = final_letter.clone();
            let _ = cfg.save_to(PoolConfig::get_config_path());
        }
    }

    match PoolMounter::start_mount(state.pool.clone(), final_letter.clone()) {
        Ok(_) => Ok(Json(ApiResponse {
            success: true,
            message: format!("Virtual pool successfully mounted on {}", final_letter),
            data: None::<()>,
        })),
        Err(e) => Ok(Json(ApiResponse {
            success: false,
            message: format!("Mount failed: {}", e),
            data: None::<()>,
        })),
    }
}

async fn unmount_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let cfg = state.pool.config.read().await;
    check_auth(&headers, &cfg)?;
    drop(cfg);

    match PoolMounter::stop_mount(state.pool.clone()) {
        Ok(_) => Ok(Json(ApiResponse {
            success: true,
            message: "Virtual pool unmounted successfully".to_string(),
            data: None::<()>,
        })),
        Err(e) => Ok(Json(ApiResponse {
            success: false,
            message: format!("Unmount failed: {}", e),
            data: None::<()>,
        })),
    }
}

async fn set_mount_letter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MountRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let mut cfg = state.pool.config.write().await;
    check_auth(&headers, &cfg)?;

    let raw_letter = payload.drive_letter.unwrap_or_else(|| "V:".to_string());
    let clean = raw_letter.trim().trim_end_matches(['\\', '/']);
    let letter_char = clean
        .chars()
        .find(|c| c.is_ascii_alphabetic())
        .unwrap_or('V')
        .to_ascii_uppercase();
    let final_letter = format!("{}:", letter_char);

    // Verify target letter is not used by an active member disk
    let disks = state.pool.disks.read().unwrap();
    for d in disks.iter() {
        let d_letter = d
            .drive_path
            .to_string_lossy()
            .chars()
            .find(|c| c.is_ascii_alphabetic())
            .unwrap_or(' ')
            .to_ascii_uppercase();
        if d_letter == letter_char {
            return Ok(Json(ApiResponse {
                success: false,
                message: format!(
                    "Drive {}: is a physical member disk in the pool and cannot be used as mount letter",
                    final_letter
                ),
                data: None::<()>,
            }));
        }
    }

    cfg.mount_point = final_letter.clone();
    let _ = cfg.save_to(PoolConfig::get_config_path());

    Ok(Json(ApiResponse {
        success: true,
        message: format!("Virtual pool mount letter updated to {}", final_letter),
        data: None::<()>,
    }))
}

// Embedded Static Assets
const HTML_CONTENT: &str = include_str!("../web/index.html");
const JS_CONTENT: &str = include_str!("../web/app.js");
const CSS_CONTENT: &str = include_str!("../web/style.css");
const LOGO_PNG: &[u8] = include_bytes!("../web/logo.png");
const FAVICON_ICO: &[u8] = include_bytes!("../web/favicon.ico");

async fn serve_index() -> impl IntoResponse {
    Html(HTML_CONTENT)
}

async fn serve_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], JS_CONTENT)
}

async fn serve_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], CSS_CONTENT)
}

async fn serve_logo() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], LOGO_PNG)
}

async fn serve_favicon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/x-icon")], FAVICON_ICO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_auth_setup_http() {
        let orig_content = std::fs::read_to_string("config.json").ok();

        let cfg = PoolConfig::default();
        let shared_cfg = Arc::new(tokio::sync::RwLock::new(cfg));
        let (pool, _) = StoragePool::new(shared_cfg);
        let app = create_router(AppState::new(Arc::new(pool)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = r#"{"password":"mypassword123"}"#;
        let req = format!(
            "POST /api/auth/setup HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            addr,
            body.len(),
            body
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp);

        // Restore original config.json
        if let Some(c) = orig_content {
            let _ = std::fs::write("config.json", c);
        }

        println!("Received HTTP response:\n{}", resp_str);
        assert!(resp_str.contains("200 OK"));
        assert!(resp_str.contains("Admin password configured successfully."));
    }

    #[tokio::test]
    async fn test_auth_setup_rejects_short_password() {
        let cfg = PoolConfig::default();
        let shared_cfg = Arc::new(tokio::sync::RwLock::new(cfg));
        let (pool, _) = StoragePool::new(shared_cfg);
        let app = create_router(AppState::new(Arc::new(pool)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = r#"{"password":"short"}"#; // 5 chars, less than 8
        let req = format!(
            "POST /api/auth/setup HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            addr,
            body.len(),
            body
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp);

        assert!(resp_str.contains("400 Bad Request"));
        assert!(resp_str.contains("Password must be at least 8 characters long."));
    }

    #[test]
    fn test_rate_limiter_unit() {
        let limiter = RateLimiter::new(3, 60);
        let client = "192.168.1.50";

        // First 3 checks succeed
        assert!(limiter.check(client).is_ok());
        limiter.record_attempt(client);
        assert!(limiter.check(client).is_ok());
        limiter.record_attempt(client);
        assert!(limiter.check(client).is_ok());
        limiter.record_attempt(client);

        // 4th check is blocked
        let err = limiter.check(client);
        assert!(err.is_err());
        assert!(err.unwrap_err() <= 60);

        // Another client is unaffected
        assert!(limiter.check("192.168.1.51").is_ok());

        // Reset clears client throttling
        limiter.reset(client);
        assert!(limiter.check(client).is_ok());
    }

    #[test]
    fn test_is_authenticated_scenarios() {
        let mut cfg = PoolConfig::default();
        let mut headers = HeaderMap::new();

        // 1. Auth disabled -> always true
        cfg.api.auth_enabled = false;
        assert!(is_authenticated(&headers, &cfg));

        // 2. Auth enabled, no password configured -> false
        cfg.api.auth_enabled = true;
        cfg.api.password_hash = String::new();
        assert!(!is_authenticated(&headers, &cfg));

        // 3. Auth enabled, password configured, session token set
        cfg.api.password_hash = "pbkdf2$20000$salt$hash".to_string();
        cfg.api.session_token = "valid-session-token-12345".to_string();
        cfg.api.session_token_created_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        // No headers provided -> false
        assert!(!is_authenticated(&headers, &cfg));

        // Valid Bearer header -> true
        headers.insert(header::AUTHORIZATION, "Bearer valid-session-token-12345".parse().unwrap());
        assert!(is_authenticated(&headers, &cfg));

        // Invalid Bearer header -> false
        headers.insert(header::AUTHORIZATION, "Bearer wrong-token".parse().unwrap());
        assert!(!is_authenticated(&headers, &cfg));
        headers.remove(header::AUTHORIZATION);

        // Valid Cookie header -> true
        headers.insert(header::COOKIE, "other=abc; session=valid-session-token-12345; user=admin".parse().unwrap());
        assert!(is_authenticated(&headers, &cfg));

        // Invalid Cookie header -> false
        headers.insert(header::COOKIE, "session=invalid-token".parse().unwrap());
        assert!(!is_authenticated(&headers, &cfg));

        // Expired token (> 7 days) -> false
        headers.insert(header::AUTHORIZATION, "Bearer valid-session-token-12345".parse().unwrap());
        cfg.api.session_token_created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(8 * 24 * 3600); // 8 days ago
        assert!(!is_authenticated(&headers, &cfg));
    }

    #[tokio::test]
    async fn test_auth_login_and_logout_flow() {
        let orig_content = std::fs::read_to_string("config.json").ok();

        let mut cfg = PoolConfig::default();
        cfg.api.auth_enabled = true;
        cfg.api.password_hash = crate::api::auth::hash_password("ValidPassword123!", None);
        cfg.api.session_secret = "test-secret".to_string();

        let shared_cfg = Arc::new(tokio::sync::RwLock::new(cfg));
        let (pool, _) = StoragePool::new(shared_cfg);
        let app = create_router(AppState::new(Arc::new(pool)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // 1. Five failed login attempts
        for _ in 0..5 {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let body = r#"{"password":"WrongPassword!"}"#;
            let req = format!(
                "POST /api/auth/login HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                addr, body.len(), body
            );
            stream.write_all(req.as_bytes()).await.unwrap();
            let mut resp = Vec::new();
            stream.read_to_end(&mut resp).await.unwrap();
            let resp_str = String::from_utf8_lossy(&resp);
            assert!(resp_str.contains("401 Unauthorized"));
        }

        // 2. 6th attempt is throttled by rate limiter with 429
        {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let body = r#"{"password":"ValidPassword123!"}"#;
            let req = format!(
                "POST /api/auth/login HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                addr, body.len(), body
            );
            stream.write_all(req.as_bytes()).await.unwrap();
            let mut resp = Vec::new();
            stream.read_to_end(&mut resp).await.unwrap();
            let resp_str = String::from_utf8_lossy(&resp);
            assert!(resp_str.contains("429 Too Many Requests"));
            assert!(resp_str.contains("retry_after_secs"));
        }

        // 3. Test logout clears session cookie
        {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let req = format!(
                "POST /api/auth/logout HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                addr
            );
            stream.write_all(req.as_bytes()).await.unwrap();
            let mut resp = Vec::new();
            stream.read_to_end(&mut resp).await.unwrap();
            let resp_str = String::from_utf8_lossy(&resp);
            assert!(resp_str.contains("200 OK"));
            assert!(resp_str.contains("Max-Age=0"));
        }

        if let Some(c) = orig_content {
            let _ = std::fs::write("config.json", c);
        }
    }
}


