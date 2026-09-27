import AppKit
import Combine
import Foundation

public enum PermissionCollectionStatus: Equatable, Sendable {
    case unknown
    case collecting
    case permissionRequired
    case error
}

public final class PermissionCollectionCoordinator {
    public var statusPublisher: AnyPublisher<PermissionCollectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }

    private let permissionManager: any PermissionManagerProtocol
    private let collectionAgent: any CollectionAgentProtocol
    private let connectionStatus: AnyPublisher<ConnectionStatus, Never>
    private let collectionSettings: CollectionSettingsModel
    private let statusSubject = CurrentValueSubject<PermissionCollectionStatus, Never>(.unknown)
    private var cancellable: AnyCancellable?
    private var lifecycleCancellables = Set<AnyCancellable>()
    private var isSuspendedForSleep = false
    private var lastInputs: (PermissionStatus, ConnectionStatus, Bool)?

    public init(
        permissionManager: any PermissionManagerProtocol,
        collectionAgent: any CollectionAgentProtocol,
        connectionStatus: AnyPublisher<ConnectionStatus, Never> = Just(.connected).eraseToAnyPublisher(),
        collectionSettings: CollectionSettingsModel = CollectionSettingsModel()
    ) {
        self.permissionManager = permissionManager
        self.collectionAgent = collectionAgent
        self.connectionStatus = connectionStatus
        self.collectionSettings = collectionSettings
    }

    public func start(
        workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter
    ) {
        guard cancellable == nil else {
            return
        }
        cancellable = Publishers.CombineLatest3(
            permissionManager.statusPublisher.map { $0[.accessibility] ?? .unknown },
            connectionStatus,
            collectionSettings.$offlineEventCollectionEnabled
        )
        .sink { [weak self] status, connection, offlineEnabled in
            self?.lastInputs = (status, connection, offlineEnabled)
            if Thread.isMainThread {
                self?.handle(
                    permissionStatus: status,
                    connectionStatus: connection,
                    offlineEventCollectionEnabled: offlineEnabled
                )
            } else {
                DispatchQueue.main.async {
                    self?.handle(
                        permissionStatus: status,
                        connectionStatus: connection,
                        offlineEventCollectionEnabled: offlineEnabled
                    )
                }
            }
        }
        workspaceNotifications.publisher(for: NSWorkspace.willSleepNotification)
            .sink { [weak self] _ in self?.prepareForSleep() }
            .store(in: &lifecycleCancellables)
        workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
            .sink { [weak self] _ in self?.resumeAfterWake() }
            .store(in: &lifecycleCancellables)
    }

    public func stop() {
        cancellable?.cancel()
        cancellable = nil
        lifecycleCancellables.removeAll()
        isSuspendedForSleep = false
        lastInputs = nil
        stopCollection(force: true)
        statusSubject.send(.unknown)
    }

    private func handle(
        permissionStatus: PermissionStatus,
        connectionStatus: ConnectionStatus,
        offlineEventCollectionEnabled: Bool
    ) {
        guard !isSuspendedForSleep else {
            stopCollection()
            return
        }
        switch permissionStatus {
        case .unknown:
            statusSubject.send(.unknown)
        case .granted:
            guard connectionStatus == .connected || offlineEventCollectionEnabled else {
                stopCollection()
                statusSubject.send(.unknown)
                return
            }
            startCollection()
        case .denied, .restricted:
            stopCollection(force: true)
            statusSubject.send(.permissionRequired)
        }
    }

    private func prepareForSleep() {
        isSuspendedForSleep = true
        stopCollection()
    }

    private func resumeAfterWake() {
        isSuspendedForSleep = false
        guard let (permission, connection, offlineEnabled) = lastInputs else { return }
        handle(
            permissionStatus: permission,
            connectionStatus: connection,
            offlineEventCollectionEnabled: offlineEnabled
        )
    }

    /// Whether collection is running is the agent's fact, not this type's.
    ///
    /// There used to be an `isCollecting` flag here that was set on a
    /// successful start and cleared on a stop this type issued. The agent also
    /// stops itself — `stopAfterPermissionRevocation` runs when the AX observer
    /// reports the permission is gone — and it does not tell anyone, so the
    /// flag survived the thing it described. Revoking Accessibility and
    /// granting it again without activating the app in between is enough:
    /// `PermissionManager.publish` drops the granted-to-granted non-transition,
    /// nothing here re-evaluates on a permission change, and the next time
    /// anything did re-evaluate the flag said collection was already running.
    /// It never started again, and no surface said a word about it.
    private func startCollection() {
        guard !collectionAgent.isRunning else {
            statusSubject.send(.collecting)
            return
        }
        do {
            try collectionAgent.start()
            statusSubject.send(.collecting)
        } catch CollectionError.permissionRevoked {
            // The agent checked the permission at the moment of starting and
            // found it gone. That is the same condition `.denied` reports, and
            // it is what the recovery surface is for; `.error` would send a
            // person looking for a fault that is a setting.
            statusSubject.send(.permissionRequired)
        } catch {
            statusSubject.send(.error)
        }
    }

    private func stopCollection(force: Bool = false) {
        guard collectionAgent.isRunning || force else { return }
        collectionAgent.stop()
    }
}
