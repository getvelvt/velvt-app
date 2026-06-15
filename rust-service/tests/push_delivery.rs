//! Integration tests for the R7 IPC push delivery layer.
//!
//! Covers: proactive push, queue drain on connect, drop policy, validation
//! gating, slow-client write timeout, reconnect after abrupt disconnect,
//! two independent concurrent clients, privacy alert queued while disconnected,
//! and validation failure isolation between DTO types.

use std::{sync::Arc, time::Duration};

use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
use velvt_service::{
    delivery::{PushAdapter, PushQueue},
    ipc::{serve_connection_with_push_queue, DefaultRouter},
};
use velvt_shared_types::{
    Acknowledged, ClientHello, ClientMessage, ConfidenceLevel, HistoryPayload, InsightPayload,
    PrivacyViolationAlert, ServerMessage, PROTOCOL_VERSION,
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
// Test 1 — Push on cache update: payload enqueued via adapter is delivered
// ---------------------------------------------------------------------------

/// After `PushAdapter::push_history` enqueues a shaped payload, a connecting
/// Swift client must receive exactly that `HistoryPayload` message.
#[tokio::test]
async fn queued_history_payload_is_delivered_to_connected_client() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));
    adapter
        .push_history(HistoryPayload {
            days: 7,
            summaries: vec![],
        })
        .await;

    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue,
        Duration::from_millis(500),
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    let msg = read_message(&mut read).await;
    assert!(
        matches!(&msg, Some(ServerMessage::HistoryPayload(h)) if h.days == 7),
        "expected HistoryPayload(days=7), got {msg:?}"
    );

    drop(write);
    drop(read);
    task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 2 — Queue drain on reconnect: all messages delivered in FIFO order
// ---------------------------------------------------------------------------

/// Five messages queued while the client was offline must all arrive in
/// insertion order as soon as the client reconnects and completes the handshake.
#[tokio::test]
async fn five_queued_messages_drain_in_fifo_order_on_connect() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));
    for i in 1u8..=5 {
        adapter.push_cache_empty(&format!("type_{i}")).await;
    }
    assert_eq!(queue.len().await, 5);

    let (client, server) = duplex(4096);
    let queue_ref = Arc::clone(&queue);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue_ref,
        Duration::from_millis(500),
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    for expected in 1u8..=5 {
        let msg = read_message(&mut read).await.unwrap();
        match msg {
            ServerMessage::CacheEmpty(ce) => {
                assert_eq!(
                    ce.payload_type,
                    format!("type_{expected}"),
                    "messages must be delivered in FIFO order"
                );
            }
            other => panic!("expected CacheEmpty, got {other:?}"),
        }
    }
    assert!(
        queue.is_empty().await,
        "all queued messages must be delivered"
    );

    drop(write);
    drop(read);
    task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 3 — Queue full drop policy: oldest is dropped, newest are delivered
// ---------------------------------------------------------------------------

