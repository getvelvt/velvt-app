//! Concrete IPC transports isolated from message routing and connection state.

#![allow(async_fn_in_trait)]

use super::IpcError;

#[cfg(unix)]
const ACCEPT_RETRY_POLICY: AcceptRetryPolicy = AcceptRetryPolicy {
    initial_delay: std::time::Duration::from_millis(50),
    max_delay: std::time::Duration::from_secs(1),
};

#[cfg(unix)]
#[derive(Clone, Copy)]
struct AcceptRetryPolicy {
    initial_delay: std::time::Duration,
    max_delay: std::time::Duration,
}

#[cfg(unix)]
trait AcceptSource {
    type Connection;

    async fn accept(&self) -> std::io::Result<Self::Connection>;
}

#[cfg(unix)]
impl AcceptSource for tokio::net::UnixListener {
    type Connection = tokio::net::UnixStream;

    async fn accept(&self) -> std::io::Result<Self::Connection> {
        tokio::net::UnixListener::accept(self)
            .await
            .map(|(stream, _)| stream)
    }
}

/// Runs an IPC accept loop without exposing transport details to handlers.
pub trait IpcTransport {
    /// Accepts clients until the transport is stopped or fails.
    async fn run(&self) -> Result<(), IpcError>;
}

/// True if another process is already listening on `socket_path`.
///
/// `TokioUnixTransport::bind` already refuses to bind over a live listener,
/// but that failure surfaces deep inside a spawned task whose `Result` is
/// never awaited until shutdown — so a second launch used to fail to bind
/// silently and then idle forever as a zombie with no working IPC listener,
/// instead of exiting. Callers should check this *before* doing any other
/// startup work and exit immediately if it's true.
#[cfg(unix)]
pub async fn socket_already_in_use(socket_path: &std::path::Path) -> bool {
    tokio::net::UnixStream::connect(socket_path).await.is_ok()
}

/// Tokio Unix-domain-socket transport for the macOS service.
///
/// Generic over the message router `R` so business handlers can be swapped
/// without touching transport or framing code.  The default is `DefaultRouter`
/// for backward compatibility.
#[cfg(unix)]
pub struct TokioUnixTransport<R = super::DefaultRouter> {
    socket_path: std::path::PathBuf,
    max_errors: usize,
    router: R,
    auth_states: Option<tokio::sync::watch::Receiver<crate::auth::AuthState>>,
    privacy_alerts:
        Option<tokio::sync::broadcast::Sender<velvt_shared_types::PrivacyViolationAlert>>,
    push_queue: Option<std::sync::Arc<crate::delivery::PushQueue>>,
    write_timeout: std::time::Duration,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    reconnect_tracker: Option<std::sync::Arc<super::ReconnectTracker>>,
}

#[cfg(unix)]
impl TokioUnixTransport<super::DefaultRouter> {
    /// Creates a Unix transport with the default router.
    pub fn new(socket_path: std::path::PathBuf, max_errors: usize) -> Self {
        Self {
            socket_path,
            max_errors,
            router: super::DefaultRouter,
            auth_states: None,
            privacy_alerts: None,
            push_queue: None,
            write_timeout: std::time::Duration::from_millis(500),
            shutdown: None,
            reconnect_tracker: None,
        }
    }
}

