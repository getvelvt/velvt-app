use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
use velvt_service::ipc::{serve_connection, DefaultRouter};
use velvt_service::{
    auth::{AuthState, AuthStateMachine},
    ipc::{serve_connection_with_auth_state, serve_connection_with_notifications},
};
use velvt_shared_types::{
    Acknowledged, ClientHello, ClientMessage, MalformedMessage, MalformedMessageCode,
    PrivacyViolationAlert, ServerHello, ServerMessage, ServiceState, VersionMismatch,
    PROTOCOL_VERSION,
};

async fn write_message(writer: &mut (impl AsyncWriteExt + Unpin), message: &ClientMessage) {
    let mut bytes = serde_json::to_vec(message).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn read_message(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
) -> Option<ServerMessage> {
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap() == 0 {
        return None;
    }
    Some(serde_json::from_str(line.trim_end()).unwrap())
}

async fn complete_handshake(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
    writer: &mut (impl AsyncWriteExt + Unpin),
) {
    assert!(matches!(
        read_message(reader).await,
        Some(ServerMessage::ServerHello(_))
    ));
    write_message(
        writer,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert_eq!(
        read_message(reader).await,
        Some(ServerMessage::Acknowledged(Acknowledged))
    );
}

#[derive(Clone)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

impl Write for LogWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn handshake_succeeds_over_in_memory_stream() {
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, 3));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);

    assert_eq!(
        read_message(&mut read).await,
        Some(ServerMessage::ServerHello(ServerHello {
            protocol_version: PROTOCOL_VERSION
        }))
    );
    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert_eq!(
        read_message(&mut read).await,
        Some(ServerMessage::Acknowledged(Acknowledged))
    );

    drop(write);
    drop(read);
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn version_mismatch_returns_typed_error_and_closes() {
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, 3));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    read_message(&mut read).await;

    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: 99,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert_eq!(
        read_message(&mut read).await,
        Some(ServerMessage::VersionMismatch(VersionMismatch {
            server_protocol_version: PROTOCOL_VERSION,
            client_protocol_version: 99,
        }))
    );
    assert_eq!(read_message(&mut read).await, None);
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn malformed_message_is_rejected_without_closing_connection() {
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, 3));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    read_message(&mut read).await;
    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    read_message(&mut read).await;

    write.write_all(b"garbage\n").await.unwrap();
    assert_eq!(
        read_message(&mut read).await,
        Some(ServerMessage::MalformedMessage(MalformedMessage {
            code: MalformedMessageCode::InvalidMessage
        }))
    );

    write.write_all(b"garbage\n").await.unwrap();
    assert!(read_message(&mut read).await.is_some());
    drop(write);
    drop(read);
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn malformed_pre_handshake_message_can_be_retried() {
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, 3));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    read_message(&mut read).await;

    write.write_all(b"garbage\n").await.unwrap();
    assert!(matches!(
        read_message(&mut read).await,
        Some(ServerMessage::MalformedMessage(_))
    ));
    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert_eq!(
        read_message(&mut read).await,
        Some(ServerMessage::Acknowledged(Acknowledged))
    );

    drop(write);
    drop(read);
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn client_disconnect_mid_handshake_finishes_connection_task() {
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, 3));
    let (read, write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    assert!(matches!(
        read_message(&mut read).await,
        Some(ServerMessage::ServerHello(_))
    ));

    drop(write);
    drop(read);

    assert!(tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
}

#[tokio::test]
async fn client_close_after_handshake_finishes_connection_task() {
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, 3));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    drop(write);
    drop(read);

    assert!(tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
}

