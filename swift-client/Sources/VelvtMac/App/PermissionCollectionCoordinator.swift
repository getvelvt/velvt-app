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
    private let statusSubject = CurrentValueSubject<PermissionCollectionStatus, Never>(.unknown)
    private var cancellable: AnyCancellable?
    private var lastAccessibilityStatus: PermissionStatus?

    public init(
        permissionManager: any PermissionManagerProtocol,
        collectionAgent: any CollectionAgentProtocol
    ) {
        self.permissionManager = permissionManager
        self.collectionAgent = collectionAgent
    }

    public func start() {
        guard cancellable == nil else {
            return
        }
        cancellable = permissionManager.statusPublisher.sink { [weak self] statuses in
            let status = statuses[.accessibility] ?? .unknown
            if Thread.isMainThread {
                self?.handle(status)
            } else {
                DispatchQueue.main.async {
                    self?.handle(status)
                }
            }
        }
    }

    public func stop() {
        cancellable?.cancel()
        cancellable = nil
        collectionAgent.stop()
        statusSubject.send(.unknown)
    }

    private func handle(_ status: PermissionStatus) {
        guard lastAccessibilityStatus != status else {
            return
        }
        lastAccessibilityStatus = status
        switch status {
        case .unknown:
            statusSubject.send(.unknown)
        case .granted:
            do {
                try collectionAgent.start()
                statusSubject.send(.collecting)
            } catch {
                statusSubject.send(.error)
            }
        case .denied, .restricted:
            collectionAgent.stop()
            statusSubject.send(.permissionRequired)
        }
    }
}