#[cfg(unix)]
impl<R: super::MessageRouter + Clone + Send + 'static> TokioUnixTransport<R> {
    /// Creates a Unix transport with a custom router.
    pub fn new_with_router(socket_path: std::path::PathBuf, max_errors: usize, router: R) -> Self {
        Self {
            socket_path,
            max_errors,
            router,
            auth_states: None,
            privacy_alerts: None,
            push_queue: None,
            write_timeout: std::time::Duration::from_millis(500),
            shutdown: None,
            reconnect_tracker: None,
        }
    }

    pub fn with_privacy_alerts(
        mut self,
        alerts: tokio::sync::broadcast::Sender<velvt_shared_types::PrivacyViolationAlert>,
    ) -> Self {
        self.privacy_alerts = Some(alerts);
        self
    }

    /// Pushes authentication state changes to every connected Swift client.
    pub fn with_auth_state(
        mut self,
        auth_states: tokio::sync::watch::Receiver<crate::auth::AuthState>,
    ) -> Self {
        self.auth_states = Some(auth_states);
        self
    }

    /// Attaches a fixed push queue; enables proactive delivery to connected clients.
    pub fn with_push_queue(
        mut self,
        queue: std::sync::Arc<crate::delivery::PushQueue>,
        write_timeout: std::time::Duration,
    ) -> Self {
        self.push_queue = Some(queue);
        self.write_timeout = write_timeout;
        self
    }

    /// Attaches a shutdown receiver; the accept loop exits when it fires `true`.
    ///
    /// Connection tasks already in progress receive their own clone of the
    /// receiver and flush the push queue (including any `ShuttingDown` message)
    /// before closing.
    pub fn with_shutdown(mut self, shutdown_rx: tokio::sync::watch::Receiver<bool>) -> Self {
        self.shutdown = Some(shutdown_rx);
        self
    }

    /// Attaches a reconnect tracker.  Each new connection calls `tracker.acquire()`
    /// to get a potentially reused push queue, and `tracker.release()` when it
    /// disconnects.  Takes precedence over any queue set via `with_push_queue`.
    pub fn with_reconnect_tracker(
        mut self,
        tracker: std::sync::Arc<super::ReconnectTracker>,
        write_timeout: std::time::Duration,
    ) -> Self {
        self.reconnect_tracker = Some(tracker);
        self.write_timeout = write_timeout;
        self
    }

    async fn bind(&self) -> Result<tokio::net::UnixListener, IpcError> {
        use std::os::unix::fs::PermissionsExt;

        let parent = self.socket_path.parent().ok_or(IpcError::Transport)?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| IpcError::Transport)?;
        tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| IpcError::Transport)?;

        if tokio::fs::symlink_metadata(&self.socket_path).await.is_ok() {
            if tokio::net::UnixStream::connect(&self.socket_path)
                .await
                .is_ok()
            {
                return Err(IpcError::Transport);
            }
            tokio::fs::remove_file(&self.socket_path)
                .await
                .map_err(|_| IpcError::Transport)?;
        }
        tokio::net::UnixListener::bind(&self.socket_path).map_err(|_| IpcError::Transport)
    }
}

#[cfg(unix)]
impl<R: super::MessageRouter + Clone + Send + 'static> IpcTransport for TokioUnixTransport<R> {
    async fn run(&self) -> Result<(), IpcError> {
        let listener = self.bind().await?;
        let mut join_set = tokio::task::JoinSet::<()>::new();
        let mut accept_shutdown = self.shutdown.clone();

        loop {
            tokio::select! {
                accept = accept_with_retry(
                    &listener,
                    &mut accept_shutdown,
                    ACCEPT_RETRY_POLICY,
                ) => {
                    let Some(stream) = accept else {
                        break;
                    };
                    let max_errors = self.max_errors;
                    let auth_states = self.auth_states.clone();
                    let write_timeout = self.write_timeout;
                    let router = self.router.clone();
                    let conn_shutdown = self.shutdown.clone();

                    if let Some(tracker) = &self.reconnect_tracker {
                        let queue = tracker.acquire();
                        let tracker_clone = std::sync::Arc::clone(tracker);
                        join_set.spawn(async move {
                            let result = if let Some(srx) = conn_shutdown {
                                super::serve_connection_with_push_queue_and_shutdown(
                                    stream, router, max_errors, auth_states,
                                    queue, write_timeout, srx,
                                )
                                .await
                            } else {
                                super::serve_connection_with_push_queue(
                                    stream, router, max_errors, auth_states,
                                    queue, write_timeout,
                                )
                                .await
                            };
                            tracker_clone.release();
                            if let Err(error) = result {
                                tracing::warn!(
                                    error = %error,
                                    "IPC client connection ended with an error"
                                );
                            }
                        });
                    } else {
                        let push_queue = self.push_queue.clone();
                        let privacy_alerts =
                            self.privacy_alerts.as_ref().map(|s| s.subscribe());
                        join_set.spawn(async move {
                            let result = if let Some(pq) = push_queue {
                                if let Some(srx) = conn_shutdown {
                                    super::serve_connection_with_push_queue_and_shutdown(
                                        stream, router, max_errors, auth_states,
                                        pq, write_timeout, srx,
                                    )
                                    .await
                                } else {
                                    super::serve_connection_with_push_queue(
                                        stream, router, max_errors, auth_states,
                                        pq, write_timeout,
                                    )
                                    .await
                                }
                            } else {
                                match (auth_states, privacy_alerts) {
                                    (Some(states), Some(alerts)) => {
                                        super::serve_connection_with_notifications(
                                            stream, router, max_errors, states, alerts,
                                        )
                                        .await
                                    }
                                    (Some(states), None) => {
                                        super::serve_connection_with_auth_state(
                                            stream, router, max_errors, states,
                                        )
                                        .await
                                    }
                                    _ => super::serve_connection(stream, router, max_errors).await,
                                }
                            };
                            if let Err(error) = result {
                                tracing::warn!(
                                    error = %error,
                                    "IPC client connection ended with an error"
                                );
                            }
                        });
                    }
                }
            }
        }

        // Wait for all in-flight connection tasks to finish.
        while join_set.join_next().await.is_some() {}
        Ok(())
    }
}

