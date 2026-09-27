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

/// Reads one newline-terminated frame.
///
/// Cancel-safe, which the connection loop depends on: it races this read
/// against pushes, auth changes and privacy alerts in `tokio::select!`, and
/// drops the losing futures. Every byte read goes straight into `pending`,
/// which the caller owns and hands back on the next call, so a frame that was
/// half read when another branch won is resumed rather than lost. When the
/// partial frame lived inside this future instead, the rest of it arrived as
/// a frame of its own and was answered with `malformed_message`: a client that
/// sent two frames back to back lost the second whenever the first queued a
/// push, and each loss counted towards the connection's error limit.
pub(super) async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
    pending: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, IpcError> {
    let mut byte = [0_u8; 1];

    loop {
        // `read` is itself cancel-safe: a cancelled call has read nothing.
        let count = reader
            .read(&mut byte)
            .await
            .map_err(|_| IpcError::Transport)?;
        if count == 0 {
            return if pending.is_empty() {
                Ok(None)
            } else {
                pending.clear();
                Err(IpcError::MalformedMessage)
            };
        }
        if byte[0] == b'\n' {
            return Ok(Some(std::mem::take(pending)));
        }
        if pending.len() >= MAX_FRAME_BYTES {
            pending.clear();
            return Err(IpcError::FrameTooLarge);
        }
        pending.push(byte[0]);
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{duplex, AsyncWriteExt};

    use super::read_frame;

    /// The connection loop's shape: a read raced against something else that
    /// becomes ready while a frame is only partly on the socket.
    #[tokio::test]
    async fn a_frame_interrupted_mid_read_is_resumed_not_lost() {
        let (mut client, mut server) = duplex(1024);
        let frame = br#"{"type":"request_demotion_state","payload":{}}"#;
        let (head, tail) = frame.split_at(20);
        let mut pending = Vec::new();

        client.write_all(head).await.unwrap();
        tokio::select! {
            biased;
            _ = read_frame(&mut server, &mut pending) => panic!("half a frame is not a frame"),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        assert_eq!(pending, head, "the bytes read so far are kept");

        client.write_all(tail).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        let read = read_frame(&mut server, &mut pending).await.unwrap();
        assert_eq!(read.as_deref(), Some(&frame[..]));
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn back_to_back_frames_are_read_one_at_a_time() {
        let (mut client, mut server) = duplex(1024);
        client.write_all(b"{\"a\":1}\n{\"b\":2}\n").await.unwrap();
        let mut pending = Vec::new();
        assert_eq!(
            read_frame(&mut server, &mut pending)
                .await
                .unwrap()
                .as_deref(),
            Some(&b"{\"a\":1}"[..])
        );
        assert_eq!(
            read_frame(&mut server, &mut pending)
                .await
                .unwrap()
                .as_deref(),
            Some(&b"{\"b\":2}"[..])
        );
    }
}
