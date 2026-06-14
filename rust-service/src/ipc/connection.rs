use tokio::io::{AsyncRead, AsyncWrite};
use velvt_shared_types::{
    Acknowledged, MalformedMessage, MalformedMessageCode, ServerHello, ServerMessage, ServiceState,
    ServiceStatus, VersionMismatch, PROTOCOL_VERSION,
};

use crate::auth::AuthState;

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
    serve_connection_inner(stream, router, max_errors, None).await
}

/// Serves one connection and pushes privacy-safe authentication state changes.
pub async fn serve_connection_with_auth_state<S, R>(
    stream: S,
    router: R,
    max_errors: usize,
    auth_states: tokio::sync::watch::Receiver<AuthState>,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: MessageRouter,
{
    serve_connection_inner(stream, router, max_errors, Some(auth_states)).await
}

async fn serve_connection_inner<S, R>(
    stream: S,
    router: R,
    max_errors: usize,
    mut auth_states: Option<tokio::sync::watch::Receiver<AuthState>>,
) -> Result<(), IpcError>
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
    if let Some(states) = &auth_states {
        let initial_state = states.borrow().clone();
        write_server_message(&mut writer, &auth_status_message(initial_state)).await?;
    }

    loop {
        let frame = tokio::select! {
            frame = read_frame(&mut reader) => frame?,
            state = next_auth_state(&mut auth_states) => {
                let Some(state) = state else {
                    auth_states = None;
                    continue;
                };
                write_server_message(&mut writer, &auth_status_message(state)).await?;
                continue;
            }
        };
        let Some(frame) = frame else {
            return Ok(());
        };
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
}

async fn next_auth_state(
    receiver: &mut Option<tokio::sync::watch::Receiver<AuthState>>,
) -> Option<AuthState> {
    let Some(receiver) = receiver else {
        return std::future::pending().await;
    };
    receiver.changed().await.ok()?;
    Some(receiver.borrow().clone())
}

fn auth_status_message(state: AuthState) -> ServerMessage {
    let (state, reason) = match state {
        AuthState::Authenticated { .. } => (ServiceState::Ready, None),
        AuthState::RefreshInFlight => (ServiceState::Degraded, Some("auth_refresh_in_flight")),
        AuthState::Unauthenticated | AuthState::NeedsReauth => {
            (ServiceState::AuthRequired, Some("needs_reauth"))
        }
        AuthState::DeviceRevoked => (ServiceState::UploadPaused, Some("device_revoked")),
    };
    ServerMessage::ServiceStatus(ServiceStatus {
        state,
        reason: reason.map(str::to_owned),
    })
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
