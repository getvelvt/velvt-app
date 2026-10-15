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
// That subdirectory is the test's to remove, not only the socket inside it:
// `SocketDir` deletes both on drop, so a failed assertion cleans up too and
// repeated runs do not leave empty `/tmp/velvt-ipc-*` directories behind.
//
// Uniqueness comes from a UUID rather than a timestamp. `SystemTime::now()` is
// only microsecond-granular on macOS, so `as_nanos()` repeats across calls made
// close together — concurrent tests in this binary share a PID and would derive
// the same path, leaving one of them to fail its bind with `EEXIST`.
struct SocketDir(PathBuf);

impl SocketDir {
    fn new() -> Self {
        Self(PathBuf::from(format!(
            "/tmp/velvt-ipc-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        )))
    }

    fn socket_path(&self, name: &str) -> PathBuf {
        self.0.join(format!("{name}.sock"))
    }
}

impl Drop for SocketDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
    let socket_dir = SocketDir::new();
    let socket_path = socket_dir.socket_path("single-client");
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
}

#[tokio::test]
async fn unix_socket_accepts_two_simultaneous_clients() {
    if !filesystem_sockets_available() {
        return;
    }
    let socket_dir = SocketDir::new();
    let socket_path = socket_dir.socket_path("two-clients");
    let transport = TokioUnixTransport::new(socket_path.clone(), 3);
    let server = tokio::spawn(async move { transport.run().await });

    let (client_one, client_two) = tokio::join!(
        connect_and_handshake(&socket_path),
        connect_and_handshake(&socket_path)
    );

    drop(client_one);
    drop(client_two);
    server.abort();
}

/// PR A0 (docs/architecture/personal-vault-and-devices.md §9.3): a Release
/// helper serves only a peer that passes, and closes any other before writing
/// it a single byte, so a refused peer never sees `server_hello` or anything
/// pushed after it.
///
/// The test binary stands in for an unsigned peer: it is linker-signed ad hoc,
/// with no team and an identifier that is not the app's. macOS will not run
/// truly unsigned code on Apple Silicon, so ad hoc is what any program a
/// process running as the person could build for itself looks like. It is
/// pinned as the expected process, so the refusal is the signature's.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_peer_that_is_not_the_signed_app_is_closed_before_server_hello() {
    use velvt_service::ipc::peer::{requirement_for, PeerPolicy, PeerProcess, APP_IDENTIFIER};

    if !filesystem_sockets_available() {
        return;
    }
    let socket_dir = SocketDir::new();
    let socket_path = socket_dir.socket_path("unsigned-peer");
    let release_policy = PeerPolicy::Require {
        requirement: requirement_for(APP_IDENTIFIER, Some("ABCDE12345")).unwrap(),
        process: PeerProcess::Id(std::process::id()),
    };
    let transport =
        TokioUnixTransport::new(socket_path.clone(), 3).with_peer_policy(release_policy);
    let server = tokio::spawn(async move { transport.run().await });

    for _ in 0..2 {
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
        let mut read = BufReader::new(stream);
        let mut line = String::new();
        let read_bytes = tokio::time::timeout(Duration::from_secs(5), read.read_line(&mut line))
            .await
            .expect("a refused peer is closed, not left waiting")
            .unwrap_or(0);
        assert_eq!(read_bytes, 0, "a refused peer was sent {line:?}");
    }

    server.abort();
}

/// A refusal ends only that connection: the helper goes on serving. Here a
/// separate process (`/usr/bin/nc`, signed by Apple, not by Velvt, and not the
/// expected process) is refused and sees nothing, then a peer that passes
/// completes the handshake on the same listener.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_refused_process_sees_nothing_and_the_next_peer_is_served() {
    use std::io::Read;
    use velvt_service::ipc::peer::{signing_identity_of_this_process, PeerPolicy, PeerProcess};

