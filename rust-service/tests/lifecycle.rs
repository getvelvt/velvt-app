//! Integration tests for R8 lifecycle features.
//!
//! Covers:
//! - Graceful shutdown: `ShuttingDown` is delivered to the client before socket close.
//! - Reconnect window: reconnect within window → same push queue; after window → fresh queue.

use std::{sync::Arc, time::Duration};

use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
use velvt_service::{
    delivery::{PushAdapter, PushQueue},
    ipc::{serve_connection_with_push_queue_and_shutdown, DefaultRouter, ReconnectTracker},
    lifecycle::CancellationToken,
};
use velvt_shared_types::{
    Acknowledged, ClientHello, ClientMessage, ServerMessage, PROTOCOL_VERSION,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 1 — ShuttingDown is delivered before socket close during graceful shutdown
// ---------------------------------------------------------------------------

/// The caller enqueues `ShuttingDown` via `push_shutting_down()` BEFORE firing
/// the shutdown receiver.  The connection task must flush the queue (delivering
/// the `ShuttingDown` message) before returning `Ok(())`.
#[tokio::test]
async fn shutting_down_is_delivered_before_socket_close() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    let (client, server) = duplex(4096);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let task = tokio::spawn(serve_connection_with_push_queue_and_shutdown(
        server,
        DefaultRouter,
        3,
        None,
        Arc::clone(&queue),
        Duration::from_millis(500),
        shutdown_rx,
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    // Enqueue ShuttingDown BEFORE signalling shutdown (the required ordering).
    adapter.push_shutting_down("sigterm").await;
    shutdown_tx.send_replace(true);

    // The connection task must deliver ShuttingDown then close.
    let msg = read_message(&mut read).await;
    assert!(
        matches!(&msg, Some(ServerMessage::ShuttingDown(s)) if s.reason == "sigterm"),
        "ShuttingDown must be delivered before socket close, got {msg:?}"
    );

    // Verify the connection closed after flushing.
    let eof = read_message(&mut read).await;
    assert!(eof.is_none(), "connection must close after ShuttingDown");

    task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 2 — Reconnect within window reuses the same push queue
// ---------------------------------------------------------------------------

/// If a client reconnects before the reconnect window expires, `acquire()`
/// must return the same `Arc<PushQueue>` as the previous connection.  Any
/// messages buffered during the disconnect survive on the same queue.
#[tokio::test]
async fn reconnect_within_window_reuses_push_queue() {
    // 200ms window, tiny capacity.
    let tracker = ReconnectTracker::new(Duration::from_millis(200), 16);

    let q1 = tracker.acquire();
    tracker.release();

    // Reconnect before the window elapses.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let q2 = tracker.acquire();

    assert!(
        Arc::ptr_eq(&q1, &q2),
        "same Arc<PushQueue> must be returned within the reconnect window"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Reconnect after window expiry gets a fresh push queue
// ---------------------------------------------------------------------------

/// After the reconnect window elapses without a reconnect, `acquire()` must
/// return a new `Arc<PushQueue>` — distinct from the one held before.
#[tokio::test]
async fn reconnect_after_window_gets_fresh_push_queue() {
    // Very short window so the test stays fast.
    let tracker = ReconnectTracker::new(Duration::from_millis(50), 16);

    let q1 = tracker.acquire();
    tracker.release();

    // Wait for the window to expire.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let q2 = tracker.acquire();

    assert!(
        !Arc::ptr_eq(&q1, &q2),
        "a fresh Arc<PushQueue> must be returned after the reconnect window expires"
    );
    assert!(q2.is_empty().await, "fresh queue must start empty");
}

// ---------------------------------------------------------------------------
// Test 4 — Double cancel (SIGTERM received twice) does not panic
// ---------------------------------------------------------------------------

/// Simulates SIGTERM received twice: `cancel()` called twice with active
/// subscribers.  Must be idempotent — no panic, no double-free.
#[test]
fn double_cancel_with_active_subscribers_does_not_panic() {
    let token = CancellationToken::new();
    let _rx1 = token.subscribe();
    let _rx2 = token.subscribe();

    token.cancel();
    token.cancel(); // second SIGTERM — must not panic or double-free

    assert!(token.is_cancelled());
}

// ---------------------------------------------------------------------------
// Test 5 — Shutdown deadline fires when tasks do not stop in time
// ---------------------------------------------------------------------------

/// Verifies the `tokio::time::timeout(deadline, join_all)` pattern used in
/// main.rs: when a task blocks indefinitely, the timeout must fire within the
/// deadline and the overall shutdown must still complete cleanly.
#[tokio::test]
async fn shutdown_deadline_fires_when_tasks_are_slow() {
    let (drop_tx, drop_rx) = tokio::sync::oneshot::channel::<()>();

    let slow_task = tokio::spawn(async move {
        let _ = drop_rx.await; // waits until the sender is dropped
    });

    let deadline = Duration::from_millis(30);
    let result = tokio::time::timeout(deadline, slow_task).await;

    assert!(
        result.is_err(),
        "timeout must fire when the task does not stop within the deadline"
    );

    // Let the task clean up after the test ends.
    drop(drop_tx);
}
