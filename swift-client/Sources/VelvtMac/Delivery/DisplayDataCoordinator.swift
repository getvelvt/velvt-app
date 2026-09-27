import Combine
import Foundation

/// Which control earned the acknowledgement currently on screen.
///
/// Not a judgement about the correction — the service authors every word of
/// the sentence — only a record of what the client last asked for, so the
/// confirmation can be drawn beside the control that caused it. Protocol 30
/// made Remove and Reset acknowledge too, and the workbench is a long scroll:
/// one banner pinned to one end of it is off screen for half the actions that
/// now produce one.
public enum CorrectionAcknowledgmentOrigin: Equatable, Sendable {
    /// A rule surface: correct, edit, remove, reset.
    case rule
    /// Teaching Velvt what a whole application is, from the triage list.
    case application
}

@MainActor
public final class MenuStatusViewModel: ObservableObject {
    /// The window the triage list is asked for, and the reason its copy can
    /// say "this week". Clamped again by `RequestUnclassifiedTriage` and once
    /// more inside the query.
    // `nonisolated` because callers read it to build a request before hopping
    // to the main actor; it is a constant, so isolation buys nothing and costs
    // a hard error under the Swift 6 language mode.
    public nonisolated static let triageLookbackDays = 7

    /// The codes the classification and teaching handlers refuse a command
    /// with, other than the `classification_correction_*` family.
    private static let classificationRejectionCodes: Set<String> = [
        "invalid_classification_category",
        "invalid_app_stable_id",
        "invalid_local_activity_name",
        "invalid_correction_history_query",
    ]

    @Published public private(set) var status: MenuStatus?
    @Published public private(set) var correctionHistoryPage: CorrectionHistoryPage?
    @Published public private(set) var correctionHistoryQuery = ""
    @Published public private(set) var sendError: String?
    /// Service-authored confirmation that a correction was taken.
    ///
    /// Latched here rather than read off `status`, because the status is
    /// refreshed on a timer and the confirmation would otherwise disappear
    /// within seconds of the correction that earned it.
    @Published public private(set) var correctionAcknowledgment: String?
    /// Where to draw `correctionAcknowledgment`. Latched with it and cleared
    /// with it, so the two can never disagree.
    @Published public private(set) var acknowledgmentOrigin: CorrectionAcknowledgmentOrigin?
    /// The applications Velvt observed but could not read.
    ///
    /// `nil` until the service answers: an empty list is the good state and
    /// must not be shown before the question has been asked.
    @Published public private(set) var unclassifiedTriage: UnclassifiedTriage?
    /// Why the triage list could not be read.
    ///
    /// Held apart from `unclassifiedTriage` for the reason the service states
    /// at `router.rs` `triage_error`: an empty list is the good state, so a
    /// failure must not borrow that sentence.
    @Published public private(set) var triageError: String?
    private let ipcClient: any IPCClientProtocol
    private var cancellables = Set<AnyCancellable>()
    private var timer: AnyCancellable?
    private var classificationCommand: Task<Void, Never>?
    private var correctionHistoryRequest: Task<Void, Never>?
    private var triageRequest: Task<Void, Never>?
    private var acknowledgmentDismissal: Task<Void, Never>?
    /// Set when a command that could produce an acknowledgement is sent, read
    /// when one arrives. The acknowledgement itself carries no origin — it is
    /// one string on a status snapshot — so this is the only thing that knows
    /// which control the user pressed.
    private var pendingAcknowledgmentOrigin: CorrectionAcknowledgmentOrigin = .rule
    /// Whether the triage surface has ever been opened in this session.
    ///
    /// Every rule write changes the triage list, so each one refreshes it —
    /// but only once something is actually showing it. A user who never opens
    /// the section never pays for the query.
    private var wantsTriageUpdates = false

