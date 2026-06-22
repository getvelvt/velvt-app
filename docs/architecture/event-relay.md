# Event Relay

## Scope

The event relay sits between the collection agent and the IPC layer. It:

1. Receives every `RawEvent` from `AXCollectionAgent` via `EventSink.receive(_:)`.
2. Forwards each event to the Rust service as a `raw_event` IPC message.
3. Buffers events in memory while the IPC socket is unavailable.
4. Flushes buffered events in chronological order when the socket reconnects.

The relay has no abstraction, persistence, filtering, or network responsibilities.
Raw app names and window titles must never appear in any log statement.

## Ring Buffer

While the IPC socket is unavailable, incoming events are held in a bounded
in-memory `CircularBuffer<RawEvent>`. The default capacity is 500 events.

### Drop policy

When the buffer is full and a new event arrives:

1. The **oldest** event (head of the buffer) is silently discarded.
2. `droppedEventCount` is incremented.
3. The new event is appended at the tail.

This preserves the most-recent activity window at the cost of the oldest events.
Events that overflow the buffer are permanently lost — nothing is written to disk,
SQLite, `UserDefaults`, or any other persistent store.

### Capacity invariant

The capacity is set at initialisation and never changes. All buffer operations
are O(1): no heap allocation, no copying, no resizing.

## Threading Model

`EventSink.receive(_:)` is declared `nonisolated` and may be called from any
thread (typically the AX callback dispatch queue). It holds an `NSLock` only for
the duration of a single `AsyncStream.Continuation.yield` call — O(1) and never
blocking. All other relay state is actor-isolated.

Two actor tasks run for the lifetime of a started relay:

| Task | Role |
|---|---|
| **Send loop** | Drains the `AsyncStream` fed by `receive(_:)`. Forwards events to IPC when connected; buffers them when not. |
| **Status observer** | Watches `IPCClientProtocol.connectionStatus`. Dispatches each status change as an independent actor task via `withTaskGroup`, so a mid-flush disconnect can reach the actor at the next suspension point without waiting for the flush to complete. |

## Reconnect Sequence

When `connectionDidChange(to: .connected)` is called:

1. `isConnected = true`, `isFlushing = true`.
2. A single structured log line is emitted — counts only, no raw event content:
   ```
   Relay flushing N buffered events; dropped M events since disconnect.
   ```
   (Omitted if both counts are zero.)
3. `droppedEventCount` resets to zero.
4. `flushBuffer()` drains the ring buffer in FIFO order, sending each event to
   the IPC client.
5. `isFlushing = false`.

The send loop checks `isFlushing` before forwarding new events. While flushing,
new events from `receive(_:)` are appended to the ring buffer's tail, so they
are picked up after the pre-existing backlog — chronological order is preserved.

### Mid-flush disconnect

`flushBuffer()` checks `isConnected` at the top of each iteration:

```
while isConnected && !Task.isCancelled, let event = ringBuffer.dequeue()
```

If `connectionDidChange(to: .disconnected)` is delivered while a flush is in
progress, `isConnected` is set to `false` on the actor at the next suspension
point inside the flush. On the following iteration the guard fails, the drain
stops, and all unprocessed events remain at the head of the ring buffer. The
next reconnect resumes the flush from that position.

At most one event may be lost at the disconnect boundary: the event whose
`await ipcClient.send()` was in flight when the guard was last checked. Events
that were not yet dequeued are never lost.

## Stream Lifecycle

A fresh `AsyncStream` is created on each `start()` call and terminated (via
`continuation.finish()`) by `stop()`. The continuation is cleared under
`NSLock` before `finish()` is called, so `receive(_:)` calls that arrive after
`stop()` are no-ops rather than writes to a finished stream.

`finish()` is used instead of `Task.cancel()` because Swift's `AsyncStream`
marks its internal storage terminal when the consuming task is cancelled, which
would prevent a subsequent `start()` call from receiving events on the same
relay instance.

## Start / Stop Idempotency

- `start()` is guarded by `sendLoopTask == nil`; calling it twice is a no-op.
- `stop()` awaits both the send loop and the status observer (including any
  in-flight `connectionDidChange` calls managed by the task group) before
  returning. Calling `stop()` twice is safe.

After `stop()`, `receive(_:)` is a no-op (continuation is nil). Events sent
while the relay is stopped are silently dropped and are not buffered.

## Privacy Invariants

- Raw `appName` and `windowTitle` values never appear in any log statement.
  Only integer counts (`bufferedEventCount`, `droppedEventCount`) are logged.
- The ring buffer is heap-only (`[Element?]` array). No data is written to disk
  or any persistent store at any point.
- `RawEventMessage` is constructed from `RawEvent` fields at send time;
  `bundleID` is always `nil` because the collection agent does not capture it.

## Testability

`EventRelayProtocol` exposes `start()`, `stop()`, and `connectionDidChange(to:)`.
All production code depends on the protocol, never on the concrete actor.

`FakeEventRelay` is the test double used in collection-agent and coordinator
tests; it records every received event behind an `NSLock` and has no-op
lifecycle methods.

`FakeIPCClient` provides a `setConnectionStatus(_:)` method to simulate
connect/disconnect transitions in tests without a live socket.
