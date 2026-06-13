use velvt_shared_types::{ClientMessage, ServerMessage};

use super::IpcError;

/// Routes validated post-handshake messages independently of their transport.
#[allow(async_fn_in_trait)]
pub trait MessageRouter: Send + Sync {
    /// Handles one validated client message and optionally returns a response.
    async fn route(&self, message: ClientMessage) -> Result<Option<ServerMessage>, IpcError>;
}

/// Minimal R1 router used until business handlers are introduced.
#[derive(Debug, Clone, Copy)]
pub struct DefaultRouter;

impl MessageRouter for DefaultRouter {
    async fn route(&self, message: ClientMessage) -> Result<Option<ServerMessage>, IpcError> {
        match message {
            ClientMessage::ClientHello(_) => Err(IpcError::MalformedMessage),
            _ => Ok(None),
        }
    }
}
