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
    /// SQLite database path. `:memory:` selects the test-only in-memory mode.
    pub database_path: PathBuf,
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
    pub upload_batch_event_limit: usize,
    pub upload_flush_interval: Duration,
    pub upload_api_base_url: String,
    pub upload_retry_scan_interval: Duration,
    /// TTL for daily history cache entries.
    pub history_ttl: Duration,
    /// TTL for daily insight cache entries (positive responses).
    pub insight_ttl: Duration,
    /// TTL for negative insight cache entries (404 responses).
    pub insight_negative_ttl: Duration,
    /// Maximum time a cache read may block before being treated as a miss.
    pub cache_read_timeout: Duration,
    /// Minimum interval between proactive fetch scheduler runs.
    pub fetch_interval: Duration,
    /// Path for the velvt-core long-poll insight endpoint.
    pub insight_poll_path: String,
    /// Client-side timeout for a single long-poll request.
    pub insight_poll_timeout: Duration,
    /// Delay before re-polling after a 204 No Content response.
    pub insight_poll_idle_interval: Duration,
    /// Initial retry delay after transient long-poll failures.
    pub insight_poll_initial_backoff: Duration,
    /// Maximum retry delay after repeated long-poll failures.
    pub insight_poll_max_backoff: Duration,
    /// Maximum messages buffered in the IPC push queue while Swift is disconnected.
    pub push_queue_capacity: usize,
    /// Per-message write timeout before a connected Swift client is declared slow.
    pub push_write_timeout: Duration,

    // --- R8 lifecycle ---
    /// How long raw events are kept in `raw_event_buffer` before expiry.
    pub raw_event_ttl: Duration,
    /// How often the retention scheduler runs.
    pub raw_event_expiry_interval: Duration,
    /// Maximum rows deleted in a single retention DAL call (batched delete).
    pub retention_batch_size: usize,
    /// How long sent upload batches are kept before deletion.
    pub sent_batch_retention: Duration,
    /// How long rejected upload batches are kept for audit before deletion.
    pub rejected_batch_audit_period: Duration,
    /// Extra time to keep an expired cache entry before deleting it.
    pub cache_expiry_grace: Duration,
    /// Maximum time to wait for in-flight tasks during graceful shutdown.
    pub shutdown_deadline: Duration,
    /// How long to keep a disconnected client's push queue before releasing it.
    pub reconnect_window: Duration,
}

