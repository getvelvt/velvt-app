import AppKit
import ApplicationServices
import Combine
import Foundation
import UserNotifications

public enum PermissionType: CaseIterable, Hashable, Sendable {
    case accessibility
    case notifications
}

public enum PermissionStatus: Equatable, Sendable {
    case unknown
    case granted
    case denied
    case restricted
}

public protocol PermissionManagerProtocol: AnyObject {
    func checkStatus(for permission: PermissionType) async -> PermissionStatus
    func requestPermission(for permission: PermissionType) async -> PermissionStatus
    var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> { get }
}

public protocol AccessibilityPermissionClient: AnyObject {
    func isProcessTrusted(prompt: Bool) -> Bool
}

public enum NotificationAuthorizationStatus: Equatable, Sendable {
    case notDetermined
    case denied
    case authorized
    case provisional
    case ephemeral
    case restricted
}

public protocol NotificationPermissionClient: AnyObject {
    func authorizationStatus() async -> NotificationAuthorizationStatus
    func requestAuthorization() async throws -> Bool
}

protocol PermissionMonitorScheduling: AnyObject {
    func start(interval: TimeInterval, handler: @escaping () -> Void)
    func stop()
}

public final class PermissionManager: PermissionManagerProtocol {
    public var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        statusSubject.eraseToAnyPublisher()
    }

    private let accessibilityClient: any AccessibilityPermissionClient
    private let notificationClient: any NotificationPermissionClient
    private let applicationIsActive: () -> Bool
    private let timerInterval: TimeInterval
    private let monitorScheduler: any PermissionMonitorScheduling
    private let accessibilityPromptPollScheduler: any PermissionMonitorScheduling
    private let accessibilityPromptPollInterval: TimeInterval
    private let accessibilityPromptPollLimit: Int
    private let activityNotifications: NotificationCenter
    private let statusSubject: CurrentValueSubject<[PermissionType: PermissionStatus], Never>
    private let lock = NSLock()
    private var activityObservers: [NSObjectProtocol] = []
    private var isMonitoring = false
    private var accessibilityPromptPollsRemaining = 0

    public convenience init() {
        self.init(
            accessibilityClient: SystemAccessibilityPermissionClient(),
            notificationClient: SystemNotificationPermissionClient()
        )
    }

    public convenience init(
        accessibilityClient: any AccessibilityPermissionClient,
        notificationClient: any NotificationPermissionClient,
        applicationIsActive: @escaping () -> Bool = { NSApplication.shared.isActive },
        timerInterval: TimeInterval = 5
    ) {
        self.init(
            accessibilityClient: accessibilityClient,
            notificationClient: notificationClient,
            applicationIsActive: applicationIsActive,
            timerInterval: timerInterval,
            monitorScheduler: DispatchPermissionMonitorScheduler(),
            accessibilityPromptPollScheduler: DispatchPermissionMonitorScheduler(),
            activityNotifications: .default
        )
    }

    init(
        accessibilityClient: any AccessibilityPermissionClient,
        notificationClient: any NotificationPermissionClient,
        applicationIsActive: @escaping () -> Bool,
        timerInterval: TimeInterval = 5,
        monitorScheduler: any PermissionMonitorScheduling,
        accessibilityPromptPollScheduler: any PermissionMonitorScheduling = DispatchPermissionMonitorScheduler(),
        accessibilityPromptPollInterval: TimeInterval = 1,
        accessibilityPromptPollLimit: Int = 120,
        activityNotifications: NotificationCenter = .default
    ) {
        self.accessibilityClient = accessibilityClient
        self.notificationClient = notificationClient
        self.applicationIsActive = applicationIsActive
        self.timerInterval = max(timerInterval, 5)
        self.monitorScheduler = monitorScheduler
        self.accessibilityPromptPollScheduler = accessibilityPromptPollScheduler
        self.accessibilityPromptPollInterval = max(accessibilityPromptPollInterval, 1)
        self.accessibilityPromptPollLimit = max(accessibilityPromptPollLimit, 1)
        self.activityNotifications = activityNotifications
        statusSubject = CurrentValueSubject([
            .accessibility: .unknown,
            .notifications: .unknown
        ])
    }

    public func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        let status: PermissionStatus
        switch permission {
        case .accessibility:
            status = await MainActor.run {
                accessibilityClient.isProcessTrusted(prompt: false) ? .granted : .denied
            }
        case .notifications:
            status = Self.map(await notificationClient.authorizationStatus())
        }
        publish(status, for: permission)
        return status
    }

    public func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        let status: PermissionStatus
        switch permission {
        case .accessibility:
            status = await MainActor.run {
                // Check without prompting first — if access was already granted on a
                // previous launch, calling isProcessTrusted(prompt: true) again would
                // needlessly re-trigger the system accessibility warning/dialog.
                if accessibilityClient.isProcessTrusted(prompt: false) {
                    stopAccessibilityPromptPolling()
                    return .granted
                }
                if accessibilityClient.isProcessTrusted(prompt: true) {
                    stopAccessibilityPromptPolling()
                    return .granted
                }
                startAccessibilityPromptPolling()
                return .denied
            }
        case .notifications:
            if currentStatus(for: .notifications) == .denied {
                return .denied
            }
            do {
                status = try await notificationClient.requestAuthorization() ? .granted : .denied
            } catch {
                status = .restricted
            }
        }
        publish(status, for: permission)
        return status
    }

    public func refreshAccessibilityPermissionOnLaunch() async -> PermissionStatus {
        await requestPermission(for: .accessibility)
    }

    public func startMonitoring() {
        let shouldStart = lock.withLock { () -> Bool in
            guard !isMonitoring else {
                return false
            }
            isMonitoring = true
            return true
        }
        guard shouldStart else {
            return
        }
        let observers = [
            activityNotifications.addObserver(
                forName: NSApplication.didBecomeActiveNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                self?.scheduleMonitor()
                Task { [weak self] in
                    _ = await self?.checkStatus(for: .notifications)
                }
            },
            activityNotifications.addObserver(
                forName: NSApplication.willResignActiveNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                self?.monitorScheduler.stop()
            }
        ]
        lock.withLock {
            activityObservers = observers
        }
        scheduleMonitor()
    }

    public func stopMonitoring() {
        let observers = lock.withLock { () -> [NSObjectProtocol] in
            let observers = activityObservers
            activityObservers = []
            isMonitoring = false
            return observers
        }
        for observer in observers {
            activityNotifications.removeObserver(observer)
        }
        monitorScheduler.stop()
        stopAccessibilityPromptPolling()
    }

    deinit {
        stopMonitoring()
    }

    private func scheduleMonitor() {
        guard lock.withLock({ isMonitoring }), applicationIsActive() else {
            return
        }
        // `scheduleMonitor()` runs from the main application lifecycle or a
        // main-queue activation notification. Publish this first check inline
        // so activation cannot leave the UI in its stale state while an
        // unstructured task waits for the main actor.
        let accessibilityStatus: PermissionStatus = accessibilityClient.isProcessTrusted(prompt: false)
            ? .granted
            : .denied
        publish(accessibilityStatus, for: .accessibility)
        monitorScheduler.start(interval: timerInterval) { [weak self] in
            guard let self, self.applicationIsActive() else {
                return
            }
            Task {
                _ = await self.checkStatus(for: .accessibility)
            }
        }
    }

    private func startAccessibilityPromptPolling() {
        accessibilityPromptPollsRemaining = accessibilityPromptPollLimit
        accessibilityPromptPollScheduler.start(interval: accessibilityPromptPollInterval) { [weak self] in
            guard let self else {
                return
            }
            if self.accessibilityClient.isProcessTrusted(prompt: false) {
                self.stopAccessibilityPromptPolling()
                self.publish(.granted, for: .accessibility)
                return
            }
            self.accessibilityPromptPollsRemaining -= 1
            if self.accessibilityPromptPollsRemaining <= 0 {
                self.stopAccessibilityPromptPolling()
            }
        }
    }

    private func stopAccessibilityPromptPolling() {
        accessibilityPromptPollsRemaining = 0
        accessibilityPromptPollScheduler.stop()
    }

    private func currentStatus(for permission: PermissionType) -> PermissionStatus {
        lock.withLock {
            statusSubject.value[permission] ?? .unknown
        }
    }

    private func publish(_ status: PermissionStatus, for permission: PermissionType) {
        let updated = lock.withLock { () -> [PermissionType: PermissionStatus]? in
            guard statusSubject.value[permission] != status else {
                return nil
            }
            var statuses = statusSubject.value
            statuses[permission] = status
            return statuses
        }
        if let updated {
            statusSubject.send(updated)
        }
    }

    private static func map(_ status: NotificationAuthorizationStatus) -> PermissionStatus {
        switch status {
        case .notDetermined:
            return .unknown
        case .authorized, .provisional, .ephemeral:
            return .granted
        case .denied:
            return .denied
        case .restricted:
            return .restricted
        }
    }
}

