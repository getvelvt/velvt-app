import AppKit
import Combine
import Foundation

/// The needs-a-category card as the service last worded it, with the id it is
/// answered by.
public struct PresentedCategoryPrompt: Equatable, Sendable {
    public let promptID: String
    public let card: CategoryPromptCard
}

/// Carries the needs-a-category card and its daily reminder (protocol 33) from
/// the Rust service to the panel and to Notification Center.
///
/// Rust owns every judgement and every word: whether the card shows, whether a
/// reminder is due today (never during an active or paused work block, never
/// in Velvt's quiet hours or a known Focus, at most once a local day, only for
/// something new, with a backoff), and what both say. This type asks, renders
/// what it is told, and reports the answer.
///
/// It asks on the three occasions every other pulled card is asked for: when
/// the socket connects, when the Mac wakes, and on the 60-second cadence the
/// menu status already runs on. It keeps no timer of its own.
///
/// A reminder is handed over once and is consumed whether or not it is
/// posted, so it is posted at once or not at all: only if notifications are
/// already allowed. It is not worth a permission dialog, so it never asks for
/// one, and it is not an intervention, so it never touches that counter.
///
/// An answer is the person's own action and nothing asks for it again, so one
/// that cannot be sent is kept and sent first when the socket reconnects. A
/// tap on the reminder when no card id is known here (the app was relaunched,
/// or the tap launched it) answers the card the next reply carries.
@MainActor
public final class CategoryPromptCoordinator: ObservableObject {
    /// The card to show, or `nil` for none. An empty `category_prompt` clears
    /// it; answering it clears it at once.
    @Published public private(set) var prompt: PresentedCategoryPrompt?

    /// The most recent reminder's posting work. Exposed so tests can await it
    /// rather than race it.
    public private(set) var inFlightNotification: Task<Void, Never>?

    private let ipcClient: any IPCClientProtocol
    private let scheduler: any CategoryPromptNotificationScheduling
    private let permissionManager: any PermissionManagerProtocol
    private let reporter: any NotificationDeliveryReporting
    private let utcOffsetSeconds: () -> Int
    private var cancellables = Set<AnyCancellable>()
    private var sendChain: Task<Void, Never>?
    /// The id of the last card the service showed in this session, kept after
    /// the card is answered. A tap on the reminder can come after the card
    /// was closed, and `opened` is still the truth about what the person did:
    /// the service records it against the latest reminder whatever the card.
    private var lastPromptID: String?
    /// The card last answered here, until a reply says the service has moved
    /// on. A request already on its way when the person answered comes back
    /// with that same card, and with any reminder claimed for it; showing
    /// either for the moment before the answer's own reply lands would undo
    /// their tap. Replies arrive in order, so the first one that carries no
    /// card or another card was written after the answer was recorded.
    private var answeredPromptID: String?
    /// A reminder tapped when no card id was known here. The next reply's
    /// card is the one the reminder was about, and is answered `opened`.
    private var pendingOpened = false
    /// An answer whose send failed. Sent before the next request once the
    /// socket reconnects.
    private var pendingAnswer: AcknowledgeCategoryPrompt?

    public init(
        ipcClient: any IPCClientProtocol,
        scheduler: any CategoryPromptNotificationScheduling,
        permissionManager: any PermissionManagerProtocol,
        reporter: any NotificationDeliveryReporting = OSLogNotificationDeliveryReporter(),
        utcOffsetSeconds: @escaping () -> Int = { TimeZone.current.secondsFromGMT() }
    ) {
        self.ipcClient = ipcClient
        self.scheduler = scheduler
        self.permissionManager = permissionManager
        self.reporter = reporter
        self.utcOffsetSeconds = utcOffsetSeconds
    }

    /// - Parameter cadence: the menu status's 60-second refresh
    ///   (`MenuStatusViewModel.cadence`).
    public func start(
        messages: some Publisher<ServerMessage, Never>,
        connectionStatus: some Publisher<ConnectionStatus, Never>,
        cadence: some Publisher<Void, Never>,
        workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter
    ) {
        messages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in
                guard case .categoryPrompt(let prompt) = message else { return }
                self?.apply(prompt)
            }
            .store(in: &cancellables)

