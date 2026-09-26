import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

/// The two cards Velvt raises on its own initiative were drawn only by
/// `WorkBlockView`, and `MenuBarPopoverView` instantiates that view in exactly
/// one place: the popover behind "Start a focus session". So the invitation —
/// the one mechanism by which Velvt asks a person to declare a block who has
/// not decided to — could only be seen by someone who had already pressed the
/// button that starts one. And the drift offer, whose notification's whole
/// instruction is "open the panel", was not on the panel.
///
/// These assert the state the panel reads, the composition that reads it, and
/// that moving the drift card onto a surface that outlives being on screen did
/// not turn `card_seen_at` into "the offer arrived".
///
/// First written in the unreviewed remediation commit 707c001 (2026-09-02),
/// dropped when that commit was split into PRs #35-#39, and re-landed on top of
/// the 1.0.11 card styling and the card-sighting report of migration 0032.
@MainActor
final class ProactiveCardReachabilityTests: XCTestCase {
    func testTheInvitationIsPresentedFromCoordinatorStateAlone() async throws {
        let (coordinator, _, messages) = startedCoordinator()
        let cards = WorkBlockProactiveCards(coordinator: coordinator, surfaceIsOnScreen: true)

        XCTAssertEqual(cards.presentedCards, [])

        messages.send(.initiationInvitation(syntheticInvitation()))
        try await waitUntil { coordinator.invitation != nil }

        XCTAssertEqual(cards.presentedCards, [.invitation])
    }

    /// A drift offer only exists while a block is running, which is precisely the
    /// state in which the bottom bar's button used to read "Start a focus
    /// session". Both halves are asserted here.
    func testTheDriftOfferIsPresentedWhileABlockIsRunning() async throws {
        let (coordinator, _, messages) = startedCoordinator()
        let cards = WorkBlockProactiveCards(coordinator: coordinator, surfaceIsOnScreen: true)

        messages.send(.workBlockState(activeSnapshot(activeIntervention: offer())))
        try await waitUntil { coordinator.snapshot?.activeIntervention != nil }

        XCTAssertEqual(cards.presentedCards, [.intervention])
        XCTAssertEqual(
            MenuBarFocusSessionButtonLabel.title(for: coordinator.snapshot?.phase),
            "Current work block"
        )
    }