    public init(ipcClient: any IPCClientProtocol, messages: some Publisher<ServerMessage, Never>) {
        self.ipcClient = ipcClient
        messages.receive(on: RunLoop.main).sink { [weak self] message in
            switch message {
            case .menuStatus(let status):
                self?.status = status
                self?.sendError = nil
                if let acknowledgment = status.correctionAcknowledgment {
                    self?.acknowledgeCorrection(acknowledgment)
                }
            case .correctionHistoryPage(let page):
                self?.correctionHistoryPage = page
                self?.sendError = nil
            case .unclassifiedTriage(let triage):
                self?.unclassifiedTriage = triage
                self?.triageError = nil
            case .errorResponse(let error) where error.code == "upload_flush_failed":
                self?.sendError = error.message
            case .errorResponse(let error) where error.code == "unclassified_triage_failed":
                self?.triageError = error.message
            // The rejection codes are listed rather than prefix-matched.
            // `SetApplicationCategory` refuses a malformed id, category or
            // name under `invalid_*` codes, and a teach that was refused had
            // already taken its row off the list — so those must be caught
            // here or the row vanishes and nothing is said. A prefix would
            // also catch `invalid_credentials` and `invalid_work_block_*`,
            // which have nothing to do with this surface.
            case .errorResponse(let error)
            where error.code.hasPrefix("classification_correction_")
                || Self.classificationRejectionCodes.contains(error.code):
                self?.sendError = error.message
                self?.refreshUnclassifiedTriageIfWanted()
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

    public func refreshCorrectionHistory(query: String? = nil, offset: Int? = nil) {
        if let query {
            correctionHistoryQuery = String(query.prefix(64))
        }
        let targetOffset = max(0, offset ?? correctionHistoryPage?.offset ?? 0)
        requestCorrectionHistory(offset: targetOffset)
    }

    /// Asks which applications Velvt could not read, and keeps the answer
    /// current from then on.
    public func refreshUnclassifiedTriage(lookbackDays: Int = MenuStatusViewModel.triageLookbackDays) {
        wantsTriageUpdates = true
        requestUnclassifiedTriage(lookbackDays: lookbackDays)
    }

    /// Teaches Velvt what one application is.
    ///
    /// `activityName` is the name the service itself reported for the
    /// application, handed straight back: it is the device-local name Velvt
    /// already holds, so returning it names the saved rule and lets the
    /// service's acknowledgement say the application's name instead of "This
    /// app". Nothing is decided here — the category is the user's answer and
    /// the sentence is the service's.
    public func teachApplication(_ entry: UnclassifiedTriageEntry, category: String) {
        // The row goes now, not when the service answers. This is the user's
        // own action on their own list, and a row that sits there looking
        // unpressed for a round trip reads as a control that does not work.
        // The refresh inside `enqueueClassificationCommand` is authoritative
        // either way: the service excludes an application the moment a rule
        // exists for it, and a refused teach brings the row straight back.
        removeTriageEntry(entry.appStableID)
        enqueueClassificationCommand(
            .setApplicationCategory(
                .init(
                    appStableID: entry.appStableID,
                    category: category,
                    activityName: entry.displayName
                )
            ),
            failureMessage: "Unable to save this app. Try again later.",
            origin: .application
        )
    }

    public func nextCorrectionHistoryPage() {
        guard let page = correctionHistoryPage, page.hasMore else { return }
        requestCorrectionHistory(offset: page.offset + page.pageSize)
    }

    public func previousCorrectionHistoryPage() {
        guard let page = correctionHistoryPage, page.offset > 0 else { return }
        requestCorrectionHistory(offset: max(0, page.offset - page.pageSize))
    }

    public func sendAllNow() {
        Task {
            do {
                try await ipcClient.send(.flushUploadQueue)
            } catch {
                sendError = "Unable to send queued events. Try again later."
            }
        }
    }

    public func correct(
        _ event: QueuedEventSummary,
        category: String,
        localActivityName: String? = nil
    ) {
        correct(
            eventID: event.eventID,
            stableID: event.stableID,
            category: category,
            localActivityName: localActivityName
        )
    }

    public func correct(
        eventID: UUID,
        stableID: String,
        category: String,
        localActivityName: String? = nil
    ) {
        enqueueClassificationCommand(
            .correctEventClassification(
                .init(
                    eventID: eventID,
                    stableID: stableID,
                    category: category,
                    localActivityName: localActivityName
                )
            ),
            failureMessage: "Unable to save this classification. Try again later."
        )
    }

    public func undoCorrection(_ event: QueuedEventSummary) {
        undoCorrection(stableID: event.stableID)
    }

    public func undoCorrection(stableID: String) {
        enqueueClassificationCommand(
            .removeClassificationOverride(.init(stableID: stableID)),
            failureMessage: "Unable to remove this correction. Try again later."
        )
    }

    public func updateCorrection(
        _ correction: ClassificationCorrectionSummary,
        category: String,
        localActivityName: String?
    ) {
        enqueueClassificationCommand(
            .updateClassificationOverride(
                .init(
                    stableID: correction.stableID,
                    category: category,
                    localActivityName: localActivityName
                )
            ),
            failureMessage: "Unable to update this correction. Try again later."
        )
    }

    /// Removes every correction and application rule the person set. The
    /// method name is historical; what it resets is a list of stored
    /// corrections, not anything learned, and the copy says so.
    public func resetClassificationLearning() {
        enqueueClassificationCommand(
            .resetClassificationOverrides,
            failureMessage: "Unable to reset your category corrections. Try again later."
        )
    }

    /// Shows the service's confirmation, then clears it.
    ///
    /// Held long enough to read and no longer: a confirmation that stays on
    /// screen stops reading as a response to what the user just did.
    private func acknowledgeCorrection(_ acknowledgment: String) {
        correctionAcknowledgment = acknowledgment
        acknowledgmentOrigin = pendingAcknowledgmentOrigin
        // Back to the default the moment it has been read. An acknowledgement
        // that arrives without a command behind it — a status pushed by the
        // service — belongs to the rule surfaces, not to the triage list.
        pendingAcknowledgmentOrigin = .rule
        acknowledgmentDismissal?.cancel()
        acknowledgmentDismissal = Task { [weak self] in
            try? await Task.sleep(for: .seconds(6))
            guard !Task.isCancelled else { return }
            self?.correctionAcknowledgment = nil
            self?.acknowledgmentOrigin = nil
        }
    }

    private func enqueueClassificationCommand(
        _ message: ClientMessage,
        failureMessage: String,
        origin: CorrectionAcknowledgmentOrigin = .rule
    ) {
        let previous = classificationCommand
        classificationCommand = Task { [weak self] in
            _ = await previous?.value
            guard let self else { return }
            do {
                // Set immediately before the send rather than when the command
                // was queued, so two commands issued in one breath produce
                // acknowledgements in the order they were actually sent.
                pendingAcknowledgmentOrigin = origin
                try await ipcClient.send(message)
                requestCorrectionHistory(offset: correctionHistoryPage?.offset ?? 0)
                // Every rule write can change the triage list, not just a
                // teach: removing an app rule puts the application back on it,
                // a reset puts them all back, and a window correction that
                // generalizes takes one off. Requested after the write on the
                // same chained task, so it cannot read the list back before
                // the write that changed it.
                refreshUnclassifiedTriageIfWanted()
            } catch {
                sendError = failureMessage
                refreshUnclassifiedTriageIfWanted()
            }
        }
    }

    /// Re-asks for the triage list, but only if something is showing it.
    private func refreshUnclassifiedTriageIfWanted() {
        guard wantsTriageUpdates else { return }
        requestUnclassifiedTriage(lookbackDays: Self.triageLookbackDays)
    }

    private func requestUnclassifiedTriage(lookbackDays: Int) {
        let previous = triageRequest
        triageRequest = Task { [weak self] in
            _ = await previous?.value
            guard let self else { return }
            do {
                try await ipcClient.send(
                    .requestUnclassifiedTriage(.init(lookbackDays: lookbackDays))
                )
            } catch {
                triageError = "Unable to list the apps Velvt could not read. Try again later."
            }
        }
    }

    /// Takes one application off the list in hand, preserving the window the
    /// service computed it over.
    private func removeTriageEntry(_ appStableID: String) {
        guard let triage = unclassifiedTriage else { return }
        unclassifiedTriage = UnclassifiedTriage(
            entries: triage.entries.filter { $0.appStableID != appStableID },
            windowDays: triage.windowDays
        )
    }

    private func requestCorrectionHistory(offset: Int) {
        let query = correctionHistoryQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        let previous = correctionHistoryRequest
        correctionHistoryRequest = Task { [weak self] in
            _ = await previous?.value
            guard let self else { return }
            do {
                try await ipcClient.send(
                    .requestCorrectionHistory(
                        .init(query: query.isEmpty ? nil : query, offset: offset)
                    )
                )
            } catch {
                sendError = "Unable to load saved corrections. Try again later."
            }
        }
    }
}

@MainActor
final class MenuBarDataLoader {
    private let ipcClient: any IPCClientProtocol
    private let currentLocalInsightDate: () -> String
    private let retryDelayNanoseconds: UInt64
    private var cancellable: AnyCancellable?
    private var requestedForConnection = false
    private var requestInFlight = false
    private var canRequest = false

    init(
        ipcClient: any IPCClientProtocol,
        currentLocalInsightDate: @escaping () -> String = {
            MenuBarDataLoader.currentUTCDateString()
        }, retryDelayNanoseconds: UInt64 = 2_000_000_000
    ) {
        self.ipcClient = ipcClient
        self.currentLocalInsightDate = currentLocalInsightDate
        self.retryDelayNanoseconds = retryDelayNanoseconds
    }

    nonisolated static func currentLocalDateString(
        now: Date = Date(),
        timeZone: TimeZone = .current
    ) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let components = calendar.dateComponents([.year, .month, .day], from: now)
        return String(
            format: "%04d-%02d-%02d",
            components.year ?? 0,
            components.month ?? 0,
            components.day ?? 0
        )
    }

    nonisolated static func currentUTCDateString(now: Date = Date()) -> String {
        currentLocalDateString(now: now, timeZone: TimeZone(secondsFromGMT: 0)!)
    }

    func start(accountState: AnyPublisher<AccountState, Never>) {
        cancellable = accountState.combineLatest(ipcClient.connectionStatus)
            .receive(on: RunLoop.main)
            .sink { [weak self] account, connection in
                guard let self else { return }
                guard case .loggedIn = account, connection == .connected else {
                    self.canRequest = false
                    self.requestedForConnection = false
                    self.requestInFlight = false
                    return
                }
                self.canRequest = true
                self.requestDisplayDataIfNeeded()
            }
    }

    private func requestDisplayDataIfNeeded() {
        guard canRequest, !requestedForConnection, !requestInFlight else { return }
        requestInFlight = true
        Task { [weak self, ipcClient, currentLocalInsightDate] in
            do {
                try await ipcClient.send(.requestLatestInsight(.init(date: currentLocalInsightDate())))
                try await ipcClient.send(.requestLatestHistory(.init(days: 14)))
                await MainActor.run {
                    guard let self else { return }
                    self.requestedForConnection = self.canRequest
                    self.requestInFlight = false
                }
            } catch {
                await MainActor.run {
                    self?.requestedForConnection = false
                    self?.requestInFlight = false
                    self?.scheduleRetry()
                }
            }
        }
    }

    private func scheduleRetry() {
        Task { [weak self, retryDelayNanoseconds] in
            try? await Task.sleep(nanoseconds: retryDelayNanoseconds)
            await MainActor.run {
                self?.requestDisplayDataIfNeeded()
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
    @Published public private(set) var insightNotReadyReason: String?
    /// Why history is not ready, kept for the same reason the insight one is.
    /// Rust already distinguishes "the backend could not be reached" from
    /// "there is nothing to show" (`router.rs` emits `backend_unavailable` and
    /// `invalid_cached_payload`), and discarding it made a network failure
    /// render as advice to keep working — the app blaming the user for its
    /// own outage.
    @Published public private(set) var historyNotReadyReason: String?

    /// The service's own account of its health, which the client used to
    /// decode and drop on the floor.
    ///
    /// Rust reports this on every connection — derived from auth state at
    /// `ipc/connection.rs:381` — and on health transitions such as Tier 2
    /// classification becoming unavailable (`delivery/push.rs:354`). Nothing in
    /// Swift referenced it outside its own decoder, so an app running degraded,
    /// signed out, or with uploads paused looked exactly like one running
    /// perfectly.
    @Published public private(set) var serviceStatus: ServiceStatus?

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
        connectionStatus: some Publisher<ConnectionStatus, Never>,
        accountState: AnyPublisher<AccountState, Never>? = nil
    ) {
        serverMessages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in
                guard let self else { return }
                switch message {
                case .insightPayload(let p): self.updateInsight(p)
                case .historyPayload(let p): self.updateHistory(p)
                case .cacheEmpty(let empty): self.handleCacheEmpty(empty)
                case .serviceStatus(let status): self.serviceStatus = status
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

        accountState?
            .receive(on: RunLoop.main)
            .sink { [weak self] state in
                self?.handleAccountState(state)
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
            insightNotReadyReason = payload.reason
        case "history_payload":
            historyAvailability = .notGenerated
            historyNotReadyReason = payload.reason
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

    private func resetDisplayData() {
        insightViewModel.reset()
        historyViewModel.reset()
        insightAvailability = .loading
        insightNotReadyReason = nil
        historyNotReadyReason = nil
        historyAvailability = .loading
        state = .loading
    }

    private func handleAccountState(_ accountState: AccountState) {
        guard case .loggedIn = accountState else {
            resetDisplayData()
            return
        }
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
