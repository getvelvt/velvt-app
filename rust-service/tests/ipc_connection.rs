use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
use velvt_service::ipc::{serve_connection, DefaultRouter};
use velvt_shared_types::{
    Acknowledged, ClientHello, ClientMessage, MalformedMessage, MalformedMessageCode, ServerHello,
    ServerMessage, VersionMismatch, PROTOCOL_VERSION,
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