private final class DispatchPermissionMonitorScheduler: PermissionMonitorScheduling {
    private var timer: DispatchSourceTimer?

    func start(interval: TimeInterval, handler: @escaping () -> Void) {
        guard timer == nil else {
            return
        }
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + interval, repeating: interval, leeway: .seconds(1))
        timer.setEventHandler(handler: handler)
        self.timer = timer
        timer.resume()
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }
}

public final class FakePermissionManager: PermissionManagerProtocol {
    public var statusPublisher: AnyPublisher<[PermissionType: PermissionStatus], Never> {
        statusSubject.eraseToAnyPublisher()
    }

    public private(set) var requestedPermissions: [PermissionType] = []
    private let statusSubject = CurrentValueSubject<[PermissionType: PermissionStatus], Never>([
        .accessibility: .unknown,
        .notifications: .unknown
    ])

    public init() {}

    public func checkStatus(for permission: PermissionType) async -> PermissionStatus {
        statusSubject.value[permission] ?? .unknown
    }

    public func requestPermission(for permission: PermissionType) async -> PermissionStatus {
        requestedPermissions.append(permission)
        return statusSubject.value[permission] ?? .unknown
    }

    public func setStatus(_ status: PermissionStatus, for permission: PermissionType) {
        var statuses = statusSubject.value
        statuses[permission] = status
        statusSubject.send(statuses)
    }
}

