import Foundation
import UserNotifications

// MARK: - NotificationSchedulerProtocol

/// Schedules and cancels user notifications. Implementations must not retain
/// or log notification content (`title`/`body`) beyond the scope of building
/// the `UNNotificationRequest`.
public protocol NotificationSchedulerProtocol: AnyObject {
    func schedule(_ payload: NotificationPayload) async
    func cancelAll()
}

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

    public init(
        center: any UNUserNotificationCenterProtocol = UNUserNotificationCenter.current(),
        now: @escaping () -> Date = Date.init
    ) {
        self.center = center
        self.now = now
    }

    public func schedule(_ payload: NotificationPayload) async {
        let content = UNMutableNotificationContent()
        content.title = payload.title
        content.body = payload.body
        content.userInfo = ["insight_date": payload.insightDate]

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
        try? await center.add(request)
    }

    public func cancelAll() {
        center.removeAllPendingNotificationRequests()
    }
}

// MARK: - FakeNotificationScheduler

/// Test double recording scheduled payloads without touching
/// `UNUserNotificationCenter`.
public final class FakeNotificationScheduler: NotificationSchedulerProtocol, @unchecked Sendable {
    public private(set) var scheduledPayloads: [NotificationPayload] = []
    public private(set) var cancelAllCallCount = 0
    private let lock = NSLock()

    public init() {}

    public func schedule(_ payload: NotificationPayload) async {
        lock.withLock { scheduledPayloads.append(payload) }
    }

    public func cancelAll() {
        lock.withLock { cancelAllCallCount += 1 }
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
