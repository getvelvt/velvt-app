import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

/// The needs-a-category card and its daily reminder (protocol 33), on the
/// Swift side: when the client asks, what it does with the answer, and where
/// the card and the reminder take the person.
@MainActor
final class CategoryPromptCoordinatorTests: XCTestCase {
    private let promptID = String(repeating: "d", count: 64)
    private let laterPromptID = String(repeating: "e", count: 64)

    // MARK: - When it asks

    func testItAsksOnConnectOnWakeAndOnTheStatusCadenceWithItsUTCOffset() async throws {
        let harness = Harness(utcOffsetSeconds: 19_800)
        let request = ClientMessage.requestCategoryPrompt(.init(utcOffsetSeconds: 19_800))

        try await Task.sleep(nanoseconds: 30_000_000)
        XCTAssertEqual(harness.requests, 0, "nothing is asked before the socket connects")

        harness.client.setConnectionStatus(.connected)
        try await waitUntil { harness.requests == 1 }
        XCTAssertEqual(harness.client.sentMessages, [request])

        // A repeated status is not a new connection.
        harness.client.setConnectionStatus(.connected)
        harness.workspace.post(name: NSWorkspace.didWakeNotification, object: nil)
        try await waitUntil { harness.requests == 2 }

        harness.cadence.send()
        try await waitUntil { harness.requests == 3 }

        harness.client.setConnectionStatus(.disconnected)
        harness.client.setConnectionStatus(.connected)
        try await waitUntil { harness.requests == 4 }
        XCTAssertTrue(harness.client.sentMessages.allSatisfy { $0 == request })
    }

    // MARK: - What it does with the answer

    func testTheCardIsTheServicesAndAnEmptyAnswerTakesItAway() async throws {
        let harness = Harness()

        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        try await waitUntil { harness.sut.prompt != nil }
        XCTAssertEqual(harness.sut.prompt, PresentedCategoryPrompt(promptID: promptID, card: card))

        harness.messages.send(.categoryPrompt(CategoryPrompt()))
        try await waitUntil { harness.sut.prompt == nil }
    }

    func testOpeningAnswersTheCardShownAsOpenedAndClosesIt() async throws {
        let harness = Harness()
        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        try await waitUntil { harness.sut.prompt != nil }

        harness.sut.open()

        XCTAssertNil(harness.sut.prompt, "the card closes on the tap, not on the reply")
        try await waitUntil {
            harness.client.sentMessages == [
                .acknowledgeCategoryPrompt(.init(promptID: self.promptID, response: .opened))
            ]
        }
    }

    func testNotNowAnswersTheCardShownAsNotNowAndClosesIt() async throws {
        let harness = Harness()
        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        try await waitUntil { harness.sut.prompt != nil }

        harness.sut.notNow()

        XCTAssertNil(harness.sut.prompt)
        try await waitUntil {
            harness.client.sentMessages == [
                .acknowledgeCategoryPrompt(.init(promptID: self.promptID, response: .notNow))
            ]
        }
    }

    /// A request already on its way when the card was answered comes back
    /// with that card. Drawing it again would undo the tap until the answer's
    /// own reply landed; a card with something new on it still shows.
    func testAnAnsweredCardDoesNotComeBackButANewOneDoes() async throws {
        let harness = Harness()
        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        try await waitUntil { harness.sut.prompt != nil }
        harness.sut.notNow()

        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        harness.messages.send(.categoryPrompt(cardOnly(laterPromptID)))
        try await waitUntil { harness.sut.prompt != nil }
        XCTAssertEqual(harness.sut.prompt?.promptID, laterPromptID)
    }

    /// The reminder can be tapped after its card was closed. The person still
    /// opened the list from it, and that is what ends a run of unopened
    /// reminders, so it is still said.
    func testATapAfterTheCardWasClosedStillAnswersOpened() async throws {
        let harness = Harness()
        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        try await waitUntil { harness.sut.prompt != nil }
        harness.sut.notNow()

        harness.sut.open()

        try await waitUntil { harness.client.sentMessages.count == 2 }
        XCTAssertEqual(
            harness.client.sentMessages.last,
            .acknowledgeCategoryPrompt(.init(promptID: promptID, response: .opened)))
    }

