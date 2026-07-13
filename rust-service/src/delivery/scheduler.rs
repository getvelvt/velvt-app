//! Periodic fetch scheduler.
//!
//! `FetchScheduler` wakes on a short tick interval and calls `fetch_all` on
//! the fetch service whenever the minimum fetch interval has elapsed AND the
//! device is in an authenticated state.  When the device is revoked or the
//! user is unauthenticated, the scheduler parks silently until auth recovers.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::watch;

use crate::auth::AuthState;

use super::fetch::Fetchable;

pub struct FetchScheduler {
    fetch_service: Arc<dyn Fetchable>,
    /// Number of history days to request on each proactive refresh.
    days: u8,
    /// Minimum wall-clock time between actual `fetch_all` calls.
    min_fetch_interval: Duration,
    /// How often the inner loop wakes to check auth state and the guard.
    tick_interval: Duration,
    auth_state: watch::Receiver<AuthState>,
    shutdown: watch::Receiver<bool>,
}

impl FetchScheduler {
    pub fn new(
        fetch_service: Arc<dyn Fetchable>,
        days: u8,
        min_fetch_interval: Duration,
        auth_state: watch::Receiver<AuthState>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        // Wake up frequently enough that auth-state changes are noticed quickly
        // without hammering the timer subsystem.
        let tick_interval = Duration::from_secs(30).min(min_fetch_interval / 2);
        Self {
            fetch_service,
            days,
            min_fetch_interval,
            tick_interval,
            auth_state,
            shutdown,
        }
    }

    /// Runs the scheduler loop until the shutdown signal fires.
    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.tick_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // On-demand Swift requests fetch initial history/insight after connect.
        // Proactive background refresh waits one interval to avoid duplicating
        // that startup traffic.
        let mut last_fetch: Option<Instant> = Some(Instant::now());

        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                result = self.shutdown.changed() => {
                    if result.is_err() || *self.shutdown.borrow() {
                        return;
                    }
                    continue;
                }
                result = self.auth_state.changed() => {
                    if result.is_err() {
                        return;
                    }
                    // Auth state changed; let the next tick decide whether to fetch.
                    continue;
                }
            }

            if *self.shutdown.borrow() {
                return;
            }

            let auth = self.auth_state.borrow().clone();
            if !matches!(auth, AuthState::Authenticated { .. }) {
                tracing::debug!(
                    error_code = "fetch_scheduler_paused",
                    "fetch skipped: device not authenticated"
                );
                continue;
            }

            // Minimum-interval guard: do not wake more than once per interval.
            let overdue = last_fetch.is_none_or(|t| t.elapsed() >= self.min_fetch_interval);
            if !overdue {
                continue;
            }

            last_fetch = Some(Instant::now());
            let service = Arc::clone(&self.fetch_service);
            let days = self.days;
            tokio::spawn(async move {
                service.fetch_all(days).await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct FakeFetchable {
        call_count: Arc<AtomicUsize>,
    }

    impl FakeFetchable {
        fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
            let counter = Arc::new(AtomicUsize::new(0));
            (
                Arc::new(Self {
                    call_count: Arc::clone(&counter),
                }),
                counter,
            )
        }
    }

    impl Fetchable for FakeFetchable {
        fn fetch_all<'a>(
            &'a self,
            _days: u8,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    fn authenticated_state() -> watch::Receiver<AuthState> {
        let (tx, rx) = watch::channel(AuthState::Authenticated {
            device_id: "test-device".into(),
        });
        // Keep the sender alive for the test duration.
        std::mem::forget(tx);
        rx
    }

    fn unauthenticated_state() -> watch::Receiver<AuthState> {
        let (tx, rx) = watch::channel(AuthState::Unauthenticated);
        std::mem::forget(tx);
        rx
    }

    fn revoked_state() -> watch::Receiver<AuthState> {
        let (tx, rx) = watch::channel(AuthState::DeviceRevoked);
        std::mem::forget(tx);
        rx
    }

    #[tokio::test]
    async fn scheduler_calls_fetch_when_authenticated() {
        let (service, counter) = FakeFetchable::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let scheduler = FetchScheduler::new(
            service,
            7,
            Duration::from_millis(10), // very short interval for test
            authenticated_state(),
            shutdown_rx,
        );

        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(
            counter.load(Ordering::SeqCst) >= 1,
            "scheduler should have called fetch at least once when authenticated"
        );
    }

    #[tokio::test]
    async fn scheduler_waits_for_interval_before_first_proactive_fetch() {
        let (service, counter) = FakeFetchable::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let scheduler = FetchScheduler::new(
            service,
            7,
            Duration::from_millis(200),
            authenticated_state(),
            shutdown_rx,
        );

        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = shutdown_tx.send(true);

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "scheduler should not duplicate startup on-demand history/insight requests"
        );
    }

    #[tokio::test]
    async fn scheduler_pauses_when_device_revoked() {
        let (service, counter) = FakeFetchable::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let scheduler = FetchScheduler::new(
            service,
            7,
            Duration::from_millis(10),
            revoked_state(),
            shutdown_rx,
        );

        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "scheduler must not fetch when device is revoked"
        );
    }

    #[tokio::test]
    async fn scheduler_pauses_when_unauthenticated() {
        let (service, counter) = FakeFetchable::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let scheduler = FetchScheduler::new(
            service,
            7,
            Duration::from_millis(10),
            unauthenticated_state(),
            shutdown_rx,
        );

        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "scheduler must not fetch when unauthenticated"
        );
    }

    #[tokio::test]
    async fn scheduler_stops_on_shutdown_signal() {
        let (service, counter) = FakeFetchable::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        // Immediate shutdown.
        let _ = shutdown_tx.send(true);

        let scheduler = FetchScheduler::new(
            service,
            7,
            Duration::from_millis(10),
            authenticated_state(),
            shutdown_rx,
        );

        // Should return promptly.
        tokio::time::timeout(Duration::from_millis(500), scheduler.run())
            .await
            .expect("scheduler did not stop within timeout");

        // May or may not have fired once depending on tick timing; just ensure
        // it did not loop indefinitely.
        let _ = counter.load(Ordering::SeqCst);
    }

    #[tokio::test]
    async fn scheduler_minimum_interval_guard_limits_fetch_rate() {
        let (service, counter) = FakeFetchable::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let scheduler = FetchScheduler::new(
            service,
            7,
            Duration::from_millis(200), // min interval 200ms
            authenticated_state(),
            shutdown_rx,
        );

        // Run for 250ms — with a 200ms guard, we expect exactly 1 or 2 fetches,
        // not one per 15ms tick.
        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(250)).await;
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let calls = counter.load(Ordering::SeqCst);
        assert!(
            calls <= 2,
            "guard should prevent more than 2 fetches in 250ms with 200ms interval, got {calls}"
        );
    }
}
