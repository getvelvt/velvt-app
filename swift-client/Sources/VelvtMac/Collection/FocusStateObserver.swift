import AppKit
import Combine
import Foundation
import Intents
import OSLog
import Security

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
    /// One read of the coarse status: whether it is readable at all, and the
    /// value only when it is.
    func currentReading() -> FocusStatusReading
}

/// What one read of Focus status found. `isFocused` is `nil` unless
/// `availability` is `.available`: a value the app cannot vouch for is not
/// reported at all, so absence of permission produces absence of evidence.
public struct FocusStatusReading: Equatable, Sendable {
    public let availability: FocusStatusAvailability
    public let isFocused: Bool?

    public init(availability: FocusStatusAvailability, isFocused: Bool?) {
        // Readable in principle but no value from the system is unreadable.
        let resolved: FocusStatusAvailability =
            availability == .available && isFocused == nil ? .unavailable : availability
        self.availability = resolved
        self.isFocused = resolved == .available ? isFocused : nil
    }
}

/// The entitlement Apple requires before `INFocusStatusCenter` reads anything
/// real: "Focus Status will only be available to apps that have the
/// Communication Notifications capability" (Apple, developer forums thread
/// 682143). Without it the person can still allow Focus status, and
/// `focusStatus.isFocused` still answers, but `donotdisturbd` refuses the
/// read ("App is missing Communication Notifications entitlement") and the
/// answer is not the person's Focus.
public enum FocusStatusEntitlement {
    public static let communicationNotifications =
        "com.apple.developer.usernotifications.communication"

    /// Whether this process's code signature carries the entitlement. Read
    /// from the signature itself, so an unsigned or ad-hoc build, the test
    /// runner, and a release signed with today's empty `Release.entitlements`
    /// all answer `false`. A claim needs a provisioning profile that
    /// authorizes it, or macOS does not launch the app, so `true` here means
    /// the claim was accepted.
    public static func isPresentInThisProcess() -> Bool {
        guard let task = SecTaskCreateFromSelf(nil) else { return false }
        let value = SecTaskCopyValueForEntitlement(task, communicationNotifications as CFString, nil)
        return (value as? Bool) == true
    }
}

extension FocusStatusAvailability {
    /// The entitlement decides first: without it the status is unreadable
    /// whatever the person answered, so an authorized but unentitled app is
    /// `.unavailable`, not `.available`. Entitled, the person's answer
    /// decides, and a restriction is `.unavailable`.
    public static func resolve(
        entitled: Bool,
        authorization: INFocusStatusAuthorizationStatus
    ) -> FocusStatusAvailability {
        guard entitled else { return .unavailable }
        switch authorization {
        case .authorized: return .available
        case .denied: return .denied
        case .notDetermined: return .notDetermined
        case .restricted: return .unavailable
        @unknown default: return .unavailable
        }
    }
}

/// Production provider backed by `INFocusStatusCenter`.
public struct INFocusStatusProvider: FocusStatusProviding {
    private let entitled: Bool

    public init(entitled: Bool = FocusStatusEntitlement.isPresentInThisProcess()) {
        self.entitled = entitled
    }

    public func currentReading() -> FocusStatusReading {
        // Unentitled, the answer is known without asking the system.
        guard entitled else {
            return FocusStatusReading(availability: .unavailable, isFocused: nil)
        }
        let availability = FocusStatusAvailability.resolve(
            entitled: entitled,
            authorization: INFocusStatusCenter.default.authorizationStatus
        )
        guard availability == .available else {
            return FocusStatusReading(availability: availability, isFocused: nil)
        }
        return FocusStatusReading(
            availability: availability,
            isFocused: INFocusStatusCenter.default.focusStatus.isFocused
        )
    }

    /// The raw values behind a reading, for the launch self-check only.
    /// `isFocused` is read even when the reading would not trust it, because
    /// whether an unentitled build gets a value at all is the question.
    public func selfCheck() -> FocusStatusSelfCheck {
        let authorization = INFocusStatusCenter.default.authorizationStatus
        return FocusStatusSelfCheck(
            authorization: authorization,
            isFocused: INFocusStatusCenter.default.focusStatus.isFocused,
            entitled: entitled,
            availability: FocusStatusAvailability.resolve(
                entitled: entitled,
                authorization: authorization
            )
        )
    }

    /// The single Focus-status authorization ask, invoked only from the
    /// onboarding step. Completion reports whether the person allowed it;
    /// whether the value is then readable also needs the entitlement
    /// (`FocusStatusAvailability.resolve`). Never called twice by Velvt: once
    /// the system has recorded an answer, re-asking is a no-op prompt-free
    /// status read.
    public static func requestAuthorization(_ completion: @escaping (Bool) -> Void) {
        INFocusStatusCenter.default.requestAuthorization { status in
            completion(status == .authorized)
        }
    }
}

