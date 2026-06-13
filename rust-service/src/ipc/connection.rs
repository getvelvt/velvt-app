use tokio::io::{AsyncRead, AsyncWrite};
use velvt_shared_types::{
    Acknowledged, MalformedMessage, MalformedMessageCode, ServerHello, ServerMessage,
    VersionMismatch, PROTOCOL_VERSION,
};

use super::{
    codec::{decode_client_hello, decode_client_message, read_frame, write_server_message},
    IpcError, MessageRouter,
};

/// Serves one transport-independent bidirectional client connection.
pub async fn serve_connection<S, R>(stream: S, router: R, max_errors: usize) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: MessageRouter,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    write_server_message(
        &mut writer,
        &ServerMessage::ServerHello(ServerHello {
            protocol_version: PROTOCOL_VERSION,
        }),
    )
    .await?;

    let mut errors = 0_usize;
    let hello = loop {
        let Some(frame) = read_frame(&mut reader).await? else {
            return Ok(());
        };
        match decode_client_hello(&frame) {
            Ok(hello) => break hello,
            Err(_) => {
                errors += 1;
                write_malformed(&mut writer).await?;
                if errors >= max_errors {
                    return Ok(());
                }
            }
        }
    };
    if hello.expected_protocol_version != PROTOCOL_VERSION {
        write_server_message(
            &mut writer,
            &ServerMessage::VersionMismatch(VersionMismatch {
                server_protocol_version: PROTOCOL_VERSION,
                client_protocol_version: hello.expected_protocol_version,
            }),
        )
        .await?;
        return Ok(());
    }
    write_server_message(&mut writer, &ServerMessage::Acknowledged(Acknowledged)).await?;

    while let Some(frame) = read_frame(&mut reader).await? {
        let message = match decode_client_message(&frame) {
            Ok(message) => message,
            Err(_) => {
                errors += 1;
                write_malformed(&mut writer).await?;
                if errors >= max_errors {
                    return Ok(());
                }
                continue;
            }
        };
        match router.route(message).await {
            Ok(Some(response)) => write_server_message(&mut writer, &response).await?,
            Ok(None) => {}
            Err(_) => {
                errors += 1;
                write_malformed(&mut writer).await?;
                if errors >= max_errors {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

async fn write_malformed(writer: &mut (impl AsyncWrite + Unpin)) -> Result<(), IpcError> {
    tracing::warn!(error_code = "malformed_message", "rejected IPC message");
    write_server_message(
        writer,
        &ServerMessage::MalformedMessage(MalformedMessage {
            code: MalformedMessageCode::InvalidMessage,
        }),
    )
    .await
}
