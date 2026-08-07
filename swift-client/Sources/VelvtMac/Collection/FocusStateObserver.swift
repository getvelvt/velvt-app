import AppKit
import Combine
import Foundation
import Intents

// MARK: - FocusStatusProviding

/// Narrow seam over the system Focus/DND status so observation can be tested
/// without Intents entitlements or a real Focus configuration.
///
/// The chosen observation API is `INFocusStatusCenter` (Intents framework):
/// after a one-time user authorization it exposes exactly one coarse boolean,
/// `focusStatus.isFocused`. The Focus mode's name, configuration, and
/// schedule are not readable through this API at all, which is precisely the
/// granularity the privacy rule wants.
public protocol FocusStatusProviding {
    /// The current coarse Focus state, or `nil` when the app is not
    /// authorized for Focus status (or the system cannot answer). `nil`
    /// reports nothing — absence of permission produces absence of evidence.
    func currentFocusActive() -> Bool?
}

/// Production provider backed by `INFocusStatusCenter`.
public struct INFocusStatusProvider: FocusStatusProviding {
    public init() {}

    public func currentFocusActive() -> Bool? {
        guard INFocusStatusCenter.default.authorizationStatus == .authorized else {
            return nil
        }
        return INFocusStatusCenter.default.focusStatus.isFocused
    }

    /// The single Focus-status authorization ask, invoked only from the
    /// onboarding step. Completion reports whether the coarse boolean is
    /// readable afterward. Never called twice by Velvt: once the system has
    /// recorded an answer, re-asking is a no-op prompt-free status read.
    public static func requestAuthorization(_ completion: @escaping (Bool) -> Void) {
        INFocusStatusCenter.default.requestAuthorization { status in
            completion(status == .authorized)
        }
    }
}

// MARK: - FocusStateObserver

/// Observes the system Focus/DND state and reports coarse transitions to the
/// Rust service over IPC.
///
/// Swift observes only: this type samples one authorized boolean at existing
/// event boundaries — app activations, wake from sleep, and IPC reconnects —
/// and sends an edge when it changes. There is no polling timer, no reading
/// of Focus configuration, and no decision-making here; the Rust service
/// owns the evidence record, bucketing, and every decision derived from it.
///
/// Failure modes (all fail toward silence, never toward guessing):
/// - Focus-status authorization missing or denied: the provider returns
///   `nil` and nothing is ever sent. The product behaves identically.
/// - Sampling is event-driven, so a transition is observed at the next
///   activation/wake/reconnect rather than the instant it happens. Rust
///   buckets transition times to a coarse granularity anyway.
/// - IPC unavailable: the send fails silently and the state is re-reported
///   on the next reconnect (reconnects always force a report so the service
///   knows the current state after either side restarts).
@MainActor
public final class FocusStateObserver {
    private let ipcClient: any IPCClientProtocol
    private let provider: any FocusStatusProviding
    private let now: () -> Date
    private let utcOffsetSeconds: () -> Int
    private var lastReported: Bool?
    private var cancellables = Set<AnyCancellable>()
    private var sendChain: Task<Void, Never>?

    /// The most recent send task, exposed so tests can await delivery
    /// deterministically.
    public private(set) var inFlightTask: Task<Void, Never>?

    public init(
        ipcClient: any IPCClientProtocol,
        provider: any FocusStatusProviding = INFocusStatusProvider(),
        now: @escaping () -> Date = Date.init,
        utcOffsetSeconds: @escaping () -> Int = { TimeZone.current.secondsFromGMT() }
    ) {
        self.ipcClient = ipcClient
        self.provider = provider
        self.now = now
        self.utcOffsetSeconds = utcOffsetSeconds
    }

    public func start(
        connectionStatus: some Publisher<ConnectionStatus, Never>,
        workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter
    ) {
        connectionStatus
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                guard status == .connected else { return }
                self?.sample(forceReport: true)
            }
            .store(in: &cancellables)
        workspaceNotifications.publisher(for: NSWorkspace.didActivateApplicationNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.sample(forceReport: false) }
            .store(in: &cancellables)
        workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.sample(forceReport: false) }
            .store(in: &cancellables)
    }

    /// Reads the coarse boolean and reports an edge. `forceReport` re-sends
    /// the current state even when unchanged (used on reconnect); the Rust
    /// service stores edges only, so repeats are idempotent.
    public func sample(forceReport: Bool) {
        guard let active = provider.currentFocusActive() else { return }
        guard forceReport || active != lastReported else { return }
        lastReported = active
        let message = ClientMessage.focusStateChanged(
            .init(
                active: active,
                occurredAt: now(),
                utcOffsetSeconds: utcOffsetSeconds()
            )
        )
        let previous = sendChain
        let task = Task { [ipcClient] in
            await previous?.value
            // Failure is silent by design: the next reconnect force-reports.
            try? await ipcClient.send(message)
        }
        sendChain = task
        inFlightTask = task
    }
}

// MARK: - FakeFocusStatusProvider

/// Test double with a settable coarse state.
public final class FakeFocusStatusProvider: FocusStatusProviding, @unchecked Sendable {
    private let lock = NSLock()
    private var state: Bool?

    public init(initial: Bool? = nil) {
        state = initial
    }

    public func set(_ value: Bool?) {
        lock.withLock { state = value }
    }

    public func currentFocusActive() -> Bool? {
        lock.withLock { state }
    }
}