#[cfg(unix)]
async fn accept_with_retry<A>(
    source: &A,
    shutdown: &mut Option<tokio::sync::watch::Receiver<bool>>,
    retry_policy: AcceptRetryPolicy,
) -> Option<A::Connection>
where
    A: AcceptSource,
{
    let mut retry_delay = retry_policy.initial_delay;

    loop {
        tokio::select! {
            accept = source.accept() => match accept {
                Ok(connection) => return Some(connection),
                Err(error) => {
                    tracing::warn!(
                        error_kind = ?error.kind(),
                        raw_os_error = ?error.raw_os_error(),
                        retry_delay_ms = retry_delay.as_millis(),
                        "IPC listener accept failed; retrying"
                    );

                    if wait_for_retry_or_shutdown(retry_delay, shutdown).await {
                        return None;
                    }
                    retry_delay = retry_delay
                        .checked_mul(2)
                        .unwrap_or(retry_policy.max_delay)
                        .min(retry_policy.max_delay);
                }
            },
            _ = next_shutdown(shutdown) => return None,
        }
    }
}

#[cfg(unix)]
async fn wait_for_retry_or_shutdown(
    delay: std::time::Duration,
    shutdown: &mut Option<tokio::sync::watch::Receiver<bool>>,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = next_shutdown(shutdown) => true,
    }
}

/// Resolves when `rx` fires `true` or the sender is dropped.
/// Never resolves when `rx` is `None`.
#[cfg(unix)]
async fn next_shutdown(rx: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(rx) = rx else {
        return std::future::pending().await;
    };
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        collections::VecDeque,
        io,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
        time::Duration,
    };

    use super::{accept_with_retry, AcceptRetryPolicy, AcceptSource};

    struct FakeAcceptSource {
        outcomes: Mutex<VecDeque<io::Result<usize>>>,
        attempts: AtomicUsize,
    }

    impl FakeAcceptSource {
        fn new(outcomes: impl IntoIterator<Item = io::Result<usize>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                attempts: AtomicUsize::new(0),
            }
        }
    }

    impl AcceptSource for FakeAcceptSource {
        type Connection = usize;

        async fn accept(&self) -> io::Result<Self::Connection> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let outcome = self.outcomes.lock().unwrap().pop_front();
            match outcome {
                Some(outcome) => outcome,
                None => std::future::pending().await,
            }
        }
    }

    #[tokio::test]
    async fn accept_errors_retry_and_reset_after_a_connection_succeeds() {
        let source = FakeAcceptSource::new([
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "connection aborted",
            )),
            Err(io::Error::from_raw_os_error(24)),
            Ok(7),
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "connection aborted",
            )),
            Ok(8),
        ]);
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let mut shutdown = Some(shutdown_rx);

        let accepted = tokio::time::timeout(
            Duration::from_secs(1),
            accept_with_retry(
                &source,
                &mut shutdown,
                AcceptRetryPolicy {
                    initial_delay: Duration::from_millis(1),
                    max_delay: Duration::from_millis(2),
                },
            ),
        )
        .await
        .expect("accept loop should survive recoverable errors");
        let accepted_after_reset = accept_with_retry(
            &source,
            &mut shutdown,
            AcceptRetryPolicy {
                initial_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(2),
            },
        )
        .await;

        assert_eq!(accepted, Some(7));
        assert_eq!(accepted_after_reset, Some(8));
        assert_eq!(source.attempts.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn shutdown_interrupts_accept_error_backoff() {
        let source = std::sync::Arc::new(FakeAcceptSource::new([Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "connection aborted",
        ))]));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task_source = std::sync::Arc::clone(&source);
        let task = tokio::spawn(async move {
            let mut shutdown = Some(shutdown_rx);
            accept_with_retry(
                task_source.as_ref(),
                &mut shutdown,
                AcceptRetryPolicy {
                    initial_delay: Duration::from_secs(60),
                    max_delay: Duration::from_secs(60),
                },
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while source.attempts.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fake accept should be attempted");
        shutdown_tx.send_replace(true);

        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("shutdown should interrupt the retry delay")
            .expect("accept loop task should exit cleanly");
        assert_eq!(source.attempts.load(Ordering::SeqCst), 1);
    }
}