    func testNothingIsAnsweredWithoutACardToAnswer() async throws {
        let harness = Harness()

        harness.sut.open()
        harness.sut.notNow()
        try await Task.sleep(nanoseconds: 30_000_000)

        XCTAssertEqual(harness.client.sentMessages, [])
    }

    // MARK: - The reminder

    func testTheReminderIsPostedVerbatimWhenNotificationsAreAllowed() async throws {
        let harness = Harness()
        harness.permissions.setStatus(.granted, for: .notifications)

        harness.messages.send(.categoryPrompt(withReminder(promptID)))
        try await waitUntil { harness.sut.inFlightNotification != nil }
        await harness.sut.inFlightNotification?.value

        XCTAssertEqual(
            harness.scheduler.scheduledCategoryPrompts,
            [.init(title: reminder.title, body: reminder.body)])
        XCTAssertEqual(harness.reporter.entries, [.init(outcome: .delivered, surface: .categoryPrompt)])
        XCTAssertEqual(harness.sut.prompt?.promptID, promptID, "the card arrives with its reminder")
        XCTAssertTrue(harness.scheduler.scheduledPayloads.isEmpty, "not through the insight's path")
        XCTAssertTrue(harness.scheduler.scheduledInterventions.isEmpty, "not through the drift offer's path")
    }

    /// The reminder is not worth a permission dialog: `unknown` is left
    /// unasked, and the reminder is simply not posted.
    func testTheReminderNeverAsksForPermission() async throws {
        let harness = Harness()

        harness.messages.send(.categoryPrompt(withReminder(promptID)))
        try await waitUntil { harness.sut.inFlightNotification != nil }
        await harness.sut.inFlightNotification?.value

        XCTAssertEqual(harness.permissions.requestedPermissions, [])
        XCTAssertEqual(harness.scheduler.scheduledCategoryPrompts, [])
        XCTAssertEqual(
            harness.reporter.entries,
            [.init(outcome: .blockedByPermission(.unknown), surface: .categoryPrompt)])
    }

    /// The service hands a reminder over once, and it is consumed whether or
    /// not it was posted. So nothing here holds it for later: not a card-only
    /// answer, not notifications being turned on afterwards.
    func testAReminderIsNeverPostedAgain() async throws {
        let harness = Harness()
        harness.permissions.setStatus(.denied, for: .notifications)
        harness.messages.send(.categoryPrompt(withReminder(promptID)))
        try await waitUntil { harness.sut.inFlightNotification != nil }
        await harness.sut.inFlightNotification?.value

        harness.permissions.setStatus(.granted, for: .notifications)
        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        harness.cadence.send()
        try await waitUntil { harness.requests == 1 }
        try await Task.sleep(nanoseconds: 30_000_000)
        XCTAssertEqual(harness.scheduler.scheduledCategoryPrompts, [])

        // Granted, one reminder, answered by the card: still one.
        harness.messages.send(.categoryPrompt(withReminder(laterPromptID)))
        try await waitUntil { harness.scheduler.scheduledCategoryPrompts.count == 1 }
        harness.messages.send(.categoryPrompt(cardOnly(laterPromptID)))
        harness.messages.send(.categoryPrompt(CategoryPrompt()))
        try await Task.sleep(nanoseconds: 30_000_000)
        await harness.sut.inFlightNotification?.value
        XCTAssertEqual(harness.scheduler.scheduledCategoryPrompts.count, 1)
        XCTAssertEqual(
            harness.reporter.entries,
            [
                .init(outcome: .blockedByPermission(.denied), surface: .categoryPrompt),
                .init(outcome: .delivered, surface: .categoryPrompt),
            ])
    }

    // MARK: - The card during a block

