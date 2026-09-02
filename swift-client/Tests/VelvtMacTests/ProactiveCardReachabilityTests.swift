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
/// These assert the state the panel reads and the composition that reads it.
/// `showsFocusSession` appears in none of them, which is the point.
@MainActor
final class ProactiveCardReachabilityTests: XCTestCase {
  func testTheInvitationIsPresentedFromCoordinatorStateAlone() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let cards = WorkBlockProactiveCards(coordinator: coordinator)

    XCTAssertEqual(cards.presentedCards, [])

    messages.send(.initiationInvitation(syntheticInvitation()))
    try await waitUntil { coordinator.invitation != nil }

    XCTAssertEqual(cards.presentedCards, [.invitation])
  }

  /// A drift offer only exists while a block is running, which is precisely the
  /// state in which the bottom bar's button used to read "Start a focus
  /// session". Both halves are asserted here.
  func testTheDriftOfferIsPresentedWhileABlockIsRunning() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let cards = WorkBlockProactiveCards(coordinator: coordinator)

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

  /// Asserted against the source, the way the weekly digest's move out of the
  /// same popover already is: view composition is not observable from a unit
  /// test, and this is the one line that would put the cards back out of reach.
  func testThePanelBodyDrawsTheCardsAndTheFocusPopoverNoLongerDoes() throws {
    let menuBar = try source("MenuBarPopoverView.swift")
    let workBlock = try source("WorkBlockView.swift")

    guard let body = menuBar.range(of: "private var selectedWorkspaceContent: some View"),
      let bottomBar = menuBar.range(of: "private var workspaceBottomBar: some View")
    else {
      return XCTFail("the panel body and the bottom bar must both still exist")
    }
    XCTAssertTrue(
      menuBar[body.upperBound..<bottomBar.lowerBound]
        .contains("WorkBlockProactiveCards(coordinator: workBlockCoordinator)"),
      "the panel body must draw the cards, above the tab content and outside the switch")

    // One home each. The panel and the focus-session popover can be on screen
    // together, and two sets of reply buttons for one drift offer is two
    // chances to record a reply the person did not make.
    XCTAssertFalse(
      workBlock.contains("invitationCard(invitation)\n      }\n      // A live block"),
      "WorkBlockView must not draw a second invitation card")
    XCTAssertFalse(
      workBlock.contains("if let intervention = snapshot.activeIntervention {"),
      "WorkBlockView must not draw a second drift offer card")
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
        "Velvt observed 4 switches away from deep work in the last 10 minutes. "
        + "Protect the next 10 minutes for the work you chose.",
      anchorCategory: "DEEP_WORK",
      switchCount: 4,
      windowSeconds: 600,
      offeredAt: Date(timeIntervalSince1970: 1_800_000_300),
      salience: .quiet
    )
  }

  private func activeSnapshot(activeIntervention: ActiveIntervention? = nil) -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1,
      phase: .active,
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
      intention: "Local intention",
      purpose: .deepWork,
      intensity: .medium,
      plannedDurationSeconds: 1_500,
      elapsedDurationSeconds: 60,
      remainingDurationSeconds: 1_440,
      startedAt: Date(timeIntervalSince1970: 1_800_000_000),
      endsAt: Date(timeIntervalSince1970: 1_800_001_500),
      pausedAt: nil,
      recoveredAfterRestart: false,
      currentCategory: "FOCUS_WORK",
      classificationStatus: .classified,
      confidence: .high,
      statusLine: "Current category: Focus work.",
      result: nil,
      activeIntervention: activeIntervention
    )
  }

  private func waitUntil(
    timeout: Duration = .seconds(1),
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