#[tokio::test]
async fn simultaneous_clients_keep_independent_handshake_state() {
    let (client_one, server_one) = duplex(4096);
    let (client_two, server_two) = duplex(4096);
    let task_one = tokio::spawn(serve_connection(server_one, DefaultRouter, 3));
    let task_two = tokio::spawn(serve_connection(server_two, DefaultRouter, 3));
    let (read_one, mut write_one) = tokio::io::split(client_one);
    let (read_two, mut write_two) = tokio::io::split(client_two);
    let mut read_one = BufReader::new(read_one);
    let mut read_two = BufReader::new(read_two);

    complete_handshake(&mut read_one, &mut write_one).await;
    write_message(
        &mut write_two,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: 99,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert!(matches!(
        read_message(&mut read_two).await,
        Some(ServerMessage::ServerHello(_))
    ));
    assert!(matches!(
        read_message(&mut read_two).await,
        Some(ServerMessage::VersionMismatch(_))
    ));
    assert_eq!(read_message(&mut read_two).await, None);

    write_one.write_all(b"garbage\n").await.unwrap();
    assert!(matches!(
        read_message(&mut read_one).await,
        Some(ServerMessage::MalformedMessage(_))
    ));

    drop(write_one);
    drop(read_one);
    drop(write_two);
    drop(read_two);
    assert!(task_one.await.unwrap().is_ok());
    assert!(task_two.await.unwrap().is_ok());
}

#[tokio::test]
async fn max_errors_threshold_closes_connection_before_n_plus_one() {
    const MAX_ERRORS: usize = 3;
    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection(server, DefaultRouter, MAX_ERRORS));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    for _ in 0..MAX_ERRORS {
        write.write_all(b"garbage\n").await.unwrap();
        assert!(matches!(
            read_message(&mut read).await,
            Some(ServerMessage::MalformedMessage(_))
        ));
    }

    assert_eq!(read_message(&mut read).await, None);
    assert!(write.write_all(b"garbage\n").await.is_err());
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn malformed_rejection_logs_never_contain_raw_user_content() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(LogCapture(Arc::clone(&captured)))
        .with_max_level(tracing::Level::TRACE)
        .without_time()
        .finish();

    let _guard = tracing::subscriber::set_default(subscriber);
    let (client, server) = duplex(4096);
    let client_future = async {
        let (read, mut write) = tokio::io::split(client);
        let mut read = BufReader::new(read);
        complete_handshake(&mut read, &mut write).await;
        write
            .write_all(b"{\"app_name\":\"PRIVATE_APP\",\"window_title\":\"PRIVATE_TITLE\",\"url\":\"https://private.example\"}\n")
            .await
            .unwrap();
        assert!(matches!(
            read_message(&mut read).await,
            Some(ServerMessage::MalformedMessage(_))
        ));
    };
    let (server_result, ()) =
        tokio::join!(serve_connection(server, DefaultRouter, 1), client_future);
    assert!(server_result.is_ok());

    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains("PRIVATE_APP"));
    assert!(!logs.contains("PRIVATE_TITLE"));
    assert!(!logs.contains("https://private.example"));
}

#[tokio::test]
async fn device_revoked_auth_state_is_pushed_to_swift_as_upload_paused() {
    let (client, server) = duplex(4096);
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let task = tokio::spawn(serve_connection_with_auth_state(
        server,
        DefaultRouter,
        3,
        state.subscribe(),
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;
    assert!(matches!(
        read_message(&mut read).await,
        Some(ServerMessage::ServiceStatus(
            velvt_shared_types::ServiceStatus {
                state: ServiceState::Ready,
                reason: None,
            }
        ))
    ));

    state.transition(AuthState::DeviceRevoked).unwrap();

    let status = read_message(&mut read).await.unwrap();
    assert!(matches!(
        status,
        ServerMessage::ServiceStatus(velvt_shared_types::ServiceStatus {
            state: ServiceState::UploadPaused,
            reason: Some(reason),
        }) if reason == "device_revoked"
    ));
    drop(write);
    drop(read);
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn newly_connected_swift_client_receives_current_device_revoked_state() {
    let (client, server) = duplex(4096);
    let state = Arc::new(AuthStateMachine::new(AuthState::DeviceRevoked));
    let task = tokio::spawn(serve_connection_with_auth_state(
        server,
        DefaultRouter,
        3,
        state.subscribe(),
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    assert!(matches!(
        read_message(&mut read).await,
        Some(ServerMessage::ServiceStatus(
            velvt_shared_types::ServiceStatus {
                state: ServiceState::UploadPaused,
                reason: Some(reason),
            }
        )) if reason == "device_revoked"
    ));
    drop(write);
    drop(read);
    assert!(task.await.unwrap().is_ok());
}

#[tokio::test]
async fn privacy_violation_alert_is_pushed_to_swift() {
    let (client, server) = duplex(4096);
    let state = Arc::new(AuthStateMachine::new(AuthState::Authenticated {
        device_id: "device-1".into(),
    }));
    let (alerts, receiver) = tokio::sync::broadcast::channel(4);
    let task = tokio::spawn(serve_connection_with_notifications(
        server,
        DefaultRouter,
        3,
        state.subscribe(),
        receiver,
    ));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;
    read_message(&mut read).await;

    alerts
        .send(PrivacyViolationAlert {
            code: "raw_field_rejected".into(),
            message: "safe rejection".into(),
        })
        .unwrap();

    assert!(matches!(
        read_message(&mut read).await,
        Some(ServerMessage::PrivacyViolationAlert(PrivacyViolationAlert { code, .. }))
            if code == "raw_field_rejected"
    ));
    drop(write);
    drop(read);
    assert!(task.await.unwrap().is_ok());
}
