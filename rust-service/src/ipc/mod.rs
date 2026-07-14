//! Unix domain socket IPC contract and server interfaces.
//!
//! This module owns newline-delimited JSON framing, version negotiation, and
//! message dispatch. It does not own event capture, abstraction, persistence,
//! cloud upload, or UI rendering.

#![allow(async_fn_in_trait)]

pub use velvt_shared_types::*;

mod codec;
mod connection;
mod reconnect;
mod router;
pub mod transport;

pub use connection::{
    serve_connection, serve_connection_with_auth_state, serve_connection_with_notifications,
    serve_connection_with_push_queue, serve_connection_with_push_queue_and_shutdown,
};
pub use reconnect::ReconnectTracker;
pub use router::{DefaultRouter, MenuStatusProvider, MessageRouter, R7Router};

/// Errors produced by IPC transport or protocol handling.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Socket transport failed.
    #[error("IPC transport failed")]
    Transport,
    /// Message framing or decoding failed.
    #[error("IPC message is malformed")]
    MalformedMessage,
    /// A client frame exceeded the configured transport limit.
    #[error("IPC message exceeded the frame limit")]
    FrameTooLarge,
}
