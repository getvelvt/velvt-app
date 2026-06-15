//! Integration tests for the R7 IPC push delivery layer.
//!
//! Covers: proactive push, queue drain on connect, drop policy, validation
//! gating, and slow-client write timeout.

use std::{sync::Arc, time::Duration};

use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
use velvt_service::{
    delivery::{PushAdapter, PushQueue},
    ipc::{serve_connection_with_push_queue, DefaultRouter},
};
use velvt_shared_types::{
    Acknowledged, ClientHello, ClientMessage, HistoryPayload, ServerMessage, PROTOCOL_VERSION,
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
