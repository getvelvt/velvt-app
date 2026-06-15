//! Concrete IPC transports isolated from message routing and connection state.

#![allow(async_fn_in_trait)]

use super::IpcError;

/// Runs an IPC accept loop without exposing transport details to handlers.
pub trait IpcTransport {
    /// Accepts clients until the transport is stopped or fails.
    async fn run(&self) -> Result<(), IpcError>;
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

    /// Attaches a push queue; enables proactive delivery to connected clients.
    pub fn with_push_queue(
        mut self,
        queue: std::sync::Arc<crate::delivery::PushQueue>,
        write_timeout: std::time::Duration,
    ) -> Self {
        self.push_queue = Some(queue);
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
        loop {
            let (stream, _) = listener.accept().await.map_err(|_| IpcError::Transport)?;
            let max_errors = self.max_errors;
            let auth_states = self.auth_states.clone();
            let privacy_alerts = self
                .privacy_alerts
                .as_ref()
                .map(|alerts| alerts.subscribe());
            let push_queue = self.push_queue.clone();
            let write_timeout = self.write_timeout;
            let router = self.router.clone();
            tokio::spawn(async move {
                let result = if let Some(pq) = push_queue {
                    super::serve_connection_with_push_queue(
                        stream,
                        router,
                        max_errors,
                        auth_states,
                        pq,
                        write_timeout,
                    )
                    .await
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
                    tracing::warn!(error = %error, "IPC client connection ended with an error");
                }
            });
        }
    }
}
