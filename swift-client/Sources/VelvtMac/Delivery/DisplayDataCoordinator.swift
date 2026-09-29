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
    /// Teaching Velvt what a whole application or site is, from the
    /// needs-a-category list.
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
        "invalid_site_stable_id",
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
    /// The applications and sites Velvt observed but could not categorize.
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
    private let ticks = PassthroughSubject<Void, Never>()
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
            // `SetApplicationCategory` and `SetSiteCategory` refuse a
            // malformed id, category or name under `invalid_*` codes, and a teach that was refused had
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
        timer = Timer.publish(every: 60, on: .main, in: .common).autoconnect().sink { [weak self] _ in
            self?.tick()
        }
    }

    /// One tick of the 60-second refresh: the status, then whatever shares
    /// the `cadence`.
    func tick() {
        refresh()
        ticks.send()
    }

    /// Fires on each tick of the 60-second status refresh, so another pull
    /// can share this cadence rather than run a timer of its own.
    public var cadence: AnyPublisher<Void, Never> { ticks.eraseToAnyPublisher() }

    public func refresh() { Task { try? await ipcClient.send(.requestMenuStatus) } }

    public func refreshCorrectionHistory(query: String? = nil, offset: Int? = nil) {
        if let query {
            correctionHistoryQuery = String(query.prefix(64))
        }
        let targetOffset = max(0, offset ?? correctionHistoryPage?.offset ?? 0)
        requestCorrectionHistory(offset: targetOffset)
    }

    /// Asks which applications and sites Velvt could not categorize, and
    /// keeps the answer current from then on.
    public func refreshUnclassifiedTriage(lookbackDays: Int = MenuStatusViewModel.triageLookbackDays) {
        wantsTriageUpdates = true
        requestUnclassifiedTriage(lookbackDays: lookbackDays)
    }

    /// Teaches Velvt what one application or site on the list is.
    ///
    /// Nothing is decided here: the category is the user's answer and the
    /// acknowledgement is the service's sentence. What this does decide is what
    /// goes back with the key, and the answer is as little as possible.
    ///
    /// - An application's local name, when the service reported one, is handed
    ///   straight back: it is the device-local name Velvt already holds, so
    ///   returning it names the saved rule and lets the acknowledgement say the
    ///   application's name. With no name, the row reads "Unnamed application",
    ///   and that placeholder is never sent back: as a rule name it would label
    ///   every later window of the application.
    /// - A site sends its key alone. Its display name is its hostname, which
    ///   the service keeps only until the site is taught; sent back as a rule
    ///   name it would be a second stored copy of the hostname.
    public func teach(_ entry: UnclassifiedTriageEntry, category: String) {
        // The row goes now, not when the service answers. This is the user's
        // own action on their own list, and a row that sits there looking
        // unpressed for a round trip reads as a control that does not work.
        // The refresh inside `enqueueClassificationCommand` is authoritative
        // either way: the service excludes an entry the moment a rule exists
        // for it, and a refused teach brings the row straight back.
        removeTriageEntry(entry)
        switch entry.kind {
        case .application:
            enqueueClassificationCommand(
                .setApplicationCategory(
                    .init(
                        appStableID: entry.stableID,
                        category: category,
                        activityName: Self.ruleName(forApplicationNamed: entry.displayName)
                    )
                ),
                failureMessage: "Unable to save this app. Try again later.",
                origin: .application
            )
        case .site:
            enqueueClassificationCommand(
                .setSiteCategory(.init(siteStableID: entry.stableID, category: category)),
                failureMessage: "Unable to save this site. Try again later.",
                origin: .application
            )
        }
    }

    /// The application's own name as a rule name, or `nil` when there is none
    /// the service would accept as one: 1 to 48 characters with no control
    /// characters, the same rule the service applies. A name it would refuse
    /// would take the whole answer down with it, and the rule is worth more
    /// than its label.
    nonisolated static func ruleName(forApplicationNamed name: String?) -> String? {
        guard let trimmed = name?.trimmingCharacters(in: .whitespacesAndNewlines),
            !trimmed.isEmpty,
            // Counted in scalars, as the service counts `char`s.
            trimmed.unicodeScalars.count <= 48,
            !trimmed.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else { return nil }
        return trimmed
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
                triageError = "Unable to list the apps and sites Velvt couldn't categorize. Try again later."
            }
        }
    }

    /// Takes one entry off the list in hand, preserving the window the
    /// service computed it over.
    private func removeTriageEntry(_ entry: UnclassifiedTriageEntry) {
        guard let triage = unclassifiedTriage else { return }
        unclassifiedTriage = UnclassifiedTriage(
            entries: triage.entries.filter { $0.id != entry.id },
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

/// Asks the service for the insight and the history the popover shows.
///
/// Requests go out only once the connection has the session the service
/// needs (`AccountStateManager.isSessionHandedOver`), so the service reads
/// them with it; the history first, so the Patterns card waits on one cloud
/// read at most rather than two; the insight only while signed in, since
/// only the cloud has one; and the history signed in or not, since a
/// signed-out Mac is answered with summaries built on it.
///
/// The history is asked for again when the account settles into a different
/// state; and, while the last answer was not the cloud's (it came from this
/// Mac, or could not be read), when a surface showing it appears and on the
/// menu status's cadence at most every `localHistoryRefreshInterval`. A
/// history built on this Mac goes stale as the day goes on, and those asks
/// keep it current; they cost no wait on the cloud, which the service stops
/// asking after a failed read until it answers its own fetch scheduler. A
/// synced history is not asked for again: the service pushes a new one each
/// time its scheduler fetches one, which is also how a recovered cloud
/// replaces this Mac's, and asking on every Patterns view would tell the
/// server when Patterns was opened.
@MainActor
final class MenuBarDataLoader {
    /// The days of history asked for: the Daily Activity chart's window.
    nonisolated static let historyDays = 14
    /// The least time between two history requests made on the cadence while
    /// the last history came from this Mac.
    nonisolated static let localHistoryRefreshInterval: TimeInterval = 10 * 60
    /// How long a history request counts as unanswered. The service answers
    /// every one, within its 10-second cloud timeout; this only keeps a reply
    /// lost with its connection from blocking every later request.
    nonisolated static let historyReplyTimeout: TimeInterval = 60

    /// An account state the service can be asked on behalf of. Signing in or
    /// out is neither: the answer would describe the state being left.
    private enum SettledAccount: Equatable {
        case signedIn
        case signedOut
    }

    private enum LastHistory: Equatable {
        case none
        case cloud
        case thisMac
        case unavailable
    }

    private let ipcClient: any IPCClientProtocol
    private let currentLocalInsightDate: () -> String
    private let utcOffsetSeconds: () -> Int
    private let now: () -> Date
    private let retryDelayNanoseconds: UInt64
    private var cancellables = Set<AnyCancellable>()
    /// The account requests may be sent for now; `nil` before the session is
    /// handed over on this connection, and while signing in or out.
    private var readyAccount: SettledAccount?
    /// The account the opening requests went out for on this connection.
    private var requestedFor: SettledAccount?
    private var openingRequestInFlight = false
    /// Advances whenever the connection goes, so an opening request that
    /// finishes after its connection has gone is not counted for the next.
    private var connectionEpoch = 0
    private var historySentAt: Date?
    private var historyAwaitingReply = false
    private var lastHistory = LastHistory.none

    init(
        ipcClient: any IPCClientProtocol,
        currentLocalInsightDate: @escaping () -> String = {
            MenuBarDataLoader.currentUTCDateString()
        },
        utcOffsetSeconds: @escaping () -> Int = { TimeZone.current.secondsFromGMT() },
        now: @escaping () -> Date = Date.init,
        retryDelayNanoseconds: UInt64 = 2_000_000_000
    ) {
        self.ipcClient = ipcClient
        self.currentLocalInsightDate = currentLocalInsightDate
        self.utcOffsetSeconds = utcOffsetSeconds
        self.now = now
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

    /// - Parameters:
    ///   - sessionHandedOver: `AccountStateManager.$isSessionHandedOver`.
    ///   - messages: the server-message fan-out, read for which history arrived.
    ///   - historyRefreshRequests: a surface showing the history appeared
    ///     (`ConcreteDisplayDataCoordinator.historyRefreshRequests`).
    ///   - cadence: the menu status's 60-second refresh (`MenuStatusViewModel.cadence`).
    func start(
        accountState: AnyPublisher<AccountState, Never>,
        sessionHandedOver: AnyPublisher<Bool, Never>,
        messages: some Publisher<ServerMessage, Never>,
        historyRefreshRequests: some Publisher<Void, Never>,
        cadence: some Publisher<Void, Never>
    ) {
        accountState.combineLatest(sessionHandedOver)
            .receive(on: RunLoop.main)
            .sink { [weak self] account, handedOver in
                self?.update(account: account, sessionHandedOver: handedOver)
            }
            .store(in: &cancellables)

        messages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in self?.observe(message) }
            .store(in: &cancellables)

        historyRefreshRequests
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.historySurfaceAppeared() }
            .store(in: &cancellables)

        cadence
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.refreshLocalHistoryIfDue() }
            .store(in: &cancellables)
    }

    /// Starts the loader on the objects the app runs it with: the account's
    /// state and session handover, its server messages, the appearances of
    /// the surfaces showing the history (`displayCoordinator`), and the menu
    /// status's 60-second cadence.
    func start(
        accountStateManager: AccountStateManager,
        displayCoordinator: ConcreteDisplayDataCoordinator,
        statusViewModel: MenuStatusViewModel
    ) {
        start(
            accountState: accountStateManager.$accountState.eraseToAnyPublisher(),
            sessionHandedOver: accountStateManager.$isSessionHandedOver.eraseToAnyPublisher(),
            messages: accountStateManager.serverMessages,
            historyRefreshRequests: displayCoordinator.historyRefreshRequests,
            cadence: statusViewModel.cadence
        )
    }

    /// A surface showing the history appeared. A synced history is kept
    /// current by the service's pushes, so only one that is not is asked for
    /// again.
    private func historySurfaceAppeared() {
        guard lastHistory != .cloud else { return }
        refreshHistory()
    }

    /// Asks for the history again, unless it cannot be asked for yet or a
    /// request is already waiting on its answer.
    func refreshHistory() {
        guard readyAccount != nil, !openingRequestInFlight, !isAwaitingHistory else { return }
        Task { [weak self] in try? await self?.sendHistoryRequest() }
    }

    private var isAwaitingHistory: Bool {
        guard historyAwaitingReply, let historySentAt else { return false }
        return now().timeIntervalSince(historySentAt) < Self.historyReplyTimeout
    }

    private func update(account: AccountState, sessionHandedOver: Bool) {
        guard sessionHandedOver else {
            readyAccount = nil
            requestedFor = nil
            historyAwaitingReply = false
            connectionEpoch &+= 1
            return
        }
        switch account {
        case .loggedIn: readyAccount = .signedIn
        case .loggedOut: readyAccount = .signedOut
        case .loggingIn, .loggingOut, .pendingErasure: readyAccount = nil
        }
        requestOpeningDataIfNeeded()
    }

    private func observe(_ message: ServerMessage) {
        switch message {
        case .historyPayload(let payload):
            historyAwaitingReply = false
            lastHistory = payload.source == .thisMac ? .thisMac : .cloud
        case .cacheEmpty(let empty) where empty.payloadType == "history_payload":
            historyAwaitingReply = false
            lastHistory = .unavailable
        default:
            break
        }
    }

    /// Built on this Mac, a history goes stale as the day goes on, and the
    /// cloud may have come back; neither says so on its own. A failed local
    /// read is asked for again on the same terms.
    private func refreshLocalHistoryIfDue() {
        guard lastHistory == .thisMac || lastHistory == .unavailable,
            let historySentAt,
            now().timeIntervalSince(historySentAt) >= Self.localHistoryRefreshInterval
        else { return }
        refreshHistory()
    }

    private func requestOpeningDataIfNeeded() {
        guard let account = readyAccount, requestedFor != account, !openingRequestInFlight else {
            return
        }
        openingRequestInFlight = true
        let epoch = connectionEpoch
        Task { [weak self] in
            guard let self else { return }
            do {
                // The history before the insight. The service answers one
                // request at a time, and with the backend down each cloud
                // read waits out its timeout: asked second, the history waited
                // on the insight's read as well as its own.
                try await sendHistoryRequest()
                if account == .signedIn {
                    try await ipcClient.send(
                        .requestLatestInsight(.init(date: currentLocalInsightDate()))
                    )
                }
                openingRequestInFlight = false
                if connectionEpoch == epoch {
                    requestedFor = account
                }
                // The account may have settled elsewhere while these were
                // going out.
                requestOpeningDataIfNeeded()
            } catch {
                openingRequestInFlight = false
                scheduleRetry()
            }
        }
    }

    private func sendHistoryRequest() async throws {
        historyAwaitingReply = true
        historySentAt = now()
        do {
            try await ipcClient.send(
                .requestLatestHistory(
                    .init(days: Self.historyDays, utcOffsetSeconds: utcOffsetSeconds())
                )
            )
        } catch {
            historyAwaitingReply = false
            throw error
        }
    }

    private func scheduleRetry() {
        Task { [weak self, retryDelayNanoseconds] in
            try? await Task.sleep(nanoseconds: retryDelayNanoseconds)
            self?.requestOpeningDataIfNeeded()
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
    /// Whether the account is signed in, as last reported. The history card
    /// says why its summaries came from this Mac, and the reason differs.
    @Published public private(set) var isSignedIn = false
    /// Signed in, and no history has arrived since: the one shown, if any,
    /// was built on this Mac while signed out, and the cloud has not yet been
    /// asked for this account's. The card must not say the synced summaries
    /// are unavailable before anything has asked for them.
    @Published public private(set) var isAwaitingSyncedHistory = false

    public var displayState: AnyPublisher<DisplayState, Never> {
        $state.eraseToAnyPublisher()
    }

    /// Fires when a surface that shows the history appears. The coordinator
    /// makes no IPC call of its own; `MenuBarDataLoader` listens and asks.
    public var historyRefreshRequests: AnyPublisher<Void, Never> {
        historyRefreshSubject.eraseToAnyPublisher()
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
    private let historyRefreshSubject = PassthroughSubject<Void, Never>()

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
        isAwaitingSyncedHistory = false
        transitionToPopulatedIfNeeded()
    }

    /// A surface showing the history appeared: ask for it again.
    public func requestHistoryRefresh() {
        historyRefreshSubject.send()
    }

    public func handleCacheEmpty(_ payload: CacheEmpty) {
        switch payload.payloadType {
        case "insight_payload":
            insightAvailability = .notGenerated
            insightNotReadyReason = payload.reason
        case "history_payload":
            historyAvailability = .notGenerated
            historyNotReadyReason = payload.reason
            isAwaitingSyncedHistory = false
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

    private func resetDisplayData(includingHistory: Bool) {
        insightViewModel.reset()
        insightAvailability = .loading
        insightNotReadyReason = nil
        if includingHistory {
            historyViewModel.reset()
            historyNotReadyReason = nil
            historyAvailability = .loading
            state = .loading
        }
    }

    /// The insight is the account's, so it goes whenever the account is not
    /// signed in. The history goes only when the account stops being signed
    /// in: a synced history belongs to the account being left, while one
    /// built on this Mac while signed out stays until its replacement
    /// arrives, and a signed-out Mac is asked for its own.
    private func handleAccountState(_ accountState: AccountState) {
        let wasSignedIn = isSignedIn
        if case .loggedIn = accountState {
            isSignedIn = true
            if !wasSignedIn { isAwaitingSyncedHistory = true }
            return
        }
        isSignedIn = false
        isAwaitingSyncedHistory = false
        resetDisplayData(includingHistory: wasSignedIn)
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
