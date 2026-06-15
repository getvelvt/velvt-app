import Foundation

/// Bridges the collection layer and the IPC layer.
///
/// The relay is the sole `EventSink` registered with the collection agent.
/// It forwards every `RawEvent` to the Rust service as a `send_raw_event` IPC
/// message, buffering events in memory while the socket is unavailable.
public protocol EventRelayProtocol: AnyObject, EventSink {
    /// Starts the relay's send loop and connection-status observer.
    func start() async

    /// Cancels internal tasks. Safe to call multiple times.
    func stop() async

    /// Informs the relay of a connection-status change.
    ///
    /// Called internally by the relay's own status observer; also exposed so
    /// tests can simulate reconnects without a live socket.
    func connectionDidChange(to status: ConnectionStatus) async
}

/// Test double that records every event passed to it.
/// Never writes to IPC, disk, or any persistent store.
public final class FakeEventRelay: EventRelayProtocol, @unchecked Sendable {
    private let lock = NSLock()
    private var _receivedEvents: [RawEvent] = []

    /// All events forwarded through `receive(_:)` in arrival order.
    public var receivedEvents: [RawEvent] {
        lock.withLock { _receivedEvents }
    }

    public init() {}

    public func receive(_ event: RawEvent) {
        lock.withLock { _receivedEvents.append(event) }
    }

    public func start() async {}
    public func stop() async {}
    public func connectionDidChange(to status: ConnectionStatus) async {}
}
