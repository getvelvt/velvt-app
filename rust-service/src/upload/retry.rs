use chrono::{DateTime, Utc};
use std::{collections::HashMap, sync::Arc, time::Duration};

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
            return delay;
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
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(
        retry_at
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or_default(),
    )
}
