import Foundation

@MainActor
public final class AuthGatedCollectionController {
    private let startCollection: () -> Void
    private let stopCollection: () -> Void
    private var isRunning = false

    public init(
        startCollection: @escaping () -> Void,
        stopCollection: @escaping () -> Void
    ) {
        self.startCollection = startCollection
        self.stopCollection = stopCollection
    }

    public func apply(accountState: AccountState) {
        if case .loggedIn = accountState {
            startIfNeeded()
        } else {
            stopIfNeeded()
        }
    }

    public func stop() {
        stopIfNeeded()
    }

    private func startIfNeeded() {
        guard !isRunning else {
            return
        }
        isRunning = true
        startCollection()
    }

    private func stopIfNeeded() {
        guard isRunning else {
            return
        }
        isRunning = false
        stopCollection()
    }
}
