import Combine
import Foundation

@MainActor
final class MenuBarDataLoader {
    private let ipcClient: any IPCClientProtocol
    private let today: () -> String
    private var cancellable: AnyCancellable?
    private var requestedForConnection = false

    init(
        ipcClient: any IPCClientProtocol,
        today: @escaping () -> String = {
            ISO8601DateFormatter().string(from: Date()).prefix(10).description
        }
    ) {
        self.ipcClient = ipcClient
        self.today = today
    }

    func start(accountState: AnyPublisher<AccountState, Never>) {
        cancellable = accountState
            .combineLatest(ipcClient.connectionStatus)
            .receive(on: RunLoop.main)
            .sink { [weak self] account, connection in
                guard let self else { return }
                guard case .loggedIn = account, connection == .connected else {
                    if connection != .connected { self.requestedForConnection = false }
                    return
                }
                guard !self.requestedForConnection else { return }
                self.requestedForConnection = true
                Task { [ipcClient, today] in
                    try? await ipcClient.send(.requestLatestInsight(.init(date: today())))
                    try? await ipcClient.send(.requestLatestHistory(.init(days: 7)))
                }
            }
    }
}
