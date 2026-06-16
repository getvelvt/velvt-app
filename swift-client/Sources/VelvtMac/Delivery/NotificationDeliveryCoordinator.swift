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
    private var cancellables = Set<AnyCancellable>()

    /// The task spawned by the most recently received payload. Exposed so
    /// tests can `await` the permission-check/schedule work deterministically
    /// instead of racing the coordinator's internal `Task`.
    public private(set) var inFlightTask: Task<Void, Never>?

    public init(
        scheduler: any NotificationSchedulerProtocol,
        permissionManager: any PermissionManagerProtocol
    ) {
        self.scheduler = scheduler
        self.permissionManager = permissionManager
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

    @discardableResult
    public func handle(_ payload: NotificationPayload) -> Task<Void, Never> {
        let task = Task { [scheduler, permissionManager] in
            let status = await permissionManager.checkStatus(for: .notifications)
            guard status == .granted else { return }
            await scheduler.schedule(payload)
        }
        inFlightTask = task
        return task
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

    func handle(userInfo: [AnyHashable: Any]) {
        guard let date = userInfo["insight_date"] as? String else { return }
        openPopover()
        scrollToDate(date)
    }
}
