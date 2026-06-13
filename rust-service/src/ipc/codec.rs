use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use velvt_shared_types::{ClientHello, ClientMessage, ServerMessage};

use super::IpcError;

const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
enum HandshakeFrame {
    ClientHello(ClientHello),
}

pub(super) async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
) -> Result<Option<Vec<u8>>, IpcError> {
    let mut frame = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let count = reader
            .read(&mut byte)
            .await
            .map_err(|_| IpcError::Transport)?;
        if count == 0 {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(IpcError::MalformedMessage)
            };
        }
        if byte[0] == b'\n' {
            return Ok(Some(frame));
        }
        if frame.len() >= MAX_FRAME_BYTES {
            return Err(IpcError::FrameTooLarge);
        }
        frame.push(byte[0]);
    }
}

pub(super) fn decode_client_hello(frame: &[u8]) -> Result<ClientHello, IpcError> {
    match serde_json::from_slice::<HandshakeFrame>(frame) {
        Ok(HandshakeFrame::ClientHello(hello)) => Ok(hello),
        Err(_) => Err(IpcError::MalformedMessage),
    }
}

pub(super) fn decode_client_message(frame: &[u8]) -> Result<ClientMessage, IpcError> {
    serde_json::from_slice(frame).map_err(|_| IpcError::MalformedMessage)
}

pub(super) async fn write_server_message(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &ServerMessage,
) -> Result<(), IpcError> {
    let mut bytes = serde_json::to_vec(message).map_err(|_| IpcError::MalformedMessage)?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| IpcError::Transport)
}
