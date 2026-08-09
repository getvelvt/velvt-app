#![cfg(unix)]

use std::{io, path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;
use velvt_service::ipc::transport::{IpcTransport, TokioUnixTransport};
use velvt_shared_types::{
    ClientHello, ClientMessage, ErrorResponse, ServerMessage, PROTOCOL_VERSION,
};

async fn read_message(reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>) -> ServerMessage {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("timed out waiting for IPC frame")
        .unwrap();
    serde_json::from_str(line.trim_end()).unwrap()
}

async fn write_message(writer: &mut (impl tokio::io::AsyncWrite + Unpin), message: &ClientMessage) {
    let mut bytes = serde_json::to_vec(message).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn connect_and_handshake(socket_path: &std::path::Path) -> tokio::net::unix::OwnedWriteHalf {
    let stream = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match tokio::net::UnixStream::connect(socket_path).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("timed out waiting for IPC socket");
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

// Sockets live under literal `/tmp`, not the working directory or
// `std::env::temp_dir()`: the checkout may sit on a filesystem that cannot
// host Unix sockets at all (exFAT returns `ENOTSUP` on bind), and macOS's
// per-user temp dir (`/var/folders/...`) is long enough that a joined path
// can exceed the 104-byte `sun_path` limit of `sockaddr_un`. Short absolute
// `/tmp` paths satisfy both constraints.
//
// Each socket gets its own subdirectory because the transport's `bind()`
// creates the socket's parent and chmods it to 0700 — pointing it at `/tmp`
// itself would make that chmod fail (and must not succeed).
//
// Uniqueness comes from a UUID rather than a timestamp. `SystemTime::now()` is
// only microsecond-granular on macOS, so `as_nanos()` repeats across calls made
// close together — concurrent tests in this binary share a PID and would derive
// the same path, leaving one of them to fail its bind with `EEXIST`.
fn socket_path(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/velvt-ipc-{}-{}/{name}.sock",
        std::process::id(),
        Uuid::new_v4().simple()
    ))
}

fn filesystem_sockets_available() -> bool {
    let path = PathBuf::from(format!(
        "/tmp/velvt-ipc-preflight-{}-{}.sock",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    match std::os::unix::net::UnixListener::bind(&path) {
        Ok(listener) => {
            drop(listener);
            let _ = std::fs::remove_file(path);
            true
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
            ) =>
        {
            eprintln!("skipping Unix socket smoke test: filesystem sockets are unavailable");
            false
        }
        Err(error) => panic!("failed to bind Unix socket preflight path: {error}"),
    }
}

#[tokio::test]
async fn unix_socket_smoke_covers_success_and_rejection() {
    if !filesystem_sockets_available() {
        return;
    }
    let socket_path = socket_path("single-client");
    let transport = TokioUnixTransport::new(socket_path.clone(), 3);
    let server = tokio::spawn(async move { transport.run().await });

    let stream = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("timed out waiting for IPC socket");
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
    if !filesystem_sockets_available() {
        return;
    }
    let socket_path = socket_path("two-clients");
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
