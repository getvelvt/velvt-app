//! Typed runtime configuration.
//!
//! This module owns validated service settings. It does not own secret
//! storage, service lifecycle, or business logic.

use std::path::PathBuf;

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
}

impl ServiceConfig {
    /// Loads and validates runtime configuration.
    pub fn load() -> Result<Self, ConfigError> {
        todo!("load service configuration")
    }
}

/// Errors produced while loading service configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required setting is missing or invalid.
    #[error("invalid service configuration")]
    Invalid,
}