    /// A focus session is the one time nothing may interrupt. The service
    /// withholds the card then too, but one it sent a moment before the block
    /// started is still in hand.
    func testTheCardIsHiddenWhileABlockIsActiveOrPausedEvenIfOneIsHeld() async throws {
        let harness = Harness()
        let workBlocks = WorkBlockCoordinator(ipcClient: FakeIPCClient())
        let blockMessages = PassthroughSubject<ServerMessage, Never>()
        workBlocks.start(
            messages: blockMessages, connectionStatus: Empty<ConnectionStatus, Never>(),
            workspaceNotifications: NotificationCenter(), systemNotifications: NotificationCenter())
        let view = CategoryPromptCardView(coordinator: harness.sut, workBlockCoordinator: workBlocks, onOpen: {})

        harness.messages.send(.categoryPrompt(cardOnly(promptID)))
        try await waitUntil { harness.sut.prompt != nil }
        XCTAssertEqual(view.presentedPrompt?.promptID, promptID)

        blockMessages.send(.workBlockState(snapshot(.active)))
        try await waitUntil { workBlocks.snapshot?.phase == .active }
        XCTAssertNotNil(harness.sut.prompt, "the stale card is still held")
        XCTAssertNil(view.presentedPrompt)

        blockMessages.send(.workBlockState(snapshot(.paused)))
        try await waitUntil { workBlocks.snapshot?.phase == .paused }
        XCTAssertNil(view.presentedPrompt)

        blockMessages.send(.workBlockState(snapshot(.completed)))
        try await waitUntil { workBlocks.snapshot?.phase == .completed }
        XCTAssertEqual(view.presentedPrompt?.promptID, promptID)
    }

    func testEveryPhaseOutsideABlockShowsTheCard() {
        let held = PresentedCategoryPrompt(promptID: promptID, card: card)
        for phase in [WorkBlockPhase?.none, .idle, .completed, .abandoned, .expired] {
            XCTAssertEqual(
                CategoryPromptCardView.presentedPrompt(held, during: phase), held, "\(String(describing: phase))")
        }
        for phase in [WorkBlockPhase.active, .paused] {
            XCTAssertNil(CategoryPromptCardView.presentedPrompt(held, during: phase), "\(phase)")
        }
    }

    // MARK: - Fixtures

    private let card = CategoryPromptCard(
        title: "Needs a category",
        body:
            "1 site you used this week doesn't have a category yet. Choose once and it covers every page "
            + "of that site.",
        primaryAction: "Choose a category",
        secondaryAction: "Not now",
        entryCount: 1)

    private let reminder = CategoryPromptNotification(
        title: "A site needs a category",
        body: "1 site you used this week doesn't have a category yet. Choose once in Velvt.")

    private func cardOnly(_ id: String) -> CategoryPrompt {
        CategoryPrompt(promptID: id, card: card)
    }

    private func withReminder(_ id: String) -> CategoryPrompt {
        CategoryPrompt(promptID: id, card: card, notification: reminder)
    }

    private func snapshot(_ phase: WorkBlockPhase) -> WorkBlockSnapshot {
        let live = phase == .active || phase == .paused
        return WorkBlockSnapshot(
            stateVersion: 1,
            phase: phase,
            blockID: live || phase == .completed ? UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa") : nil,
            intention: nil,
            purpose: nil,
            intensity: .medium,
            plannedDurationSeconds: 1_500,
            elapsedDurationSeconds: 300,
            remainingDurationSeconds: 1_200,
            startedAt: Date(timeIntervalSince1970: 1_800_000_000),
            endsAt: Date(timeIntervalSince1970: 1_800_001_500),
            pausedAt: phase == .paused ? Date(timeIntervalSince1970: 1_800_000_300) : nil,
            recoveredAfterRestart: false,
            currentCategory: nil,
            anchorCategory: nil,
            classificationStatus: .classified,
            confidence: .high,
            statusLine: "",
            result: nil,
            activeIntervention: nil
        )
    }