impl ServiceConfig {
    /// Loads and validates runtime configuration.
    pub fn load() -> Result<Self, ConfigError> {
        let socket_path = match std::env::var("VELVT_IPC_SOCKET_PATH") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => canonical_socket_path()?,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(ConfigError::Detail(
                    "VELVT_IPC_SOCKET_PATH is not valid Unicode",
                ))
            }
        };
        let ipc_max_errors = parse_env("VELVT_IPC_MAX_ERRORS", 3)?;
        if ipc_max_errors == 0 {
            return Err(ConfigError::Invalid);
        }

        let upload_batch_event_limit = parse_env("VELVT_UPLOAD_BATCH_EVENT_LIMIT", 50_usize)?;
        let upload_flush_seconds = parse_env("VELVT_UPLOAD_FLUSH_SECONDS", 60_u64)?;
        let upload_retry_scan_seconds = parse_env("VELVT_UPLOAD_RETRY_SCAN_SECONDS", 5_u64)?;
        if !(25..=100).contains(&upload_batch_event_limit)
            || !(16..=180).contains(&upload_flush_seconds)
            || upload_retry_scan_seconds == 0
        {
            return Err(ConfigError::Invalid);
        }

        let history_ttl_seconds = parse_env("VELVT_HISTORY_TTL_SECONDS", 600_u64)?;
        let insight_ttl_seconds = parse_env("VELVT_INSIGHT_TTL_SECONDS", 1800_u64)?;
        let insight_negative_ttl_seconds =
            parse_env("VELVT_INSIGHT_NEGATIVE_TTL_SECONDS", 300_u64)?;
        let cache_read_timeout_ms = parse_env("VELVT_CACHE_READ_TIMEOUT_MS", 200_u64)?;
        let fetch_interval_seconds = parse_env("VELVT_FETCH_INTERVAL_SECONDS", 600_u64)?;
        let insight_poll_path =
            std::env::var("VELVT_INSIGHT_POLL_PATH").unwrap_or_else(|_| "/v1/insights/poll".into());
        let insight_poll_timeout_seconds = parse_env("VELVT_INSIGHT_POLL_TIMEOUT_SECONDS", 70_u64)?;
        let insight_poll_idle_seconds = parse_env("VELVT_INSIGHT_POLL_IDLE_SECONDS", 1_u64)?;
        let insight_poll_initial_backoff_seconds =
            parse_env("VELVT_INSIGHT_POLL_INITIAL_BACKOFF_SECONDS", 1_u64)?;
        let insight_poll_max_backoff_seconds =
            parse_env("VELVT_INSIGHT_POLL_MAX_BACKOFF_SECONDS", 60_u64)?;
        let push_queue_capacity = parse_env("VELVT_PUSH_QUEUE_CAPACITY", 50_usize)?;
        let push_write_timeout_ms = parse_env("VELVT_PUSH_WRITE_TIMEOUT_MS", 500_u64)?;

        if history_ttl_seconds == 0
            || insight_ttl_seconds == 0
            || insight_negative_ttl_seconds == 0
            || cache_read_timeout_ms == 0
            || fetch_interval_seconds == 0
            || !insight_poll_path.starts_with('/')
            || insight_poll_timeout_seconds == 0
            || insight_poll_idle_seconds == 0
            || insight_poll_initial_backoff_seconds == 0
            || insight_poll_max_backoff_seconds < insight_poll_initial_backoff_seconds
            || push_queue_capacity == 0
            || push_write_timeout_ms == 0
        {
            return Err(ConfigError::Invalid);
        }

        // 7 days, not 72 hours: the local daily-activity chart reads this same
        // table for a 7-day window, so a shorter TTL made days 4-7 permanently
        // empty on every real install. This is still the tightest retention in
        // PRIVACY.md — the same safe events live 30 days in `upload_batch` —
        // and the rows are local-only, abstracted metadata.
        let raw_event_ttl_hours = parse_env("VELVT_RAW_EVENT_TTL_HOURS", 168_u64)?;
        let raw_event_expiry_interval_minutes =
            parse_env("VELVT_RAW_EVENT_EXPIRY_INTERVAL_MINUTES", 30_u64)?;
        let retention_batch_size = parse_env("VELVT_RETENTION_BATCH_SIZE", 500_usize)?;
        let sent_batch_retention_days = parse_env("VELVT_SENT_BATCH_RETENTION_DAYS", 30_u64)?;
        let rejected_batch_audit_days = parse_env("VELVT_REJECTED_BATCH_AUDIT_DAYS", 7_u64)?;
        let cache_expiry_grace_seconds = parse_env("VELVT_CACHE_EXPIRY_GRACE_SECONDS", 3600_u64)?;
        let shutdown_deadline_seconds = parse_env("VELVT_SHUTDOWN_DEADLINE_SECONDS", 10_u64)?;
        let reconnect_window_seconds = parse_env("VELVT_RECONNECT_WINDOW_SECONDS", 30_u64)?;

        if raw_event_ttl_hours == 0
            || raw_event_expiry_interval_minutes == 0
            || retention_batch_size == 0
            || sent_batch_retention_days == 0
            || rejected_batch_audit_days == 0
            || shutdown_deadline_seconds == 0
            || reconnect_window_seconds == 0
        {
            return Err(ConfigError::Invalid);
        }

        let abstraction_taxonomy_path = taxonomy_path()?;
        let abstraction_model_path = artifact_path(
            "VELVT_ABSTRACTION_MODEL_PATH",
            &abstraction_taxonomy_path,
            "abstraction-model.onnx",
        )?;
        let abstraction_centroids_path = artifact_path(
            "VELVT_ABSTRACTION_CENTROIDS_PATH",
            &abstraction_taxonomy_path,
            "abstraction-prototypes.bin",
        )?;

        Ok(Self {
            socket_path: expand_home(&socket_path)?,
            database_path: database_path()?,
            protocol_version: PROTOCOL_VERSION,
            ipc_max_errors,
            log_level: std::env::var("VELVT_LOG_LEVEL").unwrap_or_else(|_| "info".to_owned()),
            abstraction_taxonomy_path,
            abstraction_model_path,
            abstraction_centroids_path,
            abstraction_inference_timeout: Duration::from_millis(parse_env(
                "VELVT_ABSTRACTION_INFERENCE_TIMEOUT_MS",
                20_u64,
            )?),
            abstraction_similarity_threshold: parse_threshold()?,
            upload_batch_event_limit,
            upload_flush_interval: Duration::from_secs(upload_flush_seconds),
            upload_api_base_url: {
                const URL: &str = env!("VELVT_API_BASE_URL_COMPILED");
                assert!(
                    !URL.is_empty(),
                    "VELVT_API_BASE_URL_COMPILED must not be empty"
                );
                match std::env::var("VELVT_API_BASE_URL") {
                    Ok(value) if !value.trim().is_empty() => value,
                    Ok(_) | Err(std::env::VarError::NotPresent) => URL.to_owned(),
                    Err(std::env::VarError::NotUnicode(_)) => return Err(ConfigError::Invalid),
                }
            },
            upload_retry_scan_interval: Duration::from_secs(upload_retry_scan_seconds),
            history_ttl: Duration::from_secs(history_ttl_seconds),
            insight_ttl: Duration::from_secs(insight_ttl_seconds),
            insight_negative_ttl: Duration::from_secs(insight_negative_ttl_seconds),
            cache_read_timeout: Duration::from_millis(cache_read_timeout_ms),
            fetch_interval: Duration::from_secs(fetch_interval_seconds),
            insight_poll_path,
            insight_poll_timeout: Duration::from_secs(insight_poll_timeout_seconds),
            insight_poll_idle_interval: Duration::from_secs(insight_poll_idle_seconds),
            insight_poll_initial_backoff: Duration::from_secs(insight_poll_initial_backoff_seconds),
            insight_poll_max_backoff: Duration::from_secs(insight_poll_max_backoff_seconds),
            push_queue_capacity,
            push_write_timeout: Duration::from_millis(push_write_timeout_ms),
            raw_event_ttl: Duration::from_secs(raw_event_ttl_hours * 3600),
            raw_event_expiry_interval: Duration::from_secs(raw_event_expiry_interval_minutes * 60),
            retention_batch_size,
            sent_batch_retention: Duration::from_secs(sent_batch_retention_days * 86400),
            rejected_batch_audit_period: Duration::from_secs(rejected_batch_audit_days * 86400),
            cache_expiry_grace: Duration::from_secs(cache_expiry_grace_seconds),
            shutdown_deadline: Duration::from_secs(shutdown_deadline_seconds),
            reconnect_window: Duration::from_secs(reconnect_window_seconds),
        })
    }
}

