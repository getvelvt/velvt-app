//! Concrete IPC transports isolated from message routing and connection state.

#![allow(async_fn_in_trait)]

use super::IpcError;

/// Runs an IPC accept loop without exposing transport details to handlers.
pub trait IpcTransport {
    /// Accepts clients until the transport is stopped or fails.
    async fn run(&self) -> Result<(), IpcError>;
}

/// Tokio Unix-domain-socket transport for the macOS service.
#[cfg(unix)]
pub struct TokioUnixTransport {
    socket_path: std::path::PathBuf,
    max_errors: usize,
    auth_states: Option<tokio::sync::watch::Receiver<crate::auth::AuthState>>,
}

#[cfg(unix)]
impl TokioUnixTransport {
    /// Creates a Unix transport for the configured socket path.
    pub fn new(socket_path: std::path::PathBuf, max_errors: usize) -> Self {
        Self {
            socket_path,
            max_errors,
            auth_states: None,
        }
    }

    /// Pushes authentication state changes to every connected Swift client.
    pub fn with_auth_state(
        mut self,
        auth_states: tokio::sync::watch::Receiver<crate::auth::AuthState>,
    ) -> Self {
        self.auth_states = Some(auth_states);
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
impl IpcTransport for TokioUnixTransport {
    async fn run(&self) -> Result<(), IpcError> {
        let listener = self.bind().await?;
        loop {
            let (stream, _) = listener.accept().await.map_err(|_| IpcError::Transport)?;
            let max_errors = self.max_errors;
            let auth_states = self.auth_states.clone();
            tokio::spawn(async move {
                let result = match auth_states {
                    Some(states) => {
                        super::serve_connection_with_auth_state(
                            stream,
                            super::DefaultRouter,
                            max_errors,
                            states,
                        )
                        .await
                    }
                    None => super::serve_connection(stream, super::DefaultRouter, max_errors).await,
                };
                if let Err(error) = result {
                    tracing::warn!(error = %error, "IPC client connection ended with an error");
                }
            });
        }
    }
}