    let netcat = std::path::Path::new("/usr/bin/nc");
    if !filesystem_sockets_available() || !netcat.exists() {
        return;
    }
    let socket_dir = SocketDir::new();
    let socket_path = socket_dir.socket_path("refused-then-served");
    let this_binary = signing_identity_of_this_process()
        .unwrap()
        .identifier
        .expect("the test binary is linker-signed");
    let transport =
        TokioUnixTransport::new(socket_path.clone(), 3).with_peer_policy(PeerPolicy::Require {
            requirement: format!("identifier \"{this_binary}\""),
            process: PeerProcess::Id(std::process::id()),
        });
    let server = tokio::spawn(async move { transport.run().await });
    wait_for_socket(&socket_path).await;

    // nc with stdin at EOF stays connected until the other end closes, so its
    // exit is the helper's refusal.
    let mut netcat = std::process::Command::new(netcat)
        .arg("-U")
        .arg(&socket_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while netcat.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            netcat.kill().ok();
            netcat.wait().ok();
            panic!("the refused process was left connected");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut received = String::new();
    netcat
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut received)
        .unwrap();
    assert_eq!(received, "", "a refused process was sent something");

    let writer = connect_and_handshake(&socket_path).await;
    drop(writer);
    server.abort();
}

/// The attack from the review of PR #88, end to end. A process opens a burst
/// of connections, forks a child that keeps them all and reads whatever
/// arrives, and execs a binary that satisfies the code requirement
/// (`/bin/sleep` against `identifier "com.apple.sleep"`). With the Release pin
/// to the helper's parent, every connection is refused before a byte is
/// written, and the child receives nothing.
///
/// Under a signature-only check, whether a connection is served here depends
/// on whether its token is read after the exec: the review's run, with the
/// check inside the accept loop, served 28 of 64. Checked in parallel, nearly
/// every token is read before the exec, so this test alone would rarely catch
/// a regression to signature-only. `ipc::peer`'s
/// `a_peer_that_execs_a_signed_binary_after_connecting_is_refused` reads the
/// token after the exec every time and fails without the pin.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_burst_of_connections_followed_by_fork_and_exec_is_refused() {
    use velvt_service::ipc::peer::{PeerPolicy, PeerProcess};

    if !filesystem_sockets_available() || !zsh_with_sockets() {
        return;
    }
    let socket_dir = SocketDir::new();
    let socket_path = socket_dir.socket_path("fork-exec");
    let received_path = socket_dir.0.join("received");
    let finished_path = socket_dir.0.join("finished");
    let transport =
        TokioUnixTransport::new(socket_path.clone(), 3).with_peer_policy(PeerPolicy::Require {
            requirement: "anchor apple and identifier \"com.apple.sleep\"".to_owned(),
            process: PeerProcess::Parent,
        });
    let server = tokio::spawn(async move { transport.run().await });
    wait_for_socket(&socket_path).await;

    // $1 the socket, $2 where the child writes what it read, $3 written when
    // it has read every connection to its end.
    let mut attacker = std::process::Command::new("/bin/zsh")
        .args([
            "-fc",
            r#"zmodload zsh/net/socket || exit 3
               fds=()
               repeat 16 { zsocket "$1" || exit 4; fds+=($REPLY) }
               ( for fd in $fds; do cat <&$fd; done > "$2"; print > "$3" ) &!
               exec /bin/sleep 10"#,
            "zsh",
        ])
        .arg(&socket_path)
        .arg(&received_path)
        .arg(&finished_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !finished_path.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let finished = finished_path.exists();
    let received = std::fs::read(&received_path).unwrap_or_default();
    attacker.kill().ok();
    attacker.wait().ok();
    server.abort();

    assert!(
        finished,
        "a connection was left open to the forked child (served, not refused)"
    );
    assert_eq!(
        String::from_utf8_lossy(&received),
        "",
        "the forked child was sent something"
    );
}

#[cfg(target_os = "macos")]
async fn wait_for_socket(socket_path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !socket_path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for IPC socket");
}

/// `/bin/zsh` with its socket module, which every supported macOS ships.
#[cfg(target_os = "macos")]
fn zsh_with_sockets() -> bool {
    let available = std::process::Command::new("/bin/zsh")
        .args(["-fc", "zmodload zsh/net/socket"])
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        eprintln!("SKIP: /bin/zsh with zsh/net/socket is not available");
    }
    available
}