        connectionStatus
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                guard status == .connected, let self else { return }
                refresh()
            }
            .store(in: &cancellables)

        workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.refresh() }
            .store(in: &cancellables)

        cadence
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.refresh() }
            .store(in: &cancellables)
    }

    /// Asks the service for the card and any reminder due. Always safe: the
    /// service answers every request, and a reminder it has handed over once
    /// is never handed over again.
    ///
    /// An answer whose send failed goes first, on whichever pull comes next
    /// (a reconnect, a wake or the regular cadence), so the reply is the card
    /// as it stands after that answer.
    public func refresh() {
        if let pendingAnswer {
            self.pendingAnswer = nil
            acknowledge(pendingAnswer)
        }
        send(.requestCategoryPrompt(.init(utcOffsetSeconds: utcOffsetSeconds())))
    }

    /// The list was opened, from the card's primary action or from a tap on
    /// the reminder. Routing is the caller's; this closes the card and says so.
    public func open() {
        answer(.opened)
    }

    /// The card's secondary action. The card stays away until an entry no
    /// answer has reached is among those it counts.
    public func notNow() {
        answer(.notNow)
    }

    func apply(_ prompt: CategoryPrompt) {
        if pendingOpened {
            // The first reply after a tap that had no card to answer: its
            // card, if it has one, is what the reminder was about.
            pendingOpened = false
            if let promptID = prompt.promptID {
                lastPromptID = promptID
                answer(promptID, .opened)
            }
        }
        let answered = prompt.promptID != nil && prompt.promptID == answeredPromptID
        if !answered {
            answeredPromptID = nil
        }
        if let promptID = prompt.promptID, let card = prompt.card, !answered {
            self.prompt = PresentedCategoryPrompt(promptID: promptID, card: card)
            lastPromptID = promptID
        } else {
            self.prompt = nil
        }
        if let notification = prompt.notification, !answered {
            post(notification)
        }
    }

    private func answer(_ response: CategoryPromptResponse) {
        // Closed now, not when the service answers: it is the person's own
        // action, and the reply carries the card as it stands afterwards.
        let shown = prompt?.promptID
        prompt = nil
        if let promptID = shown ?? (response == .opened ? lastPromptID : nil) {
            answer(promptID, response)
        } else if response == .opened {
            // A tap on a reminder with no card id known in this process: ask,
            // and answer the card the reply carries.
            pendingOpened = true
            refresh()
        }
    }

    private func answer(_ promptID: String, _ response: CategoryPromptResponse) {
        answeredPromptID = promptID
        acknowledge(AcknowledgeCategoryPrompt(promptID: promptID, response: response))
    }

    /// Posts a reminder only if notifications are already allowed. `unknown`
    /// is not asked about: the reminder is not worth a permission dialog.
    /// Nothing retries it, because the service will not hand it over again.
    private func post(_ notification: CategoryPromptNotification) {
        inFlightNotification = Task { [scheduler, permissionManager, reporter] in
            let status = await permissionManager.checkStatus(for: .notifications)
            guard status == .granted else {
                reporter.report(.blockedByPermission(status), surface: .categoryPrompt)
                return
            }
            let posted = await scheduler.scheduleCategoryPrompt(
                title: notification.title, body: notification.body)
            reporter.report(posted ? .delivered : .rejectedByNotificationCentre, surface: .categoryPrompt)
        }
    }

    private func send(_ message: ClientMessage) {
        let previous = sendChain
        sendChain = Task { [ipcClient] in
            await previous?.value
            // A request that fails is left for the next pull: the service
            // answers every request, and nothing is lost by waiting for it.
            try? await ipcClient.send(message)
        }
    }

    /// Sends an answer, in order with the requests, and keeps it when the send
    /// fails: nothing asks for an answer again.
    private func acknowledge(_ answer: AcknowledgeCategoryPrompt) {
        let previous = sendChain
        sendChain = Task { [weak self, ipcClient] in
            await previous?.value
            do {
                try await ipcClient.send(.acknowledgeCategoryPrompt(answer))
            } catch {
                self?.pendingAnswer = answer
            }
        }
    }
}
