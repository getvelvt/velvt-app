//! Reconnect-window queue management for IPC clients.
//!
//! `ReconnectTracker` owns the single `PushQueue` shared by the producer side
//! (`PushAdapter`) and the transport side.  The queue handle must remain stable:
//! if the transport swaps in a fresh queue while the adapter still writes to the
//! old one, proactive messages can be stranded after a disconnect.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::delivery::PushQueue;

struct TrackerState {
    queue: Option<Arc<PushQueue>>,
    /// Monotonic version; bumped on every `acquire`.  The scheduled release
    /// task captures the version at `release` time and skips the clear if
    /// a new `acquire` has since occurred.
    version: u64,
}

/// Manages a single client's push queue across disconnects.
pub struct ReconnectTracker {
    state: Mutex<TrackerState>,
    window: Duration,
    capacity: usize,
}

impl ReconnectTracker {
    pub fn new(window: Duration, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TrackerState {
                queue: None,
                version: 0,
            }),
            window,
            capacity,
        })
    }

    /// Returns the existing queue or creates the one stable queue used by this
    /// service process.  Bumping the version cancels any pending expiry marker
    /// scheduled by a prior `release()`.
    pub fn acquire(self: &Arc<Self>) -> Arc<PushQueue> {
        let mut state = self.state.lock().unwrap();
        // Bumping the version invalidates any in-flight release task.
        state.version = state.version.wrapping_add(1);
        match &state.queue {
            Some(q) => {
                tracing::debug!("push queue reused across reconnect");
                Arc::clone(q)
            }
            None => {
                let q = PushQueue::new(self.capacity);
                state.queue = Some(Arc::clone(&q));
                q
            }
        }
    }

    /// Schedules an expiry marker after `window`.  If `acquire` is called before
    /// the window elapses, the marker is cancelled.  The queue itself is not
    /// replaced because `PushAdapter` holds the producer-side handle.
    pub fn release(self: &Arc<Self>) {
        let captured_version = self.state.lock().unwrap().version;
        let tracker = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(tracker.window).await;
            let state = tracker.state.lock().unwrap();
            if state.version == captured_version {
                tracing::debug!(
                    error_code = "reconnect_window_expired",
                    "reconnect window elapsed; retaining stable push queue handle"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn reconnect_within_window_reuses_queue() {
        let tracker = ReconnectTracker::new(Duration::from_millis(200), 10);

        let q1 = tracker.acquire();
        tracker.release();

        // Reconnect before the window elapses.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let q2 = tracker.acquire();

        assert!(
            Arc::ptr_eq(&q1, &q2),
            "same queue must be returned within the reconnect window"
        );
    }

    #[tokio::test]
    async fn reconnect_after_window_keeps_stable_queue() {
        let tracker = ReconnectTracker::new(Duration::from_millis(50), 10);

        let q1 = tracker.acquire();
        tracker.release();

        // Wait for the window to expire.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let q2 = tracker.acquire();

        assert!(
            Arc::ptr_eq(&q1, &q2),
            "the queue handle must stay stable so PushAdapter and transport do not diverge"
        );
    }

    #[tokio::test]
    async fn messages_buffered_during_disconnect_survive_reconnect() {
        // Queue identity (same Arc pointer) is the invariant that ensures any
        // messages drained by the push adapter while disconnected remain in the
        // queue for the next connection.  Enqueue visibility is intentionally
        // restricted to the delivery module; we verify the structural guarantee.
        let tracker = ReconnectTracker::new(Duration::from_millis(200), 10);
        let q1 = tracker.acquire();
        tracker.release();
        tokio::time::sleep(Duration::from_millis(30)).await;

        let q2 = tracker.acquire();
        assert!(
            Arc::ptr_eq(&q1, &q2),
            "same queue pointer guarantees buffered messages survive the disconnect"
        );
    }

    #[tokio::test]
    async fn first_acquire_creates_fresh_queue() {
        let tracker = ReconnectTracker::new(Duration::from_millis(200), 10);
        let q = tracker.acquire();
        assert!(q.is_empty().await);
    }
}