fn database_path() -> Result<PathBuf, ConfigError> {
    match std::env::var("VELVT_DATABASE_PATH") {
        Ok(value) => expand_home(&value),
        Err(std::env::VarError::NotPresent) => expand_home("~/.velvt/velvt-service.sqlite3"),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid),
    }
}

fn optional_path(name: &str) -> Result<Option<PathBuf>, ConfigError> {
    match std::env::var(name) {
        Ok(value) => expand_home(&value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid),
    }
}

fn artifact_path(
    environment_name: &str,
    taxonomy_path: &std::path::Path,
    bundled_name: &str,
) -> Result<Option<PathBuf>, ConfigError> {
    if let Some(configured) = optional_path(environment_name)? {
        return Ok(Some(configured));
    }
    let Some(parent) = taxonomy_path.parent() else {
        return Ok(None);
    };
    let bundled = parent.join(bundled_name);
    Ok(bundled.is_file().then_some(bundled))
}

fn parse_threshold() -> Result<f32, ConfigError> {
    let threshold = parse_env("VELVT_ABSTRACTION_SIMILARITY_THRESHOLD", 0.72_f32)?;
    if (0.0..=1.0).contains(&threshold) {
        Ok(threshold)
    } else {
        Err(ConfigError::Invalid)
    }
}

