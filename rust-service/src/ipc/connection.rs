use std::{sync::Arc, time::Duration};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use velvt_shared_types::{
    Acknowledged, MalformedMessage, MalformedMessageCode, ServerHello, ServerMessage, ServiceState,
    ServiceStatus, VersionMismatch, PROTOCOL_VERSION,
};

use crate::{auth::AuthState, delivery::PushQueue};

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
    serve_connection_inner(
        stream,
        router,
        max_errors,
        None,
        None,
        None,
        Duration::ZERO,
        None,
    )
    .await
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
    serve_connection_inner(
        stream,
        router,
        max_errors,
        Some(auth_states),
        None,
        None,
        Duration::ZERO,
        None,
    )
    .await
}

pub async fn serve_connection_with_notifications<S, R>(
    stream: S,
    router: R,
    max_errors: usize,
    auth_states: tokio::sync::watch::Receiver<AuthState>,
    privacy_alerts: tokio::sync::broadcast::Receiver<velvt_shared_types::PrivacyViolationAlert>,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: MessageRouter,
{
    serve_connection_inner(
        stream,
        router,
        max_errors,
        Some(auth_states),
        Some(privacy_alerts),
        None,
        Duration::ZERO,
        None,
    )
    .await
}

/// Serves one connection with a bounded push queue for proactive delivery.
///
/// The queue is drained in FIFO order on every loop tick.  Each write is
/// wrapped in `write_timeout`; if the client is too slow the connection is
/// closed but the queue is preserved for reconnect.
pub async fn serve_connection_with_push_queue<S, R>(
    stream: S,
    router: R,
    max_errors: usize,
    auth_states: Option<tokio::sync::watch::Receiver<AuthState>>,
    push_queue: Arc<PushQueue>,
    write_timeout: Duration,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: MessageRouter,
{
    serve_connection_inner(
        stream,
        router,
        max_errors,
        auth_states,
        None,
        Some(push_queue),
        write_timeout,
        None,
    )
    .await
}

/// Serves one connection with a bounded push queue and graceful shutdown support.
///
/// When `shutdown_rx` fires `true`, any remaining items in the push queue
/// (including a pre-enqueued `ShuttingDown` message placed at the front by the
/// caller) are flushed to the client before the connection is closed.
pub async fn serve_connection_with_push_queue_and_shutdown<S, R>(
    stream: S,
    router: R,
    max_errors: usize,
    auth_states: Option<tokio::sync::watch::Receiver<AuthState>>,
    push_queue: Arc<PushQueue>,
    write_timeout: Duration,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: MessageRouter,
{
    serve_connection_inner(
        stream,
        router,
        max_errors,
        auth_states,
        None,
        Some(push_queue),
        write_timeout,
        Some(shutdown_rx),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection_inner<S, R>(
    stream: S,
    router: R,
    max_errors: usize,
    mut auth_states: Option<tokio::sync::watch::Receiver<AuthState>>,
    mut privacy_alerts: Option<
        tokio::sync::broadcast::Receiver<velvt_shared_types::PrivacyViolationAlert>,
    >,
    push_queue: Option<Arc<PushQueue>>,
    write_timeout: Duration,
    mut shutdown_rx: Option<watch::Receiver<bool>>,
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
        // Drain all pending push messages before blocking on the next event.
        if let Some(queue) = &push_queue {
            while let Some(msg) = queue.try_pop().await {
                write_with_timeout(&mut writer, &msg, write_timeout).await?;
            }
        }

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
            alert = next_privacy_alert(&mut privacy_alerts) => {
                let Some(alert) = alert else {
                    privacy_alerts = None;
                    continue;
                };
                write_server_message(&mut writer, &ServerMessage::PrivacyViolationAlert(alert)).await?;
                continue;
            }
            _ = next_push_notify(&push_queue) => {
                // A message was enqueued; loop back to drain it.
                continue;
            }
            _ = next_shutdown(&mut shutdown_rx) => {
                // Shutdown fired: drain remaining push messages (ShuttingDown is at front)
                // then close the connection gracefully.
                if let Some(queue) = &push_queue {
                    while let Some(msg) = queue.try_pop().await {
                        let _ = write_server_message(&mut writer, &msg).await;
                    }
                }
                return Ok(());
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

/// Writes `msg` with a per-message timeout; returns `Err` if the client is slow.
///
/// Only the message type is logged — never any payload content.
async fn write_with_timeout(
    writer: &mut (impl AsyncWrite + Unpin),
    msg: &ServerMessage,
    timeout: Duration,
) -> Result<(), IpcError> {
    match tokio::time::timeout(timeout, write_server_message(writer, msg)).await {
        Ok(result) => result,
        Err(_elapsed) => {
            tracing::warn!(
                message_type = server_message_type_name(msg),
                error_code = "slow_client_write_timeout",
                "disconnecting slow client"
            );
            Err(IpcError::Transport)
        }
    }
}

fn server_message_type_name(msg: &ServerMessage) -> &'static str {
    match msg {
        ServerMessage::ServerHello(_) => "server_hello",
        ServerMessage::Acknowledged(_) => "acknowledged",
        ServerMessage::VersionMismatch(_) => "version_mismatch",
        ServerMessage::MalformedMessage(_) => "malformed_message",
        ServerMessage::RawEventAck(_) => "raw_event_ack",
        ServerMessage::InsightPayload(_) => "insight_payload",
        ServerMessage::HistoryPayload(_) => "history_payload",
        ServerMessage::ServiceStatus(_) => "service_status",
        ServerMessage::PrivacyViolationAlert(_) => "privacy_violation_alert",
        ServerMessage::ErrorResponse(_) => "error_response",
        ServerMessage::CacheEmpty(_) => "cache_empty",
        ServerMessage::ShuttingDown(_) => "shutting_down",
    }
}

async fn next_push_notify(queue: &Option<Arc<PushQueue>>) {
    let Some(queue) = queue else {
        return std::future::pending().await;
    };
    queue.notify().notified().await
}

async fn next_privacy_alert(
    receiver: &mut Option<
        tokio::sync::broadcast::Receiver<velvt_shared_types::PrivacyViolationAlert>,
    >,
) -> Option<velvt_shared_types::PrivacyViolationAlert> {
    let Some(receiver) = receiver else {
        return std::future::pending().await;
    };
    receiver.recv().await.ok()
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

/// Resolves when `rx` fires `true` or the sender is dropped.  Never resolves
/// when `rx` is `None` (allows this arm to be permanently disabled).
async fn next_shutdown(rx: &mut Option<watch::Receiver<bool>>) {
    let Some(rx) = rx else {
        return std::future::pending().await;
    };
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        if *rx.borrow() {
            return;
        }
    }
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
