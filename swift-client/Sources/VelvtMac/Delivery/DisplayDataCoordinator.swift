import Combine
import Foundation

@MainActor
public final class MenuStatusViewModel: ObservableObject {
    @Published public private(set) var status: MenuStatus?
    @Published public private(set) var sendError: String?
    private let ipcClient: any IPCClientProtocol
    private var cancellables = Set<AnyCancellable>()
    private var timer: AnyCancellable?

    public init(ipcClient: any IPCClientProtocol, messages: some Publisher<ServerMessage, Never>) {
        self.ipcClient = ipcClient
        messages.receive(on: RunLoop.main).sink { [weak self] message in
            switch message {
            case .menuStatus(let status):
                self?.status = status
                self?.sendError = nil
            case .errorResponse(let error) where error.code == "upload_flush_failed":
                self?.sendError = error.message
            default:
                break
            }
        }.store(in: &cancellables)
    }

    public func start() {
        refresh()
        timer = Timer.publish(every: 60, on: .main, in: .common).autoconnect().sink { [weak self] _ in self?.refresh() }
    }

    public func refresh() { Task { try? await ipcClient.send(.requestMenuStatus) } }

    public func sendAllNow() {
        Task {
            do {
                try await ipcClient.send(.flushUploadQueue)
            } catch {
                sendError = "Unable to send queued events. Try again later."
            }
        }
    }
}

@MainActor
final class MenuBarDataLoader {
    private let ipcClient: any IPCClientProtocol
    private let today: () -> String
    private var cancellable: AnyCancellable?
    private var requestedForConnection = false

    init(ipcClient: any IPCClientProtocol, today: @escaping () -> String = {
        ISO8601DateFormatter().string(from: Date()).prefix(10).description
    }) {
        self.ipcClient = ipcClient
        self.today = today
    }

    func start(accountState: AnyPublisher<AccountState, Never>) {
        cancellable = accountState.combineLatest(ipcClient.connectionStatus)
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

// MARK: - DisplayState

/// The three mutually exclusive display states for the insight and history panes.
///
/// `populated` holds the view-model references so views can bind to them directly.
/// Transitioning from `.loading` to `.populated` happens on the first push from
/// Rust — either insight or history — whichever arrives first.
public enum DisplayState {
    case loading
    case populated(insight: InsightViewModel, history: HistoryViewModel)
    case error(String)
}

public enum DeliveryAvailability: Equatable {
    case loading
    case available
    case notGenerated
}

public typealias InsightAvailability = DeliveryAvailability

// MARK: - DisplayDataCoordinating

/// Interface through which the IPC delivery layer feeds the display layer.
///
/// Implementations must not make IPC calls. The display layer is read-only:
/// it receives parsed payloads and reflects them in observable view models.
@MainActor
public protocol DisplayDataCoordinating: AnyObject {
    func updateInsight(_ payload: InsightPayload)
    func updateHistory(_ payload: HistoryPayload)
    var displayState: AnyPublisher<DisplayState, Never> { get }
}

// MARK: - ConcreteDisplayDataCoordinator

/// Subscribes to the IPC fan-out relay and connection-status publisher, then
/// routes payloads to the appropriate view models and maintains `DisplayState`.
///
/// Ownership: held by `AppDelegate`. Started once after `AccountStateManager`
/// begins listening so that `serverMessages` is already hot.
@MainActor
public final class ConcreteDisplayDataCoordinator: ObservableObject, DisplayDataCoordinating {

    // MARK: Published state

    @Published public private(set) var state: DisplayState = .loading
    @Published public private(set) var insightAvailability: InsightAvailability = .loading
    @Published public private(set) var historyAvailability: DeliveryAvailability = .loading

    public var displayState: AnyPublisher<DisplayState, Never> {
        $state.eraseToAnyPublisher()
    }

    // MARK: View models

    /// Exposed so `VelvtMacApp` can thread them into the view hierarchy before
    /// the first payload arrives; both start in their own `.isLoading` state.
    public let insightViewModel: InsightViewModel
    public let historyViewModel: HistoryViewModel

    // MARK: Private

    private var cancellables = Set<AnyCancellable>()
    /// Guards against treating the initial `.disconnected` status as an error.
    private var hasConnectedAtLeastOnce = false

    // MARK: Init

    public init(
        insightViewModel: InsightViewModel? = nil,
        historyViewModel: HistoryViewModel? = nil
    ) {
        // Default values can't be @MainActor-isolated expressions, so we
        // create them inside the init body which runs on the main actor.
        self.insightViewModel = insightViewModel ?? InsightViewModel()
        self.historyViewModel = historyViewModel ?? HistoryViewModel()
    }

    // MARK: Wiring

    /// Call once after the IPC client and AccountStateManager are ready.
    ///
    /// - Parameters:
    ///   - serverMessages: Fan-out relay from `AccountStateManager.serverMessages`.
    ///     The coordinator does NOT consume `incomingMessages` directly.
    ///   - connectionStatus: Socket-level status from `IPCClientProtocol.connectionStatus`.
    public func start(
        serverMessages: some Publisher<ServerMessage, Never>,
        connectionStatus: some Publisher<ConnectionStatus, Never>
    ) {
        serverMessages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in
                guard let self else { return }
                switch message {
                case .insightPayload(let p): self.updateInsight(p)
                case .historyPayload(let p): self.updateHistory(p)
                case .cacheEmpty(let empty): self.handleCacheEmpty(empty)
                default: break
                }
            }
            .store(in: &cancellables)

        connectionStatus
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                self?.handleConnectionStatus(status)
            }
            .store(in: &cancellables)
    }

    // MARK: DisplayDataCoordinating

    public func updateInsight(_ payload: InsightPayload) {
        insightViewModel.update(from: payload)
        insightAvailability = .available
        transitionToPopulatedIfNeeded()
    }

    public func updateHistory(_ payload: HistoryPayload) {
        historyViewModel.update(from: payload)
        historyAvailability = .available
        transitionToPopulatedIfNeeded()
    }

    public func handleCacheEmpty(_ payload: CacheEmpty) {
        switch payload.payloadType {
        case "insight_payload":
            insightAvailability = .notGenerated
        case "history_payload":
            historyAvailability = .notGenerated
        default:
            return
        }
        transitionToPopulatedIfNeeded()
    }

    // MARK: Private

    private func transitionToPopulatedIfNeeded() {
        guard case .loading = state else { return }
        state = .populated(insight: insightViewModel, history: historyViewModel)
    }

    private func handleConnectionStatus(_ status: ConnectionStatus) {
        switch status {
        case .connected:
            hasConnectedAtLeastOnce = true
            if case .error = state { state = .loading }
        case .disconnected, .reconnecting:
            // Ignore the initial disconnected status before any connection attempt.
            guard hasConnectedAtLeastOnce else { return }
            if case .loading = state {
                state = .error("Service unavailable")
            }
        default:
            break
        }
    }
}