const TAXONOMY_FILE_NAME: &str = "abstraction-taxonomy-mvp-1.json";

fn taxonomy_path() -> Result<PathBuf, ConfigError> {
    match std::env::var("VELVT_ABSTRACTION_TAXONOMY_PATH") {
        Ok(value) => expand_home(&value),
        Err(std::env::VarError::NotPresent) => Ok(default_taxonomy_path()),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::Detail(
            "VELVT_ABSTRACTION_TAXONOMY_PATH is not valid Unicode",
        )),
    }
}

/// Prefers the copy shipped beside the executable over the source tree.
///
/// Inside `Velvt.app` the helper and the taxonomy are siblings in
/// `Contents/Resources`, so a distributed build resolves this without the
/// launcher having to inject an environment variable. `CARGO_MANIFEST_DIR`
/// remains only as a `cargo run` convenience and must never be the path a
/// shipped binary depends on — it points at the build machine's checkout.
fn default_taxonomy_path() -> PathBuf {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let beside_executable = directory.join(TAXONOMY_FILE_NAME);
            if beside_executable.is_file() {
                return beside_executable;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join(TAXONOMY_FILE_NAME)
}

/// The canonical socket path, embedded at compile time from
/// `proto/ipc_socket_path`.
///
/// This has to be `include_str!` and not a runtime read. The shipped binary
/// lives in `Velvt.app` on machines with no checkout, so resolving
/// `CARGO_MANIFEST_DIR` at runtime pointed at the build machine's source
/// tree: `ServiceConfig::load()` failed on every Mac except the one that
/// produced the build, and the service exited before it could log why.
/// Embedding keeps `proto/ipc_socket_path` the single source of truth
/// without making a distributed binary depend on that file existing.
const CANONICAL_SOCKET_PATH: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../proto/ipc_socket_path"
));

fn canonical_socket_path() -> Result<String, ConfigError> {
    let path = CANONICAL_SOCKET_PATH.trim();
    if path.is_empty() {
        return Err(ConfigError::Detail("proto/ipc_socket_path is empty"));
    }
    Ok(path.to_owned())
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
        .ok_or(ConfigError::Detail("neither HOME nor USERPROFILE is set"))?;
    Ok(PathBuf::from(home).join(relative))
}