// MARK: - FocusStatusSelfCheck

/// One line at launch saying whether this build can read Focus status (1.0.14
/// simulation, B5): the authorization status, the raw `isFocused`, whether the
/// signature carries the Communication Notifications entitlement, and the
/// availability the app reports to the service. Values only, every one a
/// fixed token: no Focus mode name, nothing about the person beyond the one
/// boolean the product already reads.
public struct FocusStatusSelfCheck: Equatable, Sendable {
    public let authorization: INFocusStatusAuthorizationStatus
    public let isFocused: Bool?
    public let entitled: Bool
    public let availability: FocusStatusAvailability

    private static let log = Logger(subsystem: "com.velvt.mac", category: "FocusStatus")

    public init(
        authorization: INFocusStatusAuthorizationStatus,
        isFocused: Bool?,
        entitled: Bool,
        availability: FocusStatusAvailability
    ) {
        self.authorization = authorization
        self.isFocused = isFocused
        self.entitled = entitled
        self.availability = availability
    }

    public var line: String {
        [
            "focus_status_self_check",
            "authorization_status=\(Self.token(for: authorization))",
            "is_focused=\(isFocused.map { $0 ? "true" : "false" } ?? "nil")",
            "communication_notifications_entitlement=\(entitled ? "present" : "absent")",
            "availability=\(availability.rawValue)",
        ].joined(separator: " ")
    }

    /// Written at a persisted level, so `log show --predicate 'subsystem ==
    /// "com.velvt.mac" AND category == "FocusStatus"'` answers after the fact.
    public func log() {
        Self.log.log(level: .default, "\(line, privacy: .public)")
    }

    static func token(for status: INFocusStatusAuthorizationStatus) -> String {
        switch status {
        case .notDetermined: return "not_determined"
        case .restricted: return "restricted"
        case .denied: return "denied"
        case .authorized: return "authorized"
        @unknown default: return "unknown"
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
/// Each sample also reports whether Focus status is readable at all
/// (`focus_status_availability`, proto v36), on every reconnect and whenever
/// it changes, ahead of any edge, so the service can record a decision made
/// while it could not see Focus as `unknown` rather than `off`.
///
/// Failure modes (all fail toward silence, never toward guessing):
/// - Focus status unreadable (no Communication Notifications entitlement,
///   denied, never answered, restricted): only the availability is sent,
///   never an edge. The gate behaves exactly as it does with no Focus
///   evidence, and each decision records `unknown`.
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
    private var lastAvailability: FocusStatusAvailability?
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

    /// Reads the coarse status and reports what changed: the availability
    /// first, then an edge when the status is readable. `forceReport`
    /// re-sends both even when unchanged (used on reconnect); the Rust
    /// service keeps the latest availability and stores edges only, so
    /// repeats are idempotent.
    public func sample(forceReport: Bool) {
        let reading = provider.currentReading()
        if forceReport || reading.availability != lastAvailability {
            lastAvailability = reading.availability
            enqueue(.focusStatusAvailability(.init(availability: reading.availability)))
        }
        guard let active = reading.isFocused else {
            // Unreadable now, so whatever was last reported may be stale by
            // the time it is readable again: report it afresh then.
            lastReported = nil
            return
        }
        guard forceReport || active != lastReported else { return }
        lastReported = active
        enqueue(
            .focusStateChanged(
                .init(
                    active: active,
                    occurredAt: now(),
                    utcOffsetSeconds: utcOffsetSeconds()
                )
            )
        )
    }

    /// Sends in order: an availability ahead of the edge it qualifies.
    private func enqueue(_ message: ClientMessage) {
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

/// Test double with a settable coarse state and availability. A value set
/// with no explicit availability is readable; `nil` is never asked.
public final class FakeFocusStatusProvider: FocusStatusProviding, @unchecked Sendable {
    private let lock = NSLock()
    private var state: Bool?
    private var availability: FocusStatusAvailability?

    public init(initial: Bool? = nil, availability: FocusStatusAvailability? = nil) {
        state = initial
        self.availability = availability
    }

    public func set(_ value: Bool?) {
        lock.withLock { state = value }
    }

    public func setAvailability(_ value: FocusStatusAvailability?) {
        lock.withLock { availability = value }
    }

    public func currentReading() -> FocusStatusReading {
        lock.withLock {
            FocusStatusReading(
                availability: availability ?? (state == nil ? .notDetermined : .available),
                isFocused: state
            )
        }
    }
}
