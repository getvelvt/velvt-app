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
}

#[cfg(unix)]
impl TokioUnixTransport {
    /// Creates a Unix transport for the configured socket path.
    pub fn new(socket_path: std::path::PathBuf, max_errors: usize) -> Self {
        Self {
            socket_path,
            max_errors,
        }
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
            tokio::spawn(async move {
                if let Err(error) =
                    super::serve_connection(stream, super::DefaultRouter, max_errors).await
                {
                    tracing::warn!(error = %error, "IPC client connection ended with an error");
                }
            });
        }
    }
}
