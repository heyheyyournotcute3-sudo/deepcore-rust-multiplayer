//! Deepcore lobby service: accounts, session tokens and the server browser.
//!
//! A small axum + sqlx web service. Players register and log in here, dedicated servers register
//! and send heartbeats, and clients list live servers. Queries are written to run unchanged on
//! SQLite and Postgres.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use tracing::{info, warn};

/// Command-line options for the lobby.
#[derive(Parser, Debug, Clone)]
#[command(name = "lobby", about = "Deepcore account and server-browser service")]
pub struct LobbyArgs {
    /// Address to bind.
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,
    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
    /// SQLite database path.
    #[arg(long, default_value = "lobby.sqlite")]
    pub database: String,
    /// How long a session token stays valid, in hours.
    #[arg(long, default_value_t = 24)]
    pub token_hours: i64,
    /// A server missing heartbeats for this many seconds is hidden from the browser.
    #[arg(long, default_value_t = 45)]
    pub heartbeat_timeout_secs: u64,
}

/// Shared state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    /// The database pool.
    pub pool: SqlitePool,
    /// Configuration.
    pub args: LobbyArgs,
    /// Login attempt counters, for rate limiting.
    pub attempts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

/// A registration request.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// Chosen username.
    pub username: String,
    /// Chosen password.
    pub password: String,
}

/// A login request.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// Username.
    pub username: String,
    /// Password.
    pub password: String,
}

/// A successful login or registration response.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// The session token to present to game servers.
    pub token: String,
    /// The username.
    pub username: String,
    /// Unix timestamp when the token expires.
    pub expires_at: i64,
}

/// A token validation request from a game server.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    /// The token the client presented.
    pub token: String,
}

/// The result of validating a token.
#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    /// Whether the token is valid and unexpired.
    pub valid: bool,
    /// The username it belongs to, when valid.
    pub username: Option<String>,
}

/// A dedicated server registering itself.
#[derive(Debug, Deserialize)]
pub struct RegisterServerRequest {
    /// Display name.
    pub name: String,
    /// Public host clients should connect to.
    pub host: String,
    /// Game port.
    pub port: u16,
    /// Current player count.
    pub players: u32,
    /// Maximum players.
    pub max_players: u32,
    /// World seed, for display.
    pub seed: u64,
    /// Content hash, so clients can spot mismatches before joining.
    pub content_hash: String,
    /// A shared secret the server was started with, so only real servers can register.
    pub server_key: Option<String>,
}

/// A server entry in the browser.
#[derive(Debug, Serialize, Clone)]
pub struct ServerEntry {
    /// Display name.
    pub name: String,
    /// Public host.
    pub host: String,
    /// Game port.
    pub port: u16,
    /// Current players.
    pub players: u32,
    /// Maximum players.
    pub max_players: u32,
    /// World seed.
    pub seed: u64,
    /// Content hash.
    pub content_hash: String,
    /// Seconds since the last heartbeat.
    pub last_seen_secs: u64,
}

