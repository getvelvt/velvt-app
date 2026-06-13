//! Unix domain socket IPC contract and server interfaces.
//!
//! This module owns newline-delimited JSON framing, version negotiation, and
//! message dispatch. It does not own event capture, abstraction, persistence,
//! cloud upload, or UI rendering.

#![allow(async_fn_in_trait)]

use std::path::Path;
pub use velvt_shared_types::*;

/// Starts and stops the local IPC server.
///
/// Implementors bind the configured Unix domain socket, require a successful
/// handshake before accepting raw events, dispatch only [`InboundMessage`]
/// variants, and emit only [`OutboundMessage`] variants. They must remove only
/// a confirmed stale socket file and must never log decoded message content.
pub trait IpcServer {
    /// Runs the server at `socket_path` until shutdown is requested.
    async fn run(&self, socket_path: &Path, protocol_version: u32) -> Result<(), IpcError>;

    /// Requests a clean server shutdown.
    async fn shutdown(&self) -> Result<(), IpcError>;
}

/// Encodes and decodes one newline-delimited JSON frame.
///
/// Implementors must preserve the exact snake_case field names defined in
/// `proto/schema/`, append exactly one `\n` when encoding, reject undeclared
/// fields, and reject frames that are not valid UTF-8 JSON objects.
pub trait IpcMessageCodec {
    /// Decodes one complete client-to-server JSON line without the delimiter.
    fn decode_inbound(&self, frame: &[u8]) -> Result<InboundMessage, IpcError>;

    /// Encodes one server-to-client message and appends one newline delimiter.
    fn encode_outbound(&self, message: &OutboundMessage) -> Result<Vec<u8>, IpcError>;
}

/// Negotiates the protocol before any other inbound message is dispatched.
pub trait HandshakeNegotiator {
    /// Accepts an exact version match or returns a rejected response.
    fn negotiate(
        &self,
        request: &HandshakeRequest,
        server_protocol_version: u32,
    ) -> HandshakeResponse;
}

/// Errors produced by IPC transport or protocol handling.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// Socket transport failed.
    #[error("IPC transport failed")]
    Transport,
    /// Message framing or decoding failed.
    #[error("IPC message is malformed")]
    MalformedMessage,
    /// Client and server protocol versions do not match.
    #[error("unsupported IPC protocol version")]
    UnsupportedProtocolVersion,
}
