import Combine
import Foundation

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
        transitionToPopulatedIfNeeded()
    }

    public func updateHistory(_ payload: HistoryPayload) {
        historyViewModel.update(from: payload)
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