/// When capacity+1 messages are enqueued, the oldest is dropped.  The client
/// must receive exactly the `capacity` newest messages — not the dropped one.
#[tokio::test]
async fn full_queue_drops_oldest_and_delivers_newest_on_connect() {
    const CAPACITY: usize = 3;
    let queue = PushQueue::new(CAPACITY);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    for i in 1u8..=4 {
        adapter.push_cache_empty(&format!("type_{i}")).await;
    }
    assert_eq!(
        queue.len().await,
        CAPACITY,
        "queue must not exceed capacity"
    );

    let (client, server) = duplex(4096);
    let queue_ref = Arc::clone(&queue);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue_ref,
        Duration::from_millis(500),
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    // Messages 2, 3, 4 remain; message 1 was dropped.
    for expected in 2u8..=4 {
        let msg = read_message(&mut read).await.unwrap();
        match msg {
            ServerMessage::CacheEmpty(ce) => {
                assert_eq!(ce.payload_type, format!("type_{expected}"));
            }
            other => panic!("expected CacheEmpty, got {other:?}"),
        }
    }
    assert!(queue.is_empty().await);

    drop(write);
    drop(read);
    task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 4 — Validation failure: invalid payload is silently dropped
// ---------------------------------------------------------------------------

/// A payload that fails `ValidatePayload::validate_fields` must never reach
/// the transport layer.  Only the subsequent valid message is delivered.
#[tokio::test]
async fn validation_failure_is_silently_dropped_and_never_reaches_client() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    // Empty payload_type fails CacheEmpty's validate_fields — not enqueued.
    adapter.push_cache_empty("").await;
    assert_eq!(
        queue.len().await,
        0,
        "invalid payload must not enter the queue"
    );

    // A valid follow-up message is enqueued and delivered.
    adapter.push_cache_empty("history_payload").await;
    assert_eq!(queue.len().await, 1);

    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue,
        Duration::from_millis(500),
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    let msg = read_message(&mut read).await.unwrap();
    assert!(
        matches!(&msg, ServerMessage::CacheEmpty(ce) if ce.payload_type == "history_payload"),
        "only the valid CacheEmpty must be delivered, got {msg:?}"
    );

    drop(write);
    drop(read);
    task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 5 — Slow client: write timeout closes connection, queue preserved
// ---------------------------------------------------------------------------

/// When the socket write buffer is full and the client does not drain it within
/// `write_timeout`, the connection must be closed.  Messages not yet sent must
/// remain in the push queue so they can be replayed on reconnect.
#[tokio::test]
async fn slow_client_write_timeout_closes_connection_and_queue_is_preserved() {
    // A 48-byte buffer is smaller than one push message (~58 bytes), so the
    // very first write blocks after the handshake.
    let (client, server) = duplex(48);
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    for i in 1u8..=5 {
        adapter.push_cache_empty(&format!("type_{i}")).await;
    }
    assert_eq!(queue.len().await, 5);

    let queue_ref = Arc::clone(&queue);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue_ref,
        Duration::from_millis(5), // very short timeout
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);

    // Complete handshake — client actively reads protocol messages.
    read_message(&mut read).await; // ServerHello
    write_message(
        &mut write,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    read_message(&mut read).await; // Acknowledged

    // Client stops reading; buffer fills up, server write blocks, timeout fires.
    let result = tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("server should disconnect within 500 ms")
        .expect("task should not panic");

    assert!(
        result.is_err(),
        "slow-client write timeout must close the connection with an error"
    );
    assert!(
        !queue.is_empty().await,
        "push queue must retain undelivered messages after slow-client disconnect"
    );

    // read and write are dropped here; server stream is already closed.
}

// ---------------------------------------------------------------------------
// Test 6 — Reconnect after abrupt disconnect: no panic, queue coherent
// ---------------------------------------------------------------------------

/// Client A disconnects without reading any push messages.  The server task
/// must not panic.  A second client connecting to the same queue receives any
/// messages that were enqueued after client A disconnected.
#[tokio::test]
async fn reconnect_after_abrupt_disconnect_no_panic_and_queue_coherent() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    // Pre-load the queue before the first connection.
    for i in 1u8..=3 {
        adapter.push_cache_empty(&format!("pre_{i}")).await;
    }

    // Connection A — client drops its write half after the handshake so the
    // server gets EOF and exits cleanly (no slow-client timeout needed).
    let (client_a, server_a) = duplex(4096);
    let task_a = tokio::spawn(serve_connection_with_push_queue(
        server_a,
        DefaultRouter,
        3,
        None,
        Arc::clone(&queue),
        Duration::from_millis(500),
    ));

    let (read_a, mut write_a) = tokio::io::split(client_a);
    let mut read_a = BufReader::new(read_a);
    assert!(matches!(
        read_message(&mut read_a).await,
        Some(ServerMessage::ServerHello(_))
    ));
    write_message(
        &mut write_a,
        &ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
    )
    .await;
    assert_eq!(
        read_message(&mut read_a).await,
        Some(ServerMessage::Acknowledged(Acknowledged))
    );
    // Drop the write half — server receives EOF on the next read_frame and
    // exits its message loop without panicking.
    drop(write_a);
    drop(read_a);

    let result_a = tokio::time::timeout(Duration::from_secs(1), task_a)
        .await
        .expect("server A must terminate within 1 s")
        .expect("task must not panic");
    let _ = result_a; // Ok or Err is fine; panic is the only failure.

    // Enqueue a sentinel message after the disconnect.
    adapter.push_cache_empty("post_reconnect").await;
    let remaining = queue.len().await;
    assert!(remaining >= 1, "sentinel message must be in queue");

    // Connection B — must drain remaining messages from the same queue.
    let (client_b, server_b) = duplex(4096);
    let task_b = tokio::spawn(serve_connection_with_push_queue(
        server_b,
        DefaultRouter,
        3,
        None,
        Arc::clone(&queue),
        Duration::from_millis(500),
    ));

    let (read_b, mut write_b) = tokio::io::split(client_b);
    let mut read_b = BufReader::new(read_b);
    complete_handshake(&mut read_b, &mut write_b).await;

    for _ in 0..remaining {
        assert!(
            read_message(&mut read_b).await.is_some(),
            "client B must receive all remaining queued messages"
        );
    }
    assert!(
        queue.is_empty().await,
        "queue must be empty after client B drains it"
    );

    drop(write_b);
    drop(read_b);
    task_b.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 7 — Two simultaneous clients with independent queues do not interfere
// ---------------------------------------------------------------------------

/// Two concurrent connections backed by separate queues must each receive
/// exactly their own messages, with no cross-contamination and no panic.
#[tokio::test]
async fn two_concurrent_clients_with_independent_queues_do_not_interfere() {
    let queue_a = PushQueue::new(10);
    let queue_b = PushQueue::new(10);
    let adapter_a = PushAdapter::new(Arc::clone(&queue_a));
    let adapter_b = PushAdapter::new(Arc::clone(&queue_b));

    adapter_a.push_cache_empty("only_for_a").await;
    adapter_a.push_cache_empty("also_for_a").await;
    adapter_b.push_cache_empty("only_for_b").await;

    let (client_a, server_a) = duplex(4096);
    let (client_b, server_b) = duplex(4096);

    let task_a = tokio::spawn(serve_connection_with_push_queue(
        server_a,
        DefaultRouter,
        3,
        None,
        queue_a,
        Duration::from_millis(500),
    ));
    let task_b = tokio::spawn(serve_connection_with_push_queue(
        server_b,
        DefaultRouter,
        3,
        None,
        queue_b,
        Duration::from_millis(500),
    ));

    // Drive both client handshakes and reads concurrently.
    let (msgs_a, msg_b) = tokio::join!(
        async {
            let (read, mut write) = tokio::io::split(client_a);
            let mut read = BufReader::new(read);
            complete_handshake(&mut read, &mut write).await;
            let m1 = read_message(&mut read).await;
            let m2 = read_message(&mut read).await;
            drop(write);
            (m1, m2)
        },
        async {
            let (read, mut write) = tokio::io::split(client_b);
            let mut read = BufReader::new(read);
            complete_handshake(&mut read, &mut write).await;
            let m = read_message(&mut read).await;
            drop(write);
            m
        },
    );

    assert!(
        matches!(&msgs_a.0, Some(ServerMessage::CacheEmpty(ce)) if ce.payload_type == "only_for_a"),
        "client A first message wrong: {:?}",
        msgs_a.0
    );
    assert!(
        matches!(&msgs_a.1, Some(ServerMessage::CacheEmpty(ce)) if ce.payload_type == "also_for_a"),
        "client A second message wrong: {:?}",
        msgs_a.1
    );
    assert!(
        matches!(&msg_b, Some(ServerMessage::CacheEmpty(ce)) if ce.payload_type == "only_for_b"),
        "client B message wrong: {:?}",
        msg_b
    );

    task_a.await.unwrap().unwrap();
    task_b.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 8 — PrivacyViolationAlert queued while disconnected, delivered on reconnect
// ---------------------------------------------------------------------------

/// A `PrivacyViolationAlert` that arrives when no client is connected must
/// enter the queue (urgent, at the front) and be delivered as the first message
/// when a client eventually reconnects.
#[tokio::test]
async fn privacy_alert_queued_while_disconnected_is_delivered_on_reconnect() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    // Enqueue a history payload first so the alert's priority ordering is visible.
    adapter
        .push_history(HistoryPayload {
            days: 7,
            summaries: vec![],
        })
        .await;

    // Alert arrives while no client is connected.
    adapter
        .push_privacy_alert(PrivacyViolationAlert {
            code: "raw_field_rejected".into(),
            message: "safe diagnostic for test".into(),
        })
        .await;

    // Alert is urgent — it should be at the front of the queue.
    assert_eq!(queue.len().await, 2);

    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue,
        Duration::from_millis(500),
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    // First message must be the privacy alert (urgent, prepended to front).
    let first = read_message(&mut read).await;
    assert!(
        matches!(&first, Some(ServerMessage::PrivacyViolationAlert(a)) if a.code == "raw_field_rejected"),
        "privacy alert must be delivered first on reconnect, got {first:?}"
    );

    // Second message is the history payload enqueued before the alert.
    let second = read_message(&mut read).await;
    assert!(
        matches!(&second, Some(ServerMessage::HistoryPayload(h)) if h.days == 7),
        "history payload must follow the alert, got {second:?}"
    );

    drop(write);
    drop(read);
    task.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Test 9 — Validation failure for one DTO type does not affect other types
// ---------------------------------------------------------------------------

/// When a payload fails validation, the push adapter silently drops it.
/// A subsequent valid payload of a *different* type must still be enqueued
/// and delivered, proving the validation failure does not corrupt queue state.
#[tokio::test]
async fn validation_failure_for_one_type_does_not_block_another_type() {
    let queue = PushQueue::new(10);
    let adapter = PushAdapter::new(Arc::clone(&queue));

    // InsightPayload with empty text — validate_fields rejects it.
    adapter
        .push_insight(InsightPayload {
            date: chrono::Utc::now().date_naive(),
            text: String::new(),
            confidence_level: ConfidenceLevel::High,
            low_confidence: false,
            generated_at: chrono::Utc::now(),
        })
        .await;
    assert_eq!(
        queue.len().await,
        0,
        "invalid insight must not enter the queue"
    );

    // A valid HistoryPayload must still be enqueued and delivered.
    adapter
        .push_history(HistoryPayload {
            days: 7,
            summaries: vec![],
        })
        .await;
    assert_eq!(
        queue.len().await,
        1,
        "valid history must be enqueued despite prior failure"
    );

    let (client, server) = duplex(4096);
    let task = tokio::spawn(serve_connection_with_push_queue(
        server,
        DefaultRouter,
        3,
        None,
        queue,
        Duration::from_millis(500),
    ));

    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    complete_handshake(&mut read, &mut write).await;

    let msg = read_message(&mut read).await;
    assert!(
        matches!(&msg, Some(ServerMessage::HistoryPayload(h)) if h.days == 7),
        "history must be delivered after insight validation failure, got {msg:?}"
    );

    drop(write);
    drop(read);
    task.await.unwrap().unwrap();
}
