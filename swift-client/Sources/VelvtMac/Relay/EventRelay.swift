import Foundation
import os.log

// MARK: - CircularBuffer

/// Fixed-capacity FIFO ring buffer with drop-oldest-on-overflow semantics.
///
/// All operations are O(1) with no heap allocation after initialisation.
/// Not thread-safe; callers must ensure exclusive access (e.g. via actor isolation).
struct CircularBuffer<Element> {
    private var storage: [Element?]
    private var head = 0
    private var tail = 0
    private(set) var count = 0
    let capacity: Int

    init(capacity: Int) {
        precondition(capacity > 0, "CircularBuffer capacity must be positive")
        self.capacity = capacity
        storage = Array(repeating: nil, count: capacity)
    }

    var isEmpty: Bool { count == 0 }
    var isFull: Bool { count == capacity }

    /// Appends `element` at the tail. Caller must ensure `!isFull`.
    mutating func enqueue(_ element: Element) {
        storage[tail] = element
        tail = (tail + 1) % capacity
        count += 1
    }

    /// Removes and returns the oldest element, or `nil` when empty.
    mutating func dequeue() -> Element? {
        guard !isEmpty else { return nil }
        let element = storage[head]
        storage[head] = nil
        head = (head + 1) % capacity
        count -= 1
        return element
    }

    /// Silently discards the oldest element to make room for a new one.
    /// Caller must ensure `!isEmpty`.
    mutating func dropOldest() {
        guard !isEmpty else { return }
        storage[head] = nil
        head = (head + 1) % capacity
        count -= 1
    }
}

// MARK: - EventRelay

private let logger = Logger(subsystem: "com.velvt.mac", category: "EventRelay")