public final class SystemAccessibilityPermissionClient: AccessibilityPermissionClient {
    public init() {}

    public func isProcessTrusted(prompt: Bool) -> Bool {
        AXIsProcessTrustedWithOptions([
            kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: prompt
        ] as CFDictionary)
    }
}

public final class SystemAccessibilityPermissionChecker: AccessibilityPermissionChecking {
    private let client: any AccessibilityPermissionClient

    public init(client: any AccessibilityPermissionClient = SystemAccessibilityPermissionClient()) {
        self.client = client
    }

    public func hasPermission() -> Bool {
        client.isProcessTrusted(prompt: false)
    }
}

public final class SystemNotificationPermissionClient: NotificationPermissionClient {
    private let center: UNUserNotificationCenter

    public init(center: UNUserNotificationCenter = .current()) {
        self.center = center
    }

    public func authorizationStatus() async -> NotificationAuthorizationStatus {
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .notDetermined:
            return .notDetermined
        case .denied:
            return .denied
        case .authorized:
            return .authorized
        case .provisional:
            return .provisional
        case .ephemeral:
            return .ephemeral
        @unknown default:
            return .restricted
        }
    }

    public func requestAuthorization() async throws -> Bool {
        try await center.requestAuthorization(options: [.alert, .badge, .sound])
    }
}
