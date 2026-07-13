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
    private var isCollecting = false

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

    public func start() {
        guard cancellable == nil else {
            return
        }
        cancellable = Publishers.CombineLatest3(
            permissionManager.statusPublisher.map { $0[.accessibility] ?? .unknown },
            connectionStatus,
            collectionSettings.$offlineEventCollectionEnabled
        )
        .sink { [weak self] status, connection, offlineEnabled in
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
    }

    public func stop() {
        cancellable?.cancel()
        cancellable = nil
        stopCollection(force: true)
        statusSubject.send(.unknown)
    }

    private func handle(
        permissionStatus: PermissionStatus,
        connectionStatus: ConnectionStatus,
        offlineEventCollectionEnabled: Bool
    ) {
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
