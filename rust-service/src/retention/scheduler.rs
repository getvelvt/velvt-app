use std::time::Duration;
use tokio::sync::watch;

use super::RetentionTarget;

/// Runs registered `RetentionTarget`s on a fixed interval until shutdown.
///
/// Each cycle calls every target once with a single batched-delete operation.
/// Targets that still have rows to delete will be called again on the next
/// cycle — the scheduler never loops internally within a single cycle.
pub struct RetentionScheduler {
    targets: Vec<Box<dyn RetentionTarget>>,
    interval: Duration,
    shutdown: watch::Receiver<bool>,
}

impl RetentionScheduler {
    pub fn new(interval: Duration, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            targets: Vec::new(),
            interval,
            shutdown,
        }
    }

    /// Registers a retention target.  Targets are called in registration order.
    pub fn add_target(mut self, target: impl RetentionTarget + 'static) -> Self {
        self.targets.push(Box::new(target));
        self
    }

    /// Runs the scheduler loop until the shutdown signal fires.
    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                result = self.shutdown.changed() => {
                    if result.is_err() || *self.shutdown.borrow() {
                        return;
                    }
                    continue;
                }
            }

            if *self.shutdown.borrow() {
                return;
            }

            for target in &self.targets {
                match target.run_cleanup() {
                    Ok(report) if report.deleted > 0 => {
                        tracing::debug!(
                            target = target.name(),
                            deleted = report.deleted,
                            "retention cleanup pass completed"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::error!(
                            target = target.name(),
                            error = %err,
                            error_code = "retention_cleanup_failed",
                            "retention target returned an error; will retry next cycle"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::{CleanupReport, RetentionError, RetentionTarget};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct CountingTarget {
        count: Arc<AtomicUsize>,
    }

    impl RetentionTarget for CountingTarget {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(CleanupReport { deleted: 0 })
        }
    }

    #[tokio::test]
    async fn scheduler_calls_targets_on_each_tick() {
        let count = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let scheduler = RetentionScheduler::new(Duration::from_millis(10), shutdown_rx).add_target(
            CountingTarget {
                count: Arc::clone(&count),
            },
        );

        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(120)).await;
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(
            count.load(Ordering::SeqCst) >= 3,
            "expected at least 3 calls in 120ms with 10ms interval"
        );
    }

    #[tokio::test]
    async fn scheduler_stops_on_shutdown() {
        let count = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let _ = shutdown_tx.send(true);

        let scheduler = RetentionScheduler::new(Duration::from_millis(10), shutdown_rx).add_target(
            CountingTarget {
                count: Arc::clone(&count),
            },
        );

        tokio::time::timeout(Duration::from_millis(200), scheduler.run())
            .await
            .expect("scheduler did not stop after shutdown");
    }

    #[tokio::test]
    async fn new_target_called_without_modifying_scheduler() {
        struct FakeTarget {
            calls: Arc<AtomicUsize>,
        }
        impl RetentionTarget for FakeTarget {
            fn name(&self) -> &'static str {
                "fake"
            }
            fn run_cleanup(&self) -> Result<CleanupReport, RetentionError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(CleanupReport { deleted: 0 })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        // Register a brand-new target type — scheduler core is unchanged.
        let scheduler = RetentionScheduler::new(Duration::from_millis(10), shutdown_rx).add_target(
            FakeTarget {
                calls: Arc::clone(&calls),
            },
        );

        tokio::spawn(async move { scheduler.run().await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(
            calls.load(Ordering::SeqCst) >= 3,
            "fake target must be called each cycle without modifying the scheduler"
        );
    }
}
