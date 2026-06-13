#![cfg(unix)]

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use velvt_service::ipc::transport::{IpcTransport, TokioUnixTransport};
use velvt_shared_types::{
    ClientHello, ClientMessage, ErrorResponse, ServerMessage, PROTOCOL_VERSION,
};

async fn read_message(reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>) -> ServerMessage {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(line.trim_end()).unwrap()
}

async fn write_message(writer: &mut (impl tokio::io::AsyncWrite + Unpin), message: &ClientMessage) {
    let mut bytes = serde_json::to_vec(message).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn connect_and_handshake(socket_path: &std::path::Path) -> tokio::net::unix::OwnedWriteHalf {
    let stream = loop {
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(stream) => break stream,
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    assert!(matches!(
        read_message(&mut read).await,
        ServerMessage::ServerHello(_)
    ));
    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert!(matches!(
        read_message(&mut read).await,
        ServerMessage::Acknowledged(_)
    ));
    write
}

#[tokio::test]
async fn unix_socket_smoke_covers_success_and_rejection() {
    let socket_path = std::env::temp_dir().join(format!(
        "velvt-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let transport = TokioUnixTransport::new(socket_path.clone(), 3);
    let server = tokio::spawn(async move { transport.run().await });

    let stream = loop {
        match tokio::net::UnixStream::connect(&socket_path).await {
            Ok(stream) => break stream,
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    assert!(matches!(
        read_message(&mut read).await,
        ServerMessage::ServerHello(_)
    ));

    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert!(matches!(
        read_message(&mut read).await,
        ServerMessage::Acknowledged(_)
    ));

    write_message(
        &mut write,
        &ClientMessage::ErrorResponse(ErrorResponse {
            code: "smoke_test".into(),
            message: "safe".into(),
            related_event_id: None,
        }),
    )
    .await;

    write.write_all(b"garbage\n").await.unwrap();
    assert!(matches!(
        read_message(&mut read).await,
        ServerMessage::MalformedMessage(_)
    ));

    server.abort();
    let _ = tokio::fs::remove_file(socket_path).await;
}

#[tokio::test]
async fn unix_socket_accepts_two_simultaneous_clients() {
    let socket_path = std::env::temp_dir().join(format!(
        "velvt-two-clients-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let transport = TokioUnixTransport::new(socket_path.clone(), 3);
    let server = tokio::spawn(async move { transport.run().await });

    let (client_one, client_two) = tokio::join!(
        connect_and_handshake(&socket_path),
        connect_and_handshake(&socket_path)
    );

    drop(client_one);
    drop(client_two);
    server.abort();
    let _ = tokio::fs::remove_file(socket_path).await;
}
