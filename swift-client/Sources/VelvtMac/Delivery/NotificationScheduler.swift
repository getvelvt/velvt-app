import Foundation
import UserNotifications

// MARK: - NotificationSchedulerProtocol

/// Schedules and cancels user notifications. Implementations must not retain
/// or log notification content (`title`/`body`) beyond the scope of building
/// the `UNNotificationRequest`.
public protocol NotificationSchedulerProtocol: AnyObject {
    @discardableResult
    func schedule(_ payload: NotificationPayload) async -> Bool
    func cancelAll()
}

// MARK: - InterventionNotificationScheduling

/// Delivers a live drift offer as an OS notification.
///
/// Deliberately separate from `NotificationSchedulerProtocol`: an insight
/// notification carries an `insightDate` and a do-not-disturb window, and a
/// drift offer has neither. Keeping the two seams apart means an intervention
/// cannot accidentally be scheduled with insight semantics, and existing
/// conformers of the insight protocol are untouched.
///
/// The copy is authored in Rust and passed through verbatim. Implementations
/// must not retain or log `title`/`body` beyond building the request.
public protocol InterventionNotificationScheduling: AnyObject {
    @discardableResult
    func scheduleIntervention(id: String, title: String, body: String) async -> Bool
}

/// `userInfo` marker identifying a notification as a drift offer, so the
/// delegate opens the popover where the reply buttons live instead of trying
/// to scroll to an insight date that does not exist.
public let interventionNotificationUserInfoKey = "velvt_intervention"

// MARK: - UNUserNotificationCenterProtocol

/// Narrow seam over `UNUserNotificationCenter` so scheduling can be tested
/// without touching the real notification system (no permission prompts, no
/// entitlement requirements in CI).
public protocol UNUserNotificationCenterProtocol: AnyObject {
    func add(_ request: UNNotificationRequest) async throws
    func removeAllPendingNotificationRequests()
}

extension UNUserNotificationCenter: UNUserNotificationCenterProtocol {}

// MARK: - UNNotificationScheduler

/// Concrete `NotificationSchedulerProtocol` backed by `UNUserNotificationCenter`.
///
/// Translates `NotificationPayload.doNotDisturbUntil` into a
/// `UNTimeIntervalNotificationTrigger` for the remaining interval when it is
/// in the future; otherwise schedules immediately (`trigger == nil`).
///
/// Callers are responsible for the notifications-permission check — this
/// type only schedules. `content` and `payload` fall out of scope once the
/// request is built; nothing is cached or logged.
public final class UNNotificationScheduler: NotificationSchedulerProtocol {
    private let center: any UNUserNotificationCenterProtocol
    private let now: () -> Date
    private let metrics: (any AppMetricsCounting)?

    public init(
        center: any UNUserNotificationCenterProtocol = UNUserNotificationCenter.current(),
        now: @escaping () -> Date = Date.init,
        metrics: (any AppMetricsCounting)? = nil
    ) {
        self.center = center
        self.now = now
        self.metrics = metrics
    }

    @discardableResult
    public func schedule(_ payload: NotificationPayload) async -> Bool {
        let content = UNMutableNotificationContent()
        content.title = payload.title
        content.body = payload.body
        content.userInfo = ["insight_date": payload.insightDate]
        content.sound = .default

        let trigger: UNNotificationTrigger? = {
            guard let until = payload.doNotDisturbUntil else { return nil }
            let remaining = until.timeIntervalSince(now())
            guard remaining > 0 else { return nil }
            return UNTimeIntervalNotificationTrigger(timeInterval: remaining, repeats: false)
        }()

        let request = UNNotificationRequest(
            identifier: payload.notificationID.uuidString,
            content: content,
            trigger: trigger
        )
        do {
            try await center.add(request)
            metrics?.incrementInterventions()
            return true
        } catch {
            return false
        }
    }

    public func cancelAll() {
        center.removeAllPendingNotificationRequests()
    }
}

extension UNNotificationScheduler: InterventionNotificationScheduling {
    /// Delivers immediately: a drift offer is only true at the moment the
    /// evidence was gathered, so a trigger that fired it later would be a
    /// claim about a moment that has passed.
    @discardableResult
    public func scheduleIntervention(id: String, title: String, body: String) async -> Bool {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = [interventionNotificationUserInfoKey: true]
        content.sound = .default

        let request = UNNotificationRequest(
            identifier: id,
            content: content,
            trigger: nil
        )
        do {
            try await center.add(request)
            metrics?.incrementInterventions()
            return true
        } catch {
            return false
        }
    }
}

// MARK: - FakeNotificationScheduler

/// Test double recording scheduled payloads without touching
/// `UNUserNotificationCenter`.
public final class FakeNotificationScheduler: NotificationSchedulerProtocol, @unchecked Sendable {
    public private(set) var scheduledPayloads: [NotificationPayload] = []
    public private(set) var cancelAllCallCount = 0
    fileprivate var scheduledInterventionsStorage: [ScheduledIntervention] = []
    fileprivate let lock = NSLock()

    public init() {}

    @discardableResult
    public func schedule(_ payload: NotificationPayload) async -> Bool {
        lock.withLock { scheduledPayloads.append(payload) }
        return true
    }

    public func cancelAll() {
        lock.withLock { cancelAllCallCount += 1 }
    }
}

extension FakeNotificationScheduler: InterventionNotificationScheduling {
    public struct ScheduledIntervention: Equatable, Sendable {
        public let id: String
        public let title: String
        public let body: String
    }

    @discardableResult
    public func scheduleIntervention(id: String, title: String, body: String) async -> Bool {
        lock.withLock {
            scheduledInterventionsStorage.append(
                ScheduledIntervention(id: id, title: title, body: body))
        }
        return true
    }

    public var scheduledInterventions: [ScheduledIntervention] {
        lock.withLock { scheduledInterventionsStorage }
    }
}

// MARK: - FakeUNUserNotificationCenter

/// Test double recording requests passed to `add(_:)` without touching the
/// real notification system.
public final class FakeUNUserNotificationCenter: UNUserNotificationCenterProtocol, @unchecked Sendable {
    public private(set) var addedRequests: [UNNotificationRequest] = []
    public private(set) var removeAllCallCount = 0
    private let lock = NSLock()

    public init() {}

    public func add(_ request: UNNotificationRequest) async throws {
        lock.withLock { addedRequests.append(request) }
    }

    public func removeAllPendingNotificationRequests() {
        lock.withLock { removeAllCallCount += 1 }
    }
}
