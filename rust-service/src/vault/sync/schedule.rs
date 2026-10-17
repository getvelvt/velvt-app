//! When the engine runs a pass (design §10.4 "Triggers").
//!
//! - every 10 minutes;
//! - 5 s after the last local change (a rename, a routing switch, a rule the
//!   mirror records);
//! - on wake, when a key arrives in the Keychain, and when the app asks for
//!   the device list, if the last pass is more than a minute old.
//!
//! Nothing spawns [`run`] in this build; the service wires it with A4's
//! protocol, the one after 38.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Notify};

use super::{SyncEngine, POLL_INTERVAL_MS, STATE_REQUEST_MIN_INTERVAL_MS, WRITE_DEBOUNCE_MS};

/// What wakes the loop.
#[derive(Default)]
pub struct SyncTriggers {
    changed: Notify,
    asked: Notify,
}

impl SyncTriggers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A local change: written 5 s after the last one.
    pub fn local_change(&self) {
        self.changed.notify_one();
    }

    /// Wake, a key arriving, or the app asking: a pass if the last is more
    /// than a minute old.
    pub fn nudge(&self) {
        self.asked.notify_one();
    }
}

fn millis(value: i64) -> Duration {
    Duration::from_millis(u64::try_from(value).unwrap_or_default())
}

/// The three waits, §10.4's unless a test shortens them.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub poll: Duration,
    pub debounce: Duration,
    pub min_request_interval: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            poll: millis(POLL_INTERVAL_MS),
            debounce: millis(WRITE_DEBOUNCE_MS),
            min_request_interval: millis(STATE_REQUEST_MIN_INTERVAL_MS),
        }
    }
}

/// Runs passes until `shutdown` turns true.
pub async fn run(
    engine: Arc<SyncEngine>,
    triggers: Arc<SyncTriggers>,
    timings: Timings,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut last_pass = tokio::time::Instant::now();
    engine.sync_now().await;
    loop {
        let poll = tokio::time::sleep_until(last_pass + timings.poll);
        tokio::select! {
            _ = poll => {}
            _ = triggers.changed.notified() => {
                // Debounce: every change inside the window restarts it.
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(timings.debounce) => break,
                        _ = triggers.changed.notified() => continue,
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                return;
                            }
                        }
                    }
                }
            }
            _ = triggers.asked.notified() => {
                if last_pass.elapsed() < timings.min_request_interval {
                    continue;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
        }
        last_pass = tokio::time::Instant::now();
        engine.sync_now().await;
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::watch;

    use super::{run, SyncTriggers, Timings};
    use crate::auth::{AuthError, HttpClient, HttpRequest, HttpResponse};
    use crate::persistence::SqlitePersistence;
    use crate::vault::crypto::UserId;
    use crate::vault::doc::DeviceName;
    use crate::vault::sync::host::FixedHostIdentity;
    use crate::vault::sync::keys::MemoryKeyStore;
    use crate::vault::sync::{NoBlocks, SyncClock, SyncDeps, SyncEngine, SyncRouting};

    /// Counts `GET /v1/vault` and answers that the vault is switched off.
    #[derive(Default)]
    struct Counting(AtomicUsize);

    impl HttpClient for Counting {
        fn send<'a>(
            &'a self,
            request: HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
            if request.path == "/v1/vault" {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            Box::pin(async move {
                Ok(HttpResponse {
                    status: 404,
                    error_code: Some("feature_disabled".into()),
                    tokens: None,
                    retry_after: None,
                    message: None,
                    raw_body: None,
                    user_id: None,
                    device_id: None,
                })
            })
        }
    }

    struct Fixed;

    impl SyncClock for Fixed {
        fn now_ms(&self) -> i64 {
            1_760_486_400_000
        }
    }

    /// Short enough for a test, with room for a loaded machine.
    fn timings() -> Timings {
        Timings {
            poll: Duration::from_secs(60),
            debounce: Duration::from_millis(200),
            min_request_interval: Duration::from_millis(1_500),
        }
    }

    #[tokio::test]
    async fn passes_run_on_the_poll_after_the_last_change_and_on_a_late_request() {
        let http = Arc::new(Counting::default());
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let engine = Arc::new(SyncEngine::new(SyncDeps {
            http: Arc::clone(&http) as Arc<dyn HttpClient>,
            keys: Arc::new(MemoryKeyStore::new()),
            repo: persistence.vault_state_repo(),
            host: Arc::new(FixedHostIdentity(Some([1; 16]))),
            clock: Arc::new(Fixed),
            blocks: Arc::new(NoBlocks),
            routing: Arc::new(SyncRouting::default()),
            conflict_jitter: Duration::ZERO,
        }));
        let user = UserId::new("8f14e45f-ceea-467a-9575-1f2ab6c9d0e1").unwrap();
        engine.session_started(user, None).await;
        engine.turn_on(DeviceName::new("Mac")).await;
        let passes = || http.0.load(Ordering::SeqCst);
        assert_eq!(passes(), 1);

        let triggers = SyncTriggers::new();
        let (stop, shutdown) = watch::channel(false);
        let task = tokio::spawn(run(
            Arc::clone(&engine),
            Arc::clone(&triggers),
            timings(),
            shutdown,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(passes(), 2, "a pass at start");

        triggers.nudge();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(passes(), 2, "a request soon after a pass sends nothing");

        triggers.local_change();
        tokio::time::sleep(Duration::from_millis(120)).await;
        triggers.local_change();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(passes(), 2, "each change restarts the wait");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(passes(), 3);

        tokio::time::sleep(Duration::from_millis(1_700)).await;
        triggers.nudge();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(passes(), 4, "a request long after a pass passes");

        stop.send(true).unwrap();
        task.await.unwrap();
    }
}