/// Errors produced while loading service configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required setting is missing or invalid.
    #[error("invalid service configuration")]
    Invalid,
    /// A required setting is missing or invalid, naming the offending setting.
    ///
    /// Load failures are otherwise invisible: `main` returns before tracing is
    /// initialised, because the log filter comes from the config that failed.
    #[error("invalid service configuration: {0}")]
    Detail(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn compile_time_api_url_is_non_empty() {
        let url = env!("VELVT_API_BASE_URL_COMPILED");
        assert!(
            !url.is_empty(),
            "VELVT_API_BASE_URL_COMPILED must not be empty"
        );
    }

    #[test]
    fn compile_time_api_url_is_not_stale_default() {
        // Guard against an accidental revert to the old runtime default that
        // indicated a missing build-env configuration.
        let url = env!("VELVT_API_BASE_URL_COMPILED");
        assert_ne!(
            url, "https://api.velvt.test",
            "VELVT_API_BASE_URL_COMPILED must not be the old misconfiguration sentinel"
        );
    }

    #[test]
    fn canonical_socket_path_is_embedded_not_read_from_disk() {
        // Regression: this used to `read_to_string` a path derived from
        // CARGO_MANIFEST_DIR at runtime, so a shipped binary depended on the
        // build machine's checkout still existing. Every other Mac failed
        // ServiceConfig::load() and the service exited silently, which took
        // sign-in and sign-up down with it. The value must resolve with no
        // filesystem access at all.
        let path = canonical_socket_path().expect("embedded socket path must resolve");
        assert!(!path.is_empty(), "embedded socket path must not be empty");
        assert!(
            !path.contains(env!("CARGO_MANIFEST_DIR")),
            "socket path must not leak the build machine's source tree: {path}"
        );
    }

    #[test]
    fn default_taxonomy_path_prefers_sibling_of_executable() {
        // Inside Velvt.app the helper and the taxonomy are siblings in
        // Contents/Resources. The current test binary has no such sibling, so
        // this exercises the documented fallback ordering rather than asserting
        // a bundle layout the test harness does not have.
        let path = default_taxonomy_path();
        assert!(
            path.ends_with(TAXONOMY_FILE_NAME),
            "taxonomy fallback must resolve to {TAXONOMY_FILE_NAME}, got {}",
            path.display()
        );
    }

    #[test]
    fn compile_time_apns_env_is_valid() {
        let apns_env = env!("VELVT_APNS_ENV_COMPILED");
        assert!(
            apns_env == "development" || apns_env == "production",
            "VELVT_APNS_ENV_COMPILED must be 'development' or 'production', got '{apns_env}'"
        );
    }

    #[test]
    fn service_config_loads_without_api_base_url_env_var() {
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
        // Ensure ServiceConfig::load() succeeds even when VELVT_API_BASE_URL is
        // absent from the runtime environment — the compile-time constant applies.
        std::env::remove_var("VELVT_API_BASE_URL");
        let config = ServiceConfig::load();
        assert!(
            config.is_ok(),
            "ServiceConfig::load() must succeed without VELVT_API_BASE_URL in env"
        );
        let url = config.unwrap().upload_api_base_url;
        assert!(!url.is_empty(), "upload_api_base_url must not be empty");
    }

    #[test]
    fn service_config_uses_expected_compiled_api_base_url_when_requested() {
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
        let Ok(expected_url) = std::env::var("VELVT_EXPECT_COMPILED_API_BASE_URL") else {
            return;
        };

        std::env::remove_var("VELVT_API_BASE_URL");
        let config = ServiceConfig::load().unwrap();

        assert_eq!(config.upload_api_base_url, expected_url);
    }

    #[test]
    fn service_config_uses_runtime_api_base_url_override() {
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
        std::env::set_var("VELVT_API_BASE_URL", "http://localhost:8000");
        let config = ServiceConfig::load().unwrap();
        std::env::remove_var("VELVT_API_BASE_URL");

        assert_eq!(config.upload_api_base_url, "http://localhost:8000");
    }

    #[test]
    fn raw_event_retention_covers_daily_activity() {
        // The local daily-activity chart reads `raw_event_buffer`. When the TTL
        // is shorter than the rendered window, the oldest days are not "no
        // activity" — they are deleted evidence drawn as zeroes, which no test
        // could see because retention never runs in the persistence fixtures.
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
        std::env::remove_var("VELVT_RAW_EVENT_TTL_HOURS");
        let config = ServiceConfig::load().unwrap();

        let window = std::time::Duration::from_secs(
            crate::dashboard::DAILY_ACTIVITY_DAYS as u64 * 24 * 3600,
        );
        assert!(
            config.raw_event_ttl >= window,
            "raw-event TTL {:?} must cover the {}-day daily-activity window",
            config.raw_event_ttl,
            crate::dashboard::DAILY_ACTIVITY_DAYS
        );
    }

    #[test]
    fn bundled_classifier_artifact_is_discovered_beside_taxonomy() {
        let directory =
            std::env::temp_dir().join(format!("velvt-artifact-discovery-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let taxonomy = directory.join("taxonomy.json");
        let model = directory.join("abstraction-model.onnx");
        std::fs::write(&taxonomy, b"{}").unwrap();
        std::fs::write(&model, b"model").unwrap();

        assert_eq!(
            artifact_path(
                "VELVT_TEST_MISSING_ARTIFACT_PATH",
                &taxonomy,
                "abstraction-model.onnx"
            )
            .unwrap(),
            Some(model.clone())
        );

        std::fs::remove_file(model).unwrap();
        std::fs::remove_file(taxonomy).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
