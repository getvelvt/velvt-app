//! Typed runtime configuration.
//!
//! This module owns validated service settings. It does not own secret
//! storage, service lifecycle, or business logic.

use std::path::PathBuf;
use velvt_shared_types::PROTOCOL_VERSION;

/// Typed runtime configuration for the Velvt local service.
/// Loaded from environment variables and/or a config file.
/// Never hardcodes paths, ports, or version numbers.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// Absolute path to the Unix domain socket file.
    pub socket_path: PathBuf,
    /// IPC protocol version this server speaks.
    pub protocol_version: u32,
    /// Base URL for the velvt-core cloud API.
    pub api_base_url: String,
    /// SQLite database file path.
    pub db_path: PathBuf,
    /// Maximum batch size before forcing an upload.
    pub upload_batch_max_events: usize,
    /// Maximum seconds between uploads while active.
    pub upload_interval_secs: u64,
    /// Number of malformed messages allowed before closing a connection.
    pub ipc_max_errors: usize,
    /// Structured tracing filter configured for the service.
    pub log_level: String,
}

impl ServiceConfig {
    /// Loads and validates runtime configuration.
    pub fn load() -> Result<Self, ConfigError> {
        let socket_path = std::env::var("VELVT_IPC_SOCKET_PATH").unwrap_or_else(|_| {
            include_str!("../../../proto/ipc_socket_path")
                .trim()
                .to_owned()
        });
        let ipc_max_errors = parse_env("VELVT_IPC_MAX_ERRORS", 3)?;

        Ok(Self {
            socket_path: expand_home(&socket_path)?,
            protocol_version: PROTOCOL_VERSION,
            api_base_url: std::env::var("VELVT_API_BASE_URL")
                .unwrap_or_else(|_| "https://api.velvt.ai".to_owned()),
            db_path: PathBuf::from(
                std::env::var("VELVT_DB_PATH").unwrap_or_else(|_| "velvt.sqlite3".to_owned()),
            ),
            upload_batch_max_events: parse_env("VELVT_UPLOAD_BATCH_MAX_EVENTS", 50)?,
            upload_interval_secs: parse_env("VELVT_UPLOAD_INTERVAL_SECS", 60)?,
            ipc_max_errors,
            log_level: std::env::var("VELVT_LOG_LEVEL").unwrap_or_else(|_| "info".to_owned()),
        })
    }
}

fn parse_env<T>(name: &str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
{
    match std::env::var(name) {
        Ok(value) => value.parse().map_err(|_| ConfigError::Invalid),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid),
    }
}

fn expand_home(path: &str) -> Result<PathBuf, ConfigError> {
    let Some(relative) = path.strip_prefix("~/") else {
        return Ok(PathBuf::from(path));
    };
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or(ConfigError::Invalid)?;
    Ok(PathBuf::from(home).join(relative))
}

/// Errors produced while loading service configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required setting is missing or invalid.
    #[error("invalid service configuration")]
    Invalid,
}