    func testThePrimaryActionOffersToStartABlockOnlyWhenNoneIsRunning() {
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: nil), "Start a focus session")
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: .idle), "Start a focus session")
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: .completed), "Start a focus session")
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: .abandoned), "Start a focus session")
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: .expired), "Start a focus session")
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: .active), "Current work block")
        XCTAssertEqual(MenuBarFocusSessionButtonLabel.title(for: .paused), "Current work block")
    }

    /// The panel's view tree is built once and survives every closing, so a
    /// card inserted while the panel is ordered out "appears" to SwiftUI with
    /// nobody looking. The sighting is sent only while the surface is on
    /// screen; the offer arriving is not a sighting.
    func testTheDriftCardIsReportedSeenOnlyWhileItsSurfaceIsOnScreen() async throws {
        let (coordinator, client, messages) = startedCoordinator()
        messages.send(.workBlockState(activeSnapshot(activeIntervention: offer())))
        try await waitUntil { coordinator.snapshot?.activeIntervention != nil }
        let seen = ClientMessage.interventionCardSeen(.init(blockID: blockID))

        WorkBlockProactiveCards(coordinator: coordinator, surfaceIsOnScreen: false)
            .reportSightingIfOnScreen()
        // The coordinator sends on a task; give it the turn it would have taken.
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertFalse(client.sentMessages.contains(seen), "an off-screen card was reported seen")

        WorkBlockProactiveCards(coordinator: coordinator, surfaceIsOnScreen: true)
            .reportSightingIfOnScreen()
        try await waitUntil { client.sentMessages.contains(seen) }
    }

    /// With no offer live there is nothing to have seen, on screen or not.
    func testNoSightingIsReportedWithoutALiveOffer() async throws {
        let (coordinator, client, messages) = startedCoordinator()
        messages.send(.workBlockState(activeSnapshot(activeIntervention: nil)))
        try await waitUntil { coordinator.snapshot != nil }
        let sentBefore = client.sentMessages.count

        WorkBlockProactiveCards(coordinator: coordinator, surfaceIsOnScreen: true)
            .reportSightingIfOnScreen()
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(client.sentMessages.count, sentBefore)
    }

    func testAWindowThatWasNeverOrderedInIsNotOnScreen() {
        XCTAssertFalse(MenuBarWindowOnScreenReader.isOnScreen(nil))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 100, height: 100),
            styleMask: [.borderless],
            backing: .buffered,
            defer: true
        )
        window.isReleasedWhenClosed = false
        XCTAssertFalse(MenuBarWindowOnScreenReader.isOnScreen(window))
    }

    /// Asserted against the source, the way the weekly digest's move out of the
    /// same popover already is: view composition is not observable from a unit
    /// test, and each of these is one line away from putting a card back out of
    /// reach, or from reporting a sighting nobody had.
    func testThePanelBodyDrawsTheCardsAndTheFocusPopoverNoLongerDoes() throws {
        let menuBar = try source("MenuBarPopoverView.swift")
        let workBlock = try source("WorkBlockView.swift")

        guard let body = menuBar.range(of: "private var selectedWorkspaceContent: some View"),
            let tabSwitch = menuBar.range(
                of: "switch navigator.selectedWorkspaceTab {",
                range: body.upperBound..<menuBar.endIndex),
            let bottomBar = menuBar.range(of: "private var workspaceBottomBar: some View")
        else {
            return XCTFail("the panel body, its tab switch and the bottom bar must all still exist")
        }
        let aboveTheSwitch = menuBar[body.upperBound..<tabSwitch.lowerBound]
        XCTAssertTrue(
            aboveTheSwitch.contains("WorkBlockProactiveCards("),
            "the panel body must draw the cards above the tab content, outside the switch")
        XCTAssertTrue(
            aboveTheSwitch.contains("surfaceIsOnScreen: panelIsOnScreen"),
            "the cards must be told whether the panel is actually on screen")
        XCTAssertTrue(
            menuBar.contains(".background(MenuBarWindowOnScreenReader(isOnScreen: $panelIsOnScreen))"),
            "the panel's on-screen state must come from its window")
        XCTAssertTrue(
            menuBar[bottomBar.upperBound...].contains("MenuBarFocusSessionButtonLabel.title("),
            "the bottom bar's button must be named for the block's phase")

        // One home each. The panel and the focus-session popover can be on
        // screen together, and two sets of reply buttons for one drift offer is
        // two chances to record a reply the person did not make.
        guard let workBlockView = workBlock.range(of: "public struct WorkBlockView: View {"),
            let cardsView = workBlock.range(of: "public struct WorkBlockProactiveCards: View {")
        else {
            return XCTFail("both views must still exist")
        }
        let popoverSource =
            workBlockView.upperBound < cardsView.lowerBound
            ? workBlock[workBlockView.upperBound..<cardsView.lowerBound]
            : workBlock[workBlockView.upperBound...]
        XCTAssertFalse(popoverSource.contains("invitationCard("), "WorkBlockView must not draw an invitation card")
        XCTAssertFalse(popoverSource.contains("interventionCard("), "WorkBlockView must not draw a drift offer card")
        XCTAssertFalse(
            popoverSource.contains("reportInterventionCardSeen()"),
            "only the card's own view may report it seen")
    }

    // MARK: - Fixtures

    private let blockID = UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!

    private func startedCoordinator() -> (
        WorkBlockCoordinator, FakeIPCClient, PassthroughSubject<ServerMessage, Never>
    ) {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let coordinator = WorkBlockCoordinator(ipcClient: client)
        coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
        return (coordinator, client, messages)
    }

    private func source(_ name: String) throws -> String {
        try String(
            contentsOf: URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("Sources/VelvtMac/UI/\(name)"),
            encoding: .utf8)
    }

    private func syntheticInvitation() -> InitiationInvitation {
        InitiationInvitation(
            invitationID: UUID(uuidString: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")!,
            actionID: "soft_start_25",
            body: "You usually focus well around now — want a 25-minute soft start?",
            durationSeconds: 1_500,
            policyVersion: 1
        )
    }

    private func offer() -> ActiveIntervention {
        ActiveIntervention(
            actionID: "protect_next_10",
            title: "Your work block is running",
            body:
                "Velvt observed 3 switches away from focus work in the last 10 minutes. "
                + "Protect the next 10 minutes for the work you chose.",
            anchorCategory: "FOCUS_WORK",
            switchCount: 3,
            windowSeconds: 600,
            offeredAt: Date(timeIntervalSince1970: 1_800_000_300),
            salience: .quiet
        )
    }

    private func activeSnapshot(activeIntervention: ActiveIntervention?) -> WorkBlockSnapshot {
        WorkBlockSnapshot(
            stateVersion: 1,
            phase: .active,
            blockID: blockID,
            intention: "Local intention",
            purpose: .deepWork,
            intensity: .medium,
            plannedDurationSeconds: 1_500,
            elapsedDurationSeconds: 300,
            remainingDurationSeconds: 1_200,
            startedAt: Date(timeIntervalSince1970: 1_800_000_000),
            endsAt: Date(timeIntervalSince1970: 1_800_001_500),
            pausedAt: nil,
            recoveredAfterRestart: false,
            currentCategory: "COMMUNICATION",
            anchorCategory: "FOCUS_WORK",
            classificationStatus: .classified,
            confidence: .high,
            statusLine: "Current category: Communication.",
            result: nil,
            activeIntervention: activeIntervention
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
            await Task.yield()
        }
    }
}
