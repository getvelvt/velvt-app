import Combine
import Foundation
import UserNotifications

// MARK: - NotificationDeliveryCoordinator

/// Listens for `notificationPayload` server pushes and forwards them to a
/// `NotificationSchedulerProtocol` after a notifications-permission check.
///
/// Denied/restricted/unknown permission status discards the payload
/// silently: no crash, no retry, no re-request, and — since the payload is
/// simply dropped — no logging of its content.
@MainActor
public final class NotificationDeliveryCoordinator {
    private let scheduler: any NotificationSchedulerProtocol
    private let permissionManager: any PermissionManagerProtocol
    private let debounceInterval: Duration
    private var cancellables = Set<AnyCancellable>()

    /// The in-flight debounce/schedule task for each insight date. A new
    /// payload for the same date cancels the previous task before it has a
    /// chance to schedule anything, so a burst of corrected payloads for one
    /// date — arriving one at a time through the IPC listener loop, not
    /// necessarily in the same synchronous tick — still results in at most
    /// one scheduled notification: the most recently received payload.
    private var pendingTasksByDate: [String: Task<Void, Never>] = [:]

    /// The task spawned by the most recently received payload. Exposed so
    /// tests can `await` the permission-check/schedule work deterministically
    /// instead of racing the coordinator's internal `Task`.
    public private(set) var inFlightTask: Task<Void, Never>?

    public init(
        scheduler: any NotificationSchedulerProtocol,
        permissionManager: any PermissionManagerProtocol,
        debounceInterval: Duration = .milliseconds(250)
    ) {
        self.scheduler = scheduler
        self.permissionManager = permissionManager
        self.debounceInterval = debounceInterval
    }

    /// Call once after the IPC client and `AccountStateManager` are ready.
    ///
    /// - Parameter serverMessages: Fan-out relay from
    ///   `AccountStateManager.serverMessages`. This coordinator does not
    ///   consume `incomingMessages` directly.
    public func start(serverMessages: some Publisher<ServerMessage, Never>) {
        serverMessages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in
                guard case .notificationPayload(let payload) = message else { return }
                self?.handle(payload)
            }
            .store(in: &cancellables)
    }

    /// Schedules at most one notification per insight date. A new payload for
    /// a date that already has pending work cancels that work first, so a
    /// burst of corrected payloads for the same date (e.g. Rust re-sending a
    /// corrected insight) settles on scheduling only the most recently
    /// received one, never a flood of one notification per payload.
    @discardableResult
    public func handle(_ payload: NotificationPayload) -> Task<Void, Never> {
        pendingTasksByDate[payload.insightDate]?.cancel()
        let task = Task { @MainActor [weak self, scheduler, permissionManager, debounceInterval] in
            // Briefly wait so a near-simultaneous newer payload for the same
            // date can cancel this task before any scheduling work happens.
            try? await Task.sleep(for: debounceInterval)
            guard !Task.isCancelled else { return }
            let status = await permissionManager.checkStatus(for: .notifications)
            guard status == .granted, !Task.isCancelled else { return }
            await scheduler.schedule(payload)
            self?.pendingTasksByDate.removeValue(forKey: payload.insightDate)
        }
        pendingTasksByDate[payload.insightDate] = task
        inFlightTask = task
        return task
    }

    /// Debug harness used by the menu bar app to exercise the same
    /// notification path as a Rust `notification_payload` IPC push.
    @discardableResult
    public func simulateDebugInsightReceipt(now: Date = Date()) -> Task<Void, Never> {
        let payload = Self.debugNotificationPayload(now: now)
        pendingTasksByDate[payload.insightDate]?.cancel()
        let task = Task { @MainActor [weak self, scheduler, permissionManager, debounceInterval] in
            try? await Task.sleep(for: debounceInterval)
            guard !Task.isCancelled else { return }
            let currentStatus = await permissionManager.checkStatus(for: .notifications)
            let status = currentStatus == .unknown
                ? await permissionManager.requestPermission(for: .notifications)
                : currentStatus
            guard status == .granted, !Task.isCancelled else { return }
            await scheduler.schedule(payload)
            self?.pendingTasksByDate.removeValue(forKey: payload.insightDate)
        }
        pendingTasksByDate[payload.insightDate] = task
        inFlightTask = task
        return task
    }

    private static func debugNotificationPayload(now: Date) -> NotificationPayload {
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyy-MM-dd"
        return NotificationPayload(
            notificationID: UUID(),
            title: "Your Velvt insight is ready",
            body: "You switched away from your document 23 times in 40 minutes.",
            insightDate: formatter.string(from: now),
            doNotDisturbUntil: nil
        )
    }
}

// MARK: - NotificationResponseRouter

/// Routes a tapped notification to "open the popover and scroll to the
/// relevant insight date." Implements `UNUserNotificationCenterDelegate` so
/// it can be installed directly as `UNUserNotificationCenter.current().delegate`.
///
/// `handle(userInfo:)` is the testable core: it takes the already-extracted
/// `userInfo` dictionary rather than a `UNNotificationResponse`, since the
/// latter cannot be constructed in unit tests.
@MainActor
public final class NotificationResponseRouter: NSObject, UNUserNotificationCenterDelegate {
    private let openPopover: () -> Void
    private let scrollToDate: ScrollToDateAction

    public init(openPopover: @escaping () -> Void, scrollToDate: ScrollToDateAction) {
        self.openPopover = openPopover
        self.scrollToDate = scrollToDate
    }

    /// `nonisolated` so it satisfies the (non-isolated) protocol requirement;
    /// hops to the main actor to call the isolated `handle(userInfo:)`.
    nonisolated public func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let userInfo = response.notification.request.content.userInfo
        Task { @MainActor in
            self.handle(userInfo: userInfo)
        }
        completionHandler()
    }

    nonisolated public func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    func handle(userInfo: [AnyHashable: Any]) {
        guard let date = userInfo["insight_date"] as? String else { return }
        openPopover()
        scrollToDate(date)
    }
}
