use std::{collections::HashMap, sync::Arc, time::Duration};

/// The longest a `Retry-After` puts the upload host off: one hour, velvt-core's
/// longest rate-limit window (ingestion's is a minute). The header is honoured
/// above the backoff's own cap, but a 429 from a proxy or edge in front of the
/// API with a day or a far HTTP date would otherwise park the whole queue that
/// long, across restarts, since the wait is saved with the host's backoff.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

pub struct HostBackoff {
    base: Duration,
    cap: Duration,
    attempts: HashMap<String, u32>,
    jitter: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl HostBackoff {
    pub fn production(base: Duration, cap: Duration) -> Self {
        Self::new(base, cap, || {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            0.9 + f64::from(nanos % 2001) / 10_000.0
        })
    }

    pub fn new(
        base: Duration,
        cap: Duration,
        jitter: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            base,
            cap,
            attempts: HashMap::new(),
            jitter: Arc::new(jitter),
        }
    }

    pub fn next_delay(&mut self, host: &str, retry_after: Option<&str>) -> Duration {
        if let Some(delay) = retry_after.and_then(parse_retry_after) {
            return delay.min(MAX_RETRY_AFTER);
        }
        let attempt = self.attempts.entry(host.to_owned()).or_default();
        let multiplier = 2_u32.saturating_pow(*attempt);
        *attempt = attempt.saturating_add(1);
        let seconds = self
            .base
            .as_secs()
            .saturating_mul(multiplier as u64)
            .min(self.cap.as_secs());
        let jitter = (self.jitter)().clamp(0.9, 1.1);
        Duration::from_secs_f64((seconds as f64 * jitter).min(self.cap.as_secs_f64()))
    }

    pub fn reset(&mut self, host: &str) {
        self.attempts.remove(host);
    }

    pub fn set_attempt(&mut self, host: &str, attempt: u32) {
        self.attempts.insert(host.to_owned(), attempt);
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    crate::auth::retry_after_delay(value)
}