    private func waitUntil(
        timeout: Duration = .seconds(2),
        condition: @escaping @MainActor () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !condition() {
            if clock.now >= deadline {
                XCTFail("condition timed out")
                return
            }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
    }
}

/// A started coordinator and every seam it touches.
@MainActor
private final class Harness {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let cadence = PassthroughSubject<Void, Never>()
    let workspace = NotificationCenter()
    let scheduler = FakeNotificationScheduler()
    let permissions = FakePermissionManager()
    let reporter = RecordingNotificationDeliveryReporter()
    let sut: CategoryPromptCoordinator

    init(utcOffsetSeconds: Int = 0) {
        sut = CategoryPromptCoordinator(
            ipcClient: client,
            scheduler: scheduler,
            permissionManager: permissions,
            reporter: reporter,
            utcOffsetSeconds: { utcOffsetSeconds }
        )
        sut.start(
            messages: messages,
            connectionStatus: client.connectionStatus,
            cadence: cadence,
            workspaceNotifications: workspace
        )
    }

    var requests: Int {
        client.sentMessages.filter {
            if case .requestCategoryPrompt = $0 { return true }
            return false
        }.count
    }
}

/// Where the card and the reminder take the person: Settings → Apps & Sites,
/// through an opening that resets the panel to the Now tab every other time.
@MainActor
final class NeedsACategoryRoutingTests: XCTestCase {
    func testAnOpeningLandsOnARequestedDestinationAndOnlyOnce() {
        var navigator = MenuBarPopoverNavigator()
        navigator.selectWorkspaceTab(.history)

        XCTAssertEqual(navigator.resetForPopoverOpening(requested: .teachApps), .teachApps)
        XCTAssertEqual(navigator.selectedWorkspaceTab, .settings)

        XCTAssertNil(navigator.resetForPopoverOpening(requested: nil))
        XCTAssertEqual(navigator.selectedWorkspaceTab, .workBlock)
    }

    /// With the panel closed, the request is held for the opening it causes,
    /// and taken only by that opening's reset: taken any earlier, the reset
    /// would undo it.
    func testARequestForAClosedPanelWaitsForTheOpeningsReset() {
        let (sut, _) = controller()
        var deliveredNow = 0
        let cancellable = sut.destinationRequests.deliverNow.sink { deliveredNow += 1 }
        defer { cancellable.cancel() }

        sut.showNeedsACategory()

        XCTAssertTrue(sut.isPopoverShown)
        XCTAssertEqual(deliveredNow, 0)
        XCTAssertEqual(sut.destinationRequests.pending, .teachApps)

        // What the panel does on `popoverWillOpen`.
        var navigator = MenuBarPopoverNavigator()
        let destination = navigator.resetForPopoverOpening(requested: sut.destinationRequests.take())
        XCTAssertEqual(destination, .teachApps)
        XCTAssertEqual(navigator.selectedWorkspaceTab, .settings)
        XCTAssertNil(sut.destinationRequests.pending, "the next ordinary opening lands on Now")
        sut.remove()
    }

    /// An open panel gets no opening and so no reset: the request goes to it
    /// at once.
    func testARequestForAnOpenPanelIsDeliveredAtOnce() {
        let (sut, _) = controller()
        sut.showPopover()
        var deliveredNow = 0
        let cancellable = sut.destinationRequests.deliverNow.sink { deliveredNow += 1 }
        defer { cancellable.cancel() }

        sut.showNeedsACategory()

        XCTAssertEqual(deliveredNow, 1)
        XCTAssertEqual(sut.destinationRequests.take(), .teachApps)
        XCTAssertTrue(sut.isPopoverShown)
        sut.remove()
    }

