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
    private var isCollecting = false
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

    private func startCollection() {
        guard !isCollecting else {
            statusSubject.send(.collecting)
            return
        }
        do {
            try collectionAgent.start()
            isCollecting = true
            statusSubject.send(.collecting)
        } catch {
            statusSubject.send(.error)
        }
    }

    private func stopCollection(force: Bool = false) {
        guard isCollecting || force else { return }
        collectionAgent.stop()
        isCollecting = false
    }
}
