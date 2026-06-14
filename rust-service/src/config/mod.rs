//! Typed runtime configuration.
//!
//! This module owns validated service settings. It does not own secret
//! storage, service lifecycle, or business logic.

use std::path::PathBuf;
use std::time::Duration;
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
    /// Number of malformed messages allowed before closing a connection.
    pub ipc_max_errors: usize,
    /// Structured tracing filter configured for the service.
    pub log_level: String,
    /// Versioned abstraction taxonomy loaded before the service starts.
    pub abstraction_taxonomy_path: PathBuf,
    /// Optional local ONNX model path. Absence disables Tier 2.
    pub abstraction_model_path: Option<PathBuf>,
    /// Companion centroid file required when the ONNX model is configured.
    pub abstraction_centroids_path: Option<PathBuf>,
    /// Maximum Tier 2 inference time.
    pub abstraction_inference_timeout: Duration,
    /// Minimum cosine similarity accepted by Tier 2.
    pub abstraction_similarity_threshold: f32,
}

impl ServiceConfig {
    /// Loads and validates runtime configuration.
    pub fn load() -> Result<Self, ConfigError> {
        let socket_path = match std::env::var("VELVT_IPC_SOCKET_PATH") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => canonical_socket_path()?,
            Err(std::env::VarError::NotUnicode(_)) => return Err(ConfigError::Invalid),
        };
        let ipc_max_errors = parse_env("VELVT_IPC_MAX_ERRORS", 3)?;
        if ipc_max_errors == 0 {
            return Err(ConfigError::Invalid);
        }

        Ok(Self {
            socket_path: expand_home(&socket_path)?,
            protocol_version: PROTOCOL_VERSION,
            ipc_max_errors,
            log_level: std::env::var("VELVT_LOG_LEVEL").unwrap_or_else(|_| "info".to_owned()),
            abstraction_taxonomy_path: taxonomy_path()?,
            abstraction_model_path: optional_path("VELVT_ABSTRACTION_MODEL_PATH")?,
            abstraction_centroids_path: optional_path("VELVT_ABSTRACTION_CENTROIDS_PATH")?,
            abstraction_inference_timeout: Duration::from_millis(parse_env(
                "VELVT_ABSTRACTION_INFERENCE_TIMEOUT_MS",
                20_u64,
            )?),
            abstraction_similarity_threshold: parse_threshold()?,
        })
    }
}

fn optional_path(name: &str) -> Result<Option<PathBuf>, ConfigError> {
    match std::env::var(name) {
        Ok(value) => expand_home(&value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid),
    }
}

fn parse_threshold() -> Result<f32, ConfigError> {
    let threshold = parse_env("VELVT_ABSTRACTION_SIMILARITY_THRESHOLD", 0.72_f32)?;
    if (0.0..=1.0).contains(&threshold) {
        Ok(threshold)
    } else {
        Err(ConfigError::Invalid)
    }
}

fn taxonomy_path() -> Result<PathBuf, ConfigError> {
    match std::env::var("VELVT_ABSTRACTION_TAXONOMY_PATH") {
        Ok(value) => expand_home(&value),
        Err(std::env::VarError::NotPresent) => Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/abstraction-taxonomy-mvp-1.json")),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid),
    }
}

fn canonical_socket_path() -> Result<String, ConfigError> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../proto/ipc_socket_path");
    std::fs::read_to_string(path)
        .map(|value| value.trim().to_owned())
        .map_err(|_| ConfigError::Invalid)
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