/// Routes `RawEvent`s from the collection agent to the Rust service over IPC.
///
/// **Threading model**
/// `receive(_:)` is `nonisolated` and may be called from any thread (the AX
/// callback dispatch queue). It writes into an `AsyncStream` continuation guarded
/// by an `NSLock` and returns immediately — O(1), never blocking. All other state
/// is actor-isolated.
///
/// **Stream lifecycle**
/// A fresh `AsyncStream` is created on each `start()` call and finished (not
/// cancelled) by `stop()`. This prevents `AsyncStream`'s internal cancellation
/// path from permanently marking the stream terminal, which would prevent a
/// subsequent `start()` from consuming new events.
///
/// **Buffer policy**
/// While the IPC socket is unavailable, incoming events are held in a bounded
/// in-memory ring buffer (`capacity`, default 500). When the buffer is full the
/// oldest event is dropped and `droppedEventCount` is incremented. Nothing is
/// ever written to disk; events that overflow the buffer are permanently lost.
///
/// **Reconnect sequence**
/// On reconnect the relay logs a structured count-only metric, then flushes all
/// buffered events in chronological order before forwarding new ones.
public actor EventRelay: EventRelayProtocol {

    // MARK: Ingest channel
    // Recreated per start/stop cycle. All accesses from nonisolated `receive(_:)`
    // are serialised through `continuationLock`.
    // `nonisolated(unsafe)` is safe because `continuationLock` provides the
    // required mutual exclusion.
    private let continuationLock = NSLock()
    private nonisolated(unsafe) var _ingestContinuation: AsyncStream<RawEvent>.Continuation?
    private let startupBufferLock = NSLock()
    private nonisolated(unsafe) var startupBuffer: [RawEvent] = []
    private nonisolated(unsafe) var startupDroppedEventCount = 0
    private nonisolated(unsafe) var acceptsStartupEvents = true

    private let ipcClient: any IPCClientProtocol
    private let capacity: Int
    private let metrics: (any AppMetricsCounting)?

    // MARK: Actor-isolated state

    private var ringBuffer: CircularBuffer<RawEvent>

    /// Number of events dropped due to buffer overflow since the last reconnect.
    public private(set) var droppedEventCount = 0

    /// Number of events currently held in the ring buffer.
    public var bufferedEventCount: Int { ringBuffer.count }

    private var isConnected = false
    private var isFlushing = false
    private var sendLoopTask: Task<Void, Never>?
    private var statusObserverTask: Task<Void, Never>?

    // MARK: Init

    public init(
        ipcClient: any IPCClientProtocol,
        capacity: Int = 500,
        metrics: (any AppMetricsCounting)? = nil
    ) {
        self.ipcClient = ipcClient
        self.capacity = capacity
        self.metrics = metrics
        ringBuffer = CircularBuffer(capacity: capacity)
    }

    // MARK: EventSink — nonisolated, O(1), never blocks

    public nonisolated func receive(_ event: RawEvent) {
        metrics?.incrementActionsLogged()
        let didYield = continuationLock.withLock { () -> Bool in
            guard let continuation = _ingestContinuation else {
                return false
            }
            _ = continuation.yield(event)
            return true
        }
        guard !didYield else {
            return
        }
        startupBufferLock.withLock {
            guard acceptsStartupEvents else {
                return
            }
            if startupBuffer.count >= capacity {
                startupBuffer.removeFirst()
                startupDroppedEventCount += 1
            }
            startupBuffer.append(event)
        }
    }

    private nonisolated func drainStartupBuffer() -> (events: [RawEvent], dropped: Int) {
        startupBufferLock.withLock {
            let events = startupBuffer
            let dropped = startupDroppedEventCount
            startupBuffer.removeAll(keepingCapacity: true)
            startupDroppedEventCount = 0
            return (events, dropped)
        }
    }

    private nonisolated func clearStartupBuffer() {
        startupBufferLock.withLock {
            startupBuffer.removeAll(keepingCapacity: true)
            startupDroppedEventCount = 0
            acceptsStartupEvents = false
        }
    }

    private nonisolated func enqueue(_ events: [RawEvent], into continuation: AsyncStream<RawEvent>.Continuation) {
        for event in events {
            _ = continuation.yield(event)
        }
    }

    private nonisolated func installContinuation(_ continuation: AsyncStream<RawEvent>.Continuation) {
        continuationLock.withLock {
            _ingestContinuation = continuation
        }
        startupBufferLock.withLock {
            acceptsStartupEvents = false
        }
    }

    // MARK: EventRelayProtocol

    public func start() async {
        guard sendLoopTask == nil else { return }

        var cont: AsyncStream<RawEvent>.Continuation!
        let stream = AsyncStream<RawEvent> { cont = $0 }
        installContinuation(cont)
        let startup = drainStartupBuffer()
        droppedEventCount += startup.dropped
        enqueue(startup.events, into: cont)

        sendLoopTask = Task { await runSendLoop(stream: stream) }
        statusObserverTask = Task { await observeConnectionStatus() }
    }

    public func stop() async {
        // Finish the stream cleanly so the send loop exits via normal loop
        // termination. Using Task.cancel() would trigger AsyncStream's internal
        // cancellation path, which permanently marks the stream storage terminal
        // and prevents a new iterator from working after restart.
        continuationLock.withLock {
            _ingestContinuation?.finish()
            _ingestContinuation = nil
        }
        clearStartupBuffer()
        statusObserverTask?.cancel()
        // Wait for both tasks to fully exit before clearing refs, so a
        // subsequent start() always begins with a clean slate.
        await sendLoopTask?.value
        await statusObserverTask?.value
        sendLoopTask = nil
        statusObserverTask = nil
    }

    public func connectionDidChange(to status: ConnectionStatus) async {
        switch status {
        case .connected:
            isConnected = true
            isFlushing = true
            let buffered = ringBuffer.count
            let dropped = droppedEventCount
            if buffered > 0 || dropped > 0 {
                logger.info(
                    "Relay flushing \(buffered) buffered events; dropped \(dropped) events since disconnect."
                )
            }
            droppedEventCount = 0
            await flushBuffer()
            isFlushing = false
        default:
            isConnected = false
        }
    }

    // MARK: Private

    private func runSendLoop(stream: AsyncStream<RawEvent>) async {
        for await event in stream {
            if isConnected && !isFlushing {
                try? await ipcClient.send(.rawEvent(toMessage(event)))
            } else {
                bufferEvent(event)
            }
        }
    }

    private func observeConnectionStatus() async {
        // Use a TaskGroup so each status delivery becomes an independent actor
        // task. This lets `connectionDidChange(.disconnected)` reach the actor
        // at the next suspension point while a flush is in progress, rather
        // than being blocked until the flush completes.
        //
        // The group's implicit wait-for-all on exit ensures stop() correctly
        // drains in-flight connectionDidChange calls before returning.
        await withTaskGroup(of: Void.self) { group in
            for await status in ipcClient.connectionStatus.values {
                if Task.isCancelled { break }
                group.addTask { await self.connectionDidChange(to: status) }
            }
        }
    }

    /// Drains the ring buffer, sending each event to the IPC client.
    ///
    /// The `isConnected` guard is checked at the top of every iteration so that
    /// a mid-flush disconnect (detected when `connectionDidChange(.disconnected)`
    /// runs on the actor while this method is suspended at `await ipcClient.send`)
    /// stops the drain immediately. Unprocessed events remain at the head of the
    /// ring buffer and are flushed on the next reconnect in chronological order.
    ///
    /// Events arriving via `receive(_:)` during a flush are sent to `bufferEvent`
    /// by the send loop (since `isFlushing == true`) and appended to the tail,
    /// so they are dequeued after the pre-existing backlog — preserving order.
    private func flushBuffer() async {
        while isConnected && !Task.isCancelled, let event = ringBuffer.dequeue() {
            try? await ipcClient.send(.rawEvent(toMessage(event)))
        }
    }

    private func bufferEvent(_ event: RawEvent) {
        if ringBuffer.isFull {
            ringBuffer.dropOldest()
            droppedEventCount += 1
        }
        ringBuffer.enqueue(event)
    }

    private nonisolated func toMessage(_ event: RawEvent) -> RawEventMessage {
        RawEventMessage(
            eventID: UUID(),
            occurredAt: event.occurredAt,
            appName: event.appName,
            windowTitle: event.windowTitle,
            bundleID: nil
        )
    }
}