    /// The reminder's tap reaches the list and answers the card, and is
    /// neither a drift offer's tap nor an insight's.
    func testTappingTheReminderOpensTheListAndAnswersOpened() async throws {
        let (menuBar, activations) = controller()
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let prompt = CategoryPromptCoordinator(
            ipcClient: client, scheduler: FakeNotificationScheduler(), permissionManager: FakePermissionManager())
        prompt.start(
            messages: messages, connectionStatus: Empty<ConnectionStatus, Never>(),
            cadence: Empty<Void, Never>(), workspaceNotifications: NotificationCenter())
        let promptID = String(repeating: "f", count: 64)
        messages.send(
            .categoryPrompt(
                CategoryPrompt(
                    promptID: promptID,
                    card: CategoryPromptCard(
                        title: "Needs a category", body: "2 apps you used this week don't have a category yet.",
                        primaryAction: "Choose categories", secondaryAction: "Not now", entryCount: 2))))
        for _ in 0..<200 where prompt.prompt == nil {
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        var openedPopover = 0
        let router = NotificationResponseRouter(
            openPopover: { openedPopover += 1 },
            scrollToDate: ScrollToDateAction { _ in XCTFail("the reminder has no insight date") },
            openNeedsACategory: { [weak menuBar] in
                prompt.open()
                menuBar?.showNeedsACategory()
            }
        )

        router.handle(userInfo: [categoryPromptNotificationUserInfoKey: true])

        XCTAssertEqual(openedPopover, 0, "not the plain opening, which lands on Now")
        XCTAssertTrue(menuBar.isPopoverShown)
        XCTAssertEqual(activations(), 1)
        XCTAssertEqual(menuBar.destinationRequests.pending, .teachApps)
        XCTAssertNil(prompt.prompt)
        for _ in 0..<200 where client.sentMessages.isEmpty {
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        XCTAssertEqual(
            client.sentMessages, [.acknowledgeCategoryPrompt(.init(promptID: promptID, response: .opened))])
        menuBar.remove()
    }

    /// Asserted against the source, as the other proactive cards are: view
    /// composition is not observable from a unit test.
    func testThePanelDrawsTheCardAboveEveryTabAndTakesRequestsAfterItsReset() throws {
        let menuBar = try String(
            contentsOf: URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent("Sources/VelvtMac/UI/MenuBarPopoverView.swift"),
            encoding: .utf8)
        guard let body = menuBar.range(of: "private var selectedWorkspaceContent: some View"),
            let tabSwitch = menuBar.range(
                of: "switch navigator.selectedWorkspaceTab {", range: body.upperBound..<menuBar.endIndex),
            let opening = menuBar.range(of: ".onReceive(popoverWillOpen) {")
        else {
            return XCTFail("the panel body, its tab switch and its opening handler must all still exist")
        }
        let aboveTheSwitch = menuBar[body.upperBound..<tabSwitch.lowerBound]
        XCTAssertTrue(aboveTheSwitch.contains("CategoryPromptCardView("))
        XCTAssertTrue(aboveTheSwitch.contains("workBlockCoordinator: workBlockCoordinator"))
        let handler = menuBar[opening.upperBound...].prefix(400)
            .split(whereSeparator: \.isWhitespace).joined(separator: " ")
        XCTAssertTrue(
            handler.contains("resetForPopoverOpening( requested: destinationRequests.take())"),
            "the opening must take a requested destination through its own reset")
    }

    private func controller() -> (MenuBarController, () -> Int) {
        var activations = 0
        let sut = MenuBarController(
            presentation: PermissionPresentationModel(
                permissionManager: FakePermissionManager(),
                onboardingStateStore: InMemoryOnboardingStateStore()
            ),
            displayCoordinator: ConcreteDisplayDataCoordinator(),
            popover: RoutingTestPopover(),
            statusItemManager: RoutingTestStatusItemManager(),
            activateApp: { activations += 1 }
        )
        sut.install()
        return (sut, { activations })
    }
}

@MainActor
private final class RoutingTestPopover: PopoverPresenting {
    var behavior: NSPopover.Behavior = .transient
    var animates = false
    var contentViewController: NSViewController?
    var contentSize = NSSize.zero
    private(set) var isShown = false

    func show(relativeTo _: NSRect, of _: NSView, preferredEdge _: NSRectEdge) {
        isShown = true
    }

    func close() {
        isShown = false
    }
}

@MainActor
private final class RoutingTestStatusItemManager: StatusItemManaging {
    let button: NSButton? = NSButton()

    func install(target: AnyObject, action: Selector) {
        button?.target = target
        button?.action = action
    }

    func remove() {}
}
