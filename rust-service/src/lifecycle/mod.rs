//! Service lifecycle coordination.
//!
//! `CancellationToken` distributes a single shutdown signal to every
//! long-running task without a global flag.  Every task that needs to stop
//! receives a `watch::Receiver<bool>` via `subscribe()`; the owner calls
//! `cancel()` once and all subscribers see the change.

use std::sync::Arc;
use tokio::sync::watch;

/// Propagates a shutdown signal to every long-running task.
///
/// Clone-safe: all clones share the same underlying sender so any clone can
/// call `cancel()` and all subscribers are notified.
#[derive(Clone)]
pub struct CancellationToken {
    sender: Arc<watch::Sender<bool>>,
}

impl CancellationToken {
    pub fn new() -> Self {
        let (sender, _) = watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }

    /// Fires the cancellation signal.  Idempotent.
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    /// Returns `true` if `cancel` has been called.
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    /// Returns a receiver that reflects this token's state.  Pass one to each
    /// long-running task; the task checks `*rx.borrow()` or `rx.changed()`.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.sender.subscribe()
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationToken;

    #[tokio::test]
    async fn cancel_is_seen_by_all_subscribers() {
        let token = CancellationToken::new();
        let mut rx1 = token.subscribe();
        let mut rx2 = token.subscribe();

        assert!(!token.is_cancelled());
        token.cancel();

        rx1.changed().await.unwrap();
        rx2.changed().await.unwrap();
        assert!(*rx1.borrow());
        assert!(*rx2.borrow());
    }

    #[test]
    fn clone_shares_same_signal() {
        let token = CancellationToken::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_is_idempotent() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancel(); // must not panic
        assert!(token.is_cancelled());
    }
}