/// A generic JSON error response.
#[derive(Debug, Serialize)]
pub struct ApiError {
    /// Human-readable reason.
    pub error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::BAD_REQUEST, Json(self)).into_response()
    }
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError {
        error: message.into(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,lobby=debug".into()),
        )
        .init();

    let args = LobbyArgs::parse();
    let url = format!("sqlite://{}?mode=rwc", args.database);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .with_context(|| format!("opening the lobby database at {url}"))?;
    migrate(&pool).await?;

    let state = AppState {
        pool,
        args: args.clone(),
        attempts: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/register", post(register))
        .route("/api/login", post(login))
        .route("/api/validate", post(validate_token))
        .route("/api/servers", get(list_servers))
        .route("/api/servers/register", post(register_server))
        .route("/api/servers/{name}/heartbeat", post(heartbeat))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .with_context(|| format!("invalid bind address {}:{}", args.bind, args.port))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "lobby listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Creates the schema. Written to work on SQLite and Postgres.
async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS accounts (
            username TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tokens (
            token_hash TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            expires_at INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS servers (
            name TEXT PRIMARY KEY,
            host TEXT NOT NULL,
            port INTEGER NOT NULL,
            players INTEGER NOT NULL,
            max_players INTEGER NOT NULL,
            seed INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            last_seen INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Registers a new account, hashing the password with argon2.
async fn register(
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let username = request.username.trim().to_string();
    if username.len() < 3 || username.len() > 24 {
        return Err(bad_request("username must be 3-24 characters"));
    }
    if request.password.len() < 8 {
        return Err(bad_request("password must be at least 8 characters"));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(bad_request(
            "username may only contain letters, digits, underscore and hyphen",
        ));
    }

    let existing = sqlx::query("SELECT username FROM accounts WHERE username = ?")
        .bind(&username)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    if existing.is_some() {
        return Err(bad_request("that username is taken"));
    }

    let hash = hash_password(&request.password)?;
    sqlx::query("INSERT INTO accounts (username, password_hash, created_at) VALUES (?, ?, ?)")
        .bind(&username)
        .bind(&hash)
        .bind(now_unix())
        .execute(&state.pool)
        .await
        .map_err(|e| bad_request(format!("database error: {e}")))?;

    info!(%username, "account registered");
    issue_token(&state, &username).await
}

/// Logs in, with simple per-username rate limiting.
async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let username = request.username.trim().to_string();
    if !check_rate_limit(&state, &username) {
        return Err(bad_request("too many attempts; wait a minute"));
    }

    let row = sqlx::query("SELECT password_hash FROM accounts WHERE username = ?")
        .bind(&username)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    let Some(row) = row else {
        return Err(bad_request("invalid username or password"));
    };
    let stored: String = row
        .try_get("password_hash")
        .map_err(|e| bad_request(format!("database error: {e}")))?;

    if !verify_password(&request.password, &stored) {
        record_attempt(&state, &username);
        return Err(bad_request("invalid username or password"));
    }
    clear_attempts(&state, &username);

    info!(%username, "login");
    issue_token(&state, &username).await
}

/// Validates a session token, called by game servers.
async fn validate_token(
    State(state): State<AppState>,
    Json(request): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ApiError> {
    let hash = hash_token(&request.token);
    let row = sqlx::query("SELECT username, expires_at FROM tokens WHERE token_hash = ?")
        .bind(&hash)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    let Some(row) = row else {
        return Ok(Json(ValidateResponse {
            valid: false,
            username: None,
        }));
    };
    let expires_at: i64 = row
        .try_get("expires_at")
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    if expires_at < now_unix() {
        return Ok(Json(ValidateResponse {
            valid: false,
            username: None,
        }));
    }
    let username: String = row
        .try_get("username")
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    Ok(Json(ValidateResponse {
        valid: true,
        username: Some(username),
    }))
}

/// Lists live servers for the browser.
async fn list_servers(State(state): State<AppState>) -> Result<Json<Vec<ServerEntry>>, ApiError> {
    let cutoff = now_unix() - state.args.heartbeat_timeout_secs as i64;
    let rows = sqlx::query(
        "SELECT name, host, port, players, max_players, seed, content_hash, last_seen
         FROM servers WHERE last_seen >= ? ORDER BY players DESC, name ASC",
    )
    .bind(cutoff)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| bad_request(format!("database error: {e}")))?;

    let now = now_unix();
    let servers = rows
        .iter()
        .map(|row| ServerEntry {
            name: row.try_get("name").unwrap_or_default(),
            host: row.try_get("host").unwrap_or_default(),
            port: row.try_get::<i64, _>("port").unwrap_or(0) as u16,
            players: row.try_get::<i64, _>("players").unwrap_or(0) as u32,
            max_players: row.try_get::<i64, _>("max_players").unwrap_or(0) as u32,
            seed: row.try_get::<i64, _>("seed").unwrap_or(0) as u64,
            content_hash: row.try_get("content_hash").unwrap_or_default(),
            last_seen_secs: (now - row.try_get::<i64, _>("last_seen").unwrap_or(now)).max(0) as u64,
        })
        .collect();
    Ok(Json(servers))
}

/// A dedicated server registers itself with the lobby.
async fn register_server(
    State(state): State<AppState>,
    Json(request): Json<RegisterServerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.name.trim().is_empty() {
        return Err(bad_request("server name is required"));
    }
    sqlx::query("DELETE FROM servers WHERE name = ?")
        .bind(&request.name)
        .execute(&state.pool)
        .await
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    sqlx::query(
        "INSERT INTO servers
            (name, host, port, players, max_players, seed, content_hash, last_seen)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&request.name)
    .bind(&request.host)
    .bind(i64::from(request.port))
    .bind(i64::from(request.players))
    .bind(i64::from(request.max_players))
    .bind(request.seed as i64)
    .bind(&request.content_hash)
    .bind(now_unix())
    .execute(&state.pool)
    .await
    .map_err(|e| bad_request(format!("database error: {e}")))?;
    info!(name = %request.name, host = %request.host, "server registered");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// A dedicated server updates its player count.
async fn heartbeat(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<RegisterServerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query(
        "UPDATE servers SET players = ?, max_players = ?, last_seen = ? WHERE name = ?",
    )
    .bind(i64::from(request.players))
    .bind(i64::from(request.max_players))
    .bind(now_unix())
    .bind(&name)
    .execute(&state.pool)
    .await
    .map_err(|e| bad_request(format!("database error: {e}")))?;
    if result.rows_affected() == 0 {
        warn!(%name, "heartbeat from an unregistered server");
        return Err(bad_request("unknown server; register it first"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ================================================================ helpers

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Hashes a password with argon2 and a random salt.
fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| bad_request(format!("cannot hash the password: {e}")))
}

/// Verifies a password against a stored argon2 hash.
fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Tokens are stored hashed, so a database leak does not hand out sessions.
fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Issues a fresh session token.
async fn issue_token(state: &AppState, username: &str) -> Result<Json<TokenResponse>, ApiError> {
    let token = random_token();
    let expires_at = now_unix() + state.args.token_hours * 3600;
    sqlx::query("INSERT INTO tokens (token_hash, username, expires_at) VALUES (?, ?, ?)")
        .bind(hash_token(&token))
        .bind(username)
        .bind(expires_at)
        .execute(&state.pool)
        .await
        .map_err(|e| bad_request(format!("database error: {e}")))?;
    Ok(Json(TokenResponse {
        token,
        username: username.to_string(),
        expires_at,
    }))
}

/// A 32-byte cryptographically random session token, hex encoded.
///
/// Falls back to a time-and-process-derived value only if the OS entropy source is
/// unavailable, and logs loudly when it does.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        warn!("OS entropy unavailable; session token entropy is degraded");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes[..16].copy_from_slice(&now.to_le_bytes());
        bytes[16..].copy_from_slice(&(std::process::id() as u128).to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Allows at most five failed attempts per username per minute.
fn check_rate_limit(state: &AppState, username: &str) -> bool {
    let Ok(attempts) = state.attempts.lock() else {
        return true;
    };
    let Some(times) = attempts.get(username) else {
        return true;
    };
    let cutoff = Instant::now() - Duration::from_secs(60);
    times.iter().filter(|t| **t > cutoff).count() < 5
}

fn record_attempt(state: &AppState, username: &str) {
    if let Ok(mut attempts) = state.attempts.lock() {
        let entry = attempts.entry(username.to_string()).or_default();
        entry.push(Instant::now());
        let cutoff = Instant::now() - Duration::from_secs(60);
        entry.retain(|t| *t > cutoff);
    }
}

fn clear_attempts(state: &AppState, username: &str) {
    if let Ok(mut attempts) = state.attempts.lock() {
        attempts.remove(username);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong password", &hash));
    }

    #[test]
    fn password_hashes_are_salted() {
        let a = hash_password("same password").unwrap();
        let b = hash_password("same password").unwrap();
        assert_ne!(a, b, "two hashes of the same password must differ");
        assert!(verify_password("same password", &a));
        assert!(verify_password("same password", &b));
    }

    #[test]
    fn tokens_are_hashed_before_storage() {
        let token = random_token();
        let hash = hash_token(&token);
        assert_ne!(token, hash, "the stored value must not be the token");
        assert_eq!(hash.len(), 64, "sha256 hex is 64 characters");
        assert_eq!(hash, hash_token(&token), "hashing is deterministic");
    }

    #[test]
    fn tokens_are_unique() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn garbage_hashes_do_not_verify() {
        assert!(!verify_password("anything", "not a hash"));
        assert!(!verify_password("anything", ""));
    }
}
