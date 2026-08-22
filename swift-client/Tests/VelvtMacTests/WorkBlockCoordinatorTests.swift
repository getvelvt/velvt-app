import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

@MainActor
final class WorkBlockCoordinatorTests: XCTestCase {
  private var cancellables = Set<AnyCancellable>()

  func testLocalDashboardRequestIncludesBoundedWindowAndLocalDayOffset() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = LocalDashboardCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    client.setConnectionStatus(.connected)
    try await waitUntil {
      client.sentMessages.contains { message in
        guard case .requestLocalDashboard(let request) = message else { return false }
        return request.windowSeconds == 3_600
          && request.utcOffsetSeconds == TimeZone.current.secondsFromGMT()
      }
    }
  }

  func testConnectedRequestsStateOnceWithoutPolling() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    client.setConnectionStatus(.connected)
    client.setConnectionStatus(.connected)
    try await waitUntil { client.sentMessages.contains(.requestWorkBlockState) }
    try await Task.sleep(for: .milliseconds(50))

    XCTAssertEqual(client.sentMessages.filter { $0 == .requestWorkBlockState }.count, 1)
  }

  func testQuietHoursOfferIsRenderedVerbatimAndOneTapReplyClearsIt() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let offer = QuietHoursOffer(
      ruleVersion: 1,
      lateNightDays: 3,
      startLocalMinutes: 1_320,
      endLocalMinutes: 420,
      body: "Velvt can hold its own notifications overnight."
    )

    messages.send(.quietHoursOffer(offer))
    try await waitUntil { coordinator.quietHoursOffer == offer }

    coordinator.respondToQuietHoursOffer(accepted: false)
    XCTAssertNil(coordinator.quietHoursOffer, "one tap resolves the card")
    try await waitUntil {
      client.sentMessages.contains(.respondQuietHoursOffer(.init(accepted: false)))
    }

    // A second tap with no live offer sends nothing: the decline is
    // remembered by the service and never re-negotiated from Swift.
    let sentBefore = client.sentMessages.count
    coordinator.respondToQuietHoursOffer(accepted: true)
    try await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(client.sentMessages.count, sentBefore)
  }

  func testSnapshotIsRenderedAsReceivedAndCommandsContainNoSwiftEvidence() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let snapshot = activeSnapshot()

    messages.send(.workBlockState(snapshot))
    try await waitUntil { coordinator.snapshot == snapshot }
    XCTAssertEqual(coordinator.snapshot, snapshot)
    coordinator.pause()
    try await waitUntil {
      client.sentMessages.contains(.pauseWorkBlock(.init(blockID: snapshot.blockID!)))
    }

    let pauseMessage = try XCTUnwrap(client.sentMessages.last)
    let encoded = try IPCMessageCodec.makeEncoder().encode(pauseMessage)
    let text = String(decoding: encoded, as: UTF8.self)
    XCTAssertFalse(text.contains(snapshot.statusLine))
    XCTAssertFalse(text.contains("observation"))
  }

  func testStartCarriesIntentionOnlyOnLocalStartCommand() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let sentinel = "PRIVATE_INTENTION_SWIFT_SENTINEL"

    coordinator.startBlock(
      intention: sentinel,
      durationSeconds: 1_500,
      purpose: .study,
      intensity: .light
    )
    try await waitUntil { !client.sentMessages.isEmpty }
    let startJSON = String(
      decoding: try IPCMessageCodec.makeEncoder().encode(client.sentMessages.last!),
      as: UTF8.self
    )
    XCTAssertTrue(startJSON.contains(sentinel))

    let notification = NotificationPayload(
      notificationID: UUID(),
      title: "Safe title",
      body: "Safe body",
      insightDate: "2027-01-15",
      doNotDisturbUntil: nil
    )
    let notificationJSON = String(
      decoding: try IPCMessageCodec.makeEncoder().encode(
        ServerMessage.notificationPayload(notification)),
      as: UTF8.self
    )
    XCTAssertFalse(notificationJSON.contains(sentinel))
    XCTAssertEqual(
      ServerMessage.workBlockState(activeSnapshot()).safeLogDescription, "work_block_state")
    XCTAssertFalse(
      ServerMessage.workBlockState(activeSnapshot()).safeLogDescription.contains("Local intention"))
  }

  func testSleepWakeClockAndTimeZoneAreEventDrivenCommands() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let workspace = NotificationCenter()
    let system = NotificationCenter()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(
      messages: messages,
      connectionStatus: client.connectionStatus,
      workspaceNotifications: workspace,
      systemNotifications: system
    )

    workspace.post(name: NSWorkspace.willSleepNotification, object: nil)
    workspace.post(name: NSWorkspace.didWakeNotification, object: nil)
    system.post(name: .NSSystemClockDidChange, object: nil)
    system.post(name: .NSSystemTimeZoneDidChange, object: nil)
    try await waitUntil { client.sentMessages.count == 6 }

    // Each OS boundary maps to exactly one lifecycle command — no polling.
    // Wake additionally asks the initiation policy once, because a machine
    // waking is exactly the moment a stored invitation may have gone
    // stale, and asks once whether a digest week completed while asleep.
    let lifecycleMessages = client.sentMessages.filter {
      if case .workBlockLifecycle = $0 { return true }
      return false
    }
    XCTAssertEqual(
      lifecycleMessages,
      [
        .workBlockLifecycle(.init(event: .sleep)),
        .workBlockLifecycle(.init(event: .wake)),
        .workBlockLifecycle(.init(event: .clockChanged)),
        .workBlockLifecycle(.init(event: .timeZoneChanged)),
      ])
    XCTAssertEqual(
      client.sentMessages.filter {
        if case .requestInitiationInvitation = $0 { return true }
        return false
      }.count,
      1,
      "wake refreshes the invitation exactly once"
    )
    XCTAssertEqual(
      client.sentMessages.filter {
        if case .requestWeeklyDigest = $0 { return true }
        return false
      }.count,
      1,
      "wake asks for the digest exactly once"
    )
  }

  func testOfflineServiceDoesNotCreateOptimisticLocalState() async throws {
    let client = FakeIPCClient()
    client.shouldThrowOnSend = IPCError.notConnected
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    coordinator.startBlock(
      intention: "Local draft",
      durationSeconds: 1_500,
      purpose: nil,
      intensity: .medium
    )
    try await waitUntil { coordinator.commandError != nil }

    XCTAssertNil(coordinator.snapshot)
    XCTAssertTrue(coordinator.commandError?.contains("offline") == true)
  }

  func testEachInterventionResponseIsSentToTheService() async throws {
    // Exhaustive by construction: a reply added to the protocol without a
    // path to the service would fail here rather than be silently unreportable.
    for response in InterventionResponse.allCases {
      let client = FakeIPCClient()
      let messages = PassthroughSubject<ServerMessage, Never>()
      let coordinator = WorkBlockCoordinator(ipcClient: client)
      coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
      client.setConnectionStatus(.connected)
      messages.send(.workBlockState(activeSnapshot(activeIntervention: offer())))
      try await waitUntil { coordinator.snapshot?.activeIntervention != nil }

      coordinator.respondToIntervention(response)

      try await waitUntil {
        client.sentMessages.contains { message in
          guard case .reportInterventionOutcome(let report) = message else { return false }
          return report.response == response
        }
      }
    }
  }

  /// A stale view must not report against an offer the service already
  /// resolved, or silence and disagreement stop being distinguishable.
  func testNoResponseIsSentWhenThereIsNoLiveOffer() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    client.setConnectionStatus(.connected)
    messages.send(.workBlockState(activeSnapshot()))
    try await waitUntil { coordinator.snapshot != nil }

    coordinator.respondToIntervention(.dismissed)
    try await Task.sleep(for: .milliseconds(50))

    XCTAssertFalse(
      client.sentMessages.contains { message in
        if case .reportInterventionOutcome = message { return true }
        return false
      })
  }

  func testActiveInterventionSurvivesAnEncodeDecodeRoundTrip() throws {
    let snapshot = activeSnapshot(activeIntervention: offer())
    let encoded = try JSONEncoder().encode(snapshot)
    let decoded = try JSONDecoder().decode(WorkBlockSnapshot.self, from: encoded)

    XCTAssertEqual(decoded.activeIntervention, snapshot.activeIntervention)
    XCTAssertEqual(decoded.activeIntervention?.actionID, "protect_next_10")
    XCTAssertEqual(decoded.activeIntervention?.switchCount, 4)
    XCTAssertEqual(decoded.activeIntervention?.salience, .quiet)
  }

  /// Salience is carried on the wire rather than inferred locally: Rust decides
  /// how loudly to ask, and a quiet offer means the notification was never sent.
  func testSalienceDecodesFromTheServicePayload() throws {
    let payload = Data(
      """
      {
        "action_id": "protect_next_10",
        "title": "Your work block is running",
        "body": "Velvt observed 4 switches away from deep work.",
        "anchor_category": "DEEP_WORK",
        "switch_count": 4,
        "window_seconds": 600,
        "offered_at": "2027-01-15T10:05:00Z",
        "salience": "quiet"
      }
      """.utf8
    )

    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .iso8601
    let decoded = try decoder.decode(ActiveIntervention.self, from: payload)

    XCTAssertEqual(decoded.salience, .quiet)
  }

  /// v27: the invitation renders verbatim, one tap accepts through the
  /// existing start command carrying the invitation id, and a stale second
  /// tap sends nothing. Swift never re-derives good hours or backoff.
  func testInvitationRendersVerbatimAndAcceptStartsDeclaredBlockWithClaim() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let invitation = syntheticInvitation()

    messages.send(.initiationInvitation(invitation))
    try await waitUntil { coordinator.invitation == invitation }

    coordinator.acceptInvitation()
    XCTAssertNil(coordinator.invitation, "one tap resolves the card")
    try await waitUntil {
      client.sentMessages.contains(
        .startWorkBlock(
          .init(
            intention: nil,
            plannedDurationSeconds: invitation.durationSeconds,
            purpose: nil,
            intensity: .medium,
            invitationID: invitation.invitationID
          )))
    }

    // A second tap with no live invitation sends nothing.
    let sentBefore = client.sentMessages.count
    coordinator.acceptInvitation()
    coordinator.dismissInvitation()
    try await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(client.sentMessages.count, sentBefore)
  }

  func testInvitationDismissalSendsOneContentFreeRecord() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    let invitation = syntheticInvitation()

    messages.send(.initiationInvitation(invitation))
    try await waitUntil { coordinator.invitation == invitation }

    coordinator.dismissInvitation()
    XCTAssertNil(coordinator.invitation)
    try await waitUntil {
      client.sentMessages.contains(
        .dismissInitiationInvitation(.init(invitationID: invitation.invitationID)))
    }
    // The dismissal carries the opaque id and nothing else.
    let encoded = String(
      decoding: try IPCMessageCodec.makeEncoder().encode(client.sentMessages.last!),
      as: UTF8.self)
    XCTAssertFalse(encoded.contains(invitation.body))
    for scheduleShaped in ["hour", "weekday", "bucket", "window"] {
      XCTAssertFalse(encoded.contains(scheduleShaped))
    }
  }

  /// The settings toggle renders the Rust-owned state and a live block
  /// clears the invitation card.
  func testInvitationSettingsAndLiveBlockControlTheCard() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    XCTAssertTrue(coordinator.invitationsEnabled, "renders on until the service reports")
    messages.send(.initiationSettings(.init(invitationsEnabled: false)))
    try await waitUntil { !coordinator.invitationsEnabled }

    coordinator.setInvitationsEnabled(true)
    try await waitUntil {
      client.sentMessages.contains(.setInitiationSettings(.init(invitationsEnabled: true)))
    }
    // The toggle state follows the service reply, not the tap.
    XCTAssertFalse(coordinator.invitationsEnabled)
    messages.send(.initiationSettings(.init(invitationsEnabled: true)))
    try await waitUntil { coordinator.invitationsEnabled }

    messages.send(.initiationInvitation(syntheticInvitation()))
    try await waitUntil { coordinator.invitation != nil }
    messages.send(.workBlockState(activeSnapshot()))
    try await waitUntil { coordinator.invitation == nil }
  }

  func testConnectRequestsInvitationAndSettingsOnce() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client, utcOffsetSeconds: { -28_800 })
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    client.setConnectionStatus(.connected)
    try await waitUntil {
      client.sentMessages.contains(
        .requestInitiationInvitation(.init(utcOffsetSeconds: -28_800)))
        && client.sentMessages.contains(.requestInitiationSettings)
    }
  }

  /// Scope 4: the demotion disclosure renders the service state verbatim,
  /// the reset is guarded on being demoted, and the reply re-renders the
  /// new state.
  func testDemotionStateRendersVerbatimAndResetIsGuardedOneTap() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    // Active state: reset sends nothing.
    messages.send(.demotionState(syntheticDemotionState(kind: .active)))
    try await waitUntil { coordinator.demotionState?.state == .active }
    coordinator.resetDemotion()
    try await Task.sleep(for: .milliseconds(50))
    XCTAssertFalse(client.sentMessages.contains(.resetInterventionDemotion))

    // Demoted state: the disclosure is present and reset sends exactly the
    // registered command.
    let demoted = syntheticDemotionState(kind: .demoted)
    messages.send(.demotionState(demoted))
    try await waitUntil { coordinator.demotionState == demoted }
    XCTAssertNotNil(coordinator.demotionState?.disclosure)
    coordinator.resetDemotion()
    try await waitUntil { client.sentMessages.contains(.resetInterventionDemotion) }

    messages.send(.demotionState(syntheticDemotionState(kind: .active)))
    try await waitUntil { coordinator.demotionState?.state == .active }
  }

  /// Scope 4: the digest renders the stored counts verbatim; the one-tap
  /// acknowledgment closes the card and sends the week key only.
  func testWeeklyDigestRendersStoredCountsAndAcknowledgeClosesIt() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client, utcOffsetSeconds: { -28_800 })
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    client.setConnectionStatus(.connected)
    try await waitUntil {
      client.sentMessages.contains(.requestWeeklyDigest(.init(utcOffsetSeconds: -28_800)))
        && client.sentMessages.contains(.requestDemotionState)
    }

    let digest = WeeklyDigest(
      weekStartLocalDate: "2026-07-27",
      blocksDeclared: 5,
      blocksCompleted: 3,
      recoveries: 4,
      wrongInterventions: 1,
      invitationsAccepted: 2,
      withheld: 1,
      headline: "You returned 4 times and completed 3 of 5 blocks this week.",
      digestVersion: 1
    )
    messages.send(.weeklyDigest(digest))
    try await waitUntil { coordinator.weeklyDigest == digest }

    coordinator.acknowledgeWeeklyDigest()
    XCTAssertNil(coordinator.weeklyDigest, "one tap closes the card")
    try await waitUntil {
      client.sentMessages.contains(
        .acknowledgeWeeklyDigest(.init(weekStartLocalDate: "2026-07-27")))
    }

    // A second tap with no card sends nothing: there is no reply surface.
    let sentBefore = client.sentMessages.count
    coordinator.acknowledgeWeeklyDigest()
    try await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(client.sentMessages.count, sentBefore)
  }

  /// Scope 4 (D7): the explain tap is guarded on a live card, carries no
  /// user text (the DTO has no text field), renders the one sentence
  /// verbatim, and the sentence leaves with the card. No input, no reply.
  func testExplainTapIsOneShotGuardedAndSentenceLeavesWithTheCard() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client, utcOffsetSeconds: { 3_600 })
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)

    // No live card: the tap sends nothing.
    messages.send(.workBlockState(activeSnapshot()))
    try await waitUntil { coordinator.snapshot != nil }
    coordinator.requestExplanation()
    try await Task.sleep(for: .milliseconds(50))
    XCTAssertFalse(
      client.sentMessages.contains { message in
        if case .requestInterventionExplanation = message { return true }
        return false
      })

    // Live card: exactly the registered request is sent.
    messages.send(.workBlockState(activeSnapshot(activeIntervention: offer())))
    try await waitUntil { coordinator.snapshot?.activeIntervention != nil }
    coordinator.requestExplanation()
    let blockID = UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!
    try await waitUntil {
      client.sentMessages.contains(
        .requestInterventionExplanation(.init(blockID: blockID, utcOffsetSeconds: 3_600)))
    }

    let explanation = InterventionExplanation(
      blockID: blockID,
      sentence:
        "Velvt offered this nudge because it observed 4 switches away from deep work in the 10 minutes before the offer."
    )
    messages.send(.interventionExplanation(explanation))
    try await waitUntil { coordinator.explanation == explanation }

    // The card resolves; the sentence goes with it.
    messages.send(.workBlockState(activeSnapshot()))
    try await waitUntil { coordinator.explanation == nil }
  }

  private func syntheticDemotionState(kind: DemotionStateKind) -> DemotionState {
    DemotionState(
      state: kind,
      wrongCount: kind == .demoted ? 4 : 1,
      deliveredCount: 16,
      thresholdPercent: 15,
      minimumSample: 10,
      windowDays: 14,
      thresholdPolicyVersion: 1,
      repromotionPolicyVersion: 1,
      demotedAt: kind == .demoted ? Date(timeIntervalSince1970: 1_800_000_000) : nil,
      disclosure: kind == .demoted
        ? "Velvt is getting these nudges wrong too often, so it has gone quiet: no nudges will be sent for now, and you can resume them at any time."
        : nil
    )
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

  private func activeSnapshot(
    activeIntervention: ActiveIntervention? = nil
  ) -> WorkBlockSnapshot {
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

// MARK: - Work-block minutes, rendered

/// Renders every state the minutes surface can be in, from synthetic
/// Rust-shaped snapshots, so the numbers can be looked at rather than
/// reasoned about. Skipped unless `VELVT_WORKBLOCK_SCREENSHOT_DIR` names an
/// output directory, exactly like the other synthetic snapshot tests. No app
/// launch and no Accessibility permission.
///
/// The live cases matter most here: the service pushes work-block state on
/// commands and one deadline, never on a timer, so a snapshot that arrived at
/// t=0 is what a panel opened at t=10min is holding. Each "stale" case below
/// is that snapshot, rendered at the wall-clock moment the panel opens.
@MainActor
final class WorkBlockMinutesSnapshotTests: XCTestCase {
  func testRenderWorkBlockMinutesStatesWhenRequested() async throws {
    guard let output = ProcessInfo.processInfo.environment["VELVT_WORKBLOCK_SCREENSHOT_DIR"]
    else {
      throw XCTSkip("Set VELVT_WORKBLOCK_SCREENSHOT_DIR to render work-block minutes screenshots")
    }

    // 1-3. Duration selection: the default, the schema minimum, the maximum.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: idleSnapshot())),
      named: "01-startform-default-25.png", outputDirectory: output,
      size: NSSize(width: 400, height: 390))
    try render(
      WorkBlockView(
        coordinator: try await coordinator(with: idleSnapshot()),
        durationChoice: .custom, customMinutes: 5),
      named: "02-startform-custom-minimum-5.png", outputDirectory: output,
      size: NSSize(width: 400, height: 430))
    try render(
      WorkBlockView(
        coordinator: try await coordinator(with: idleSnapshot()),
        durationChoice: .custom, customMinutes: 180),
      named: "03-startform-custom-maximum-180.png", outputDirectory: output,
      size: NSSize(width: 400, height: 430))

    // 4. A 25-minute block whose snapshot just arrived.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 1_500,
        elapsedAtPush: 0, pushedSecondsAgo: 0))),
      named: "04-active-25m-fresh.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 5. The same block, ten minutes later, with the same snapshot: nothing
    //    has pushed since the start command.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 1_500,
        elapsedAtPush: 0, pushedSecondsAgo: 600))),
      named: "05-active-25m-ten-minutes-in-stale-snapshot.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 6. The minimum block, one minute in.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 300,
        elapsedAtPush: 0, pushedSecondsAgo: 60))),
      named: "06-active-5m-minimum.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 7. The maximum block, ninety minutes in.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 10_800,
        elapsedAtPush: 0, pushedSecondsAgo: 5_400))),
      named: "07-active-3h-maximum.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 8. Ending while the panel is open: the deadline is one second away.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 1_500,
        elapsedAtPush: 1_499, pushedSecondsAgo: 0))),
      named: "08-active-ending-while-open.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 8b. Past its own deadline while the panel is open. The deadline sleep
    //     fires at `ends_at`, but a disconnected or busy service leaves the
    //     last active snapshot on screen after that instant.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 1_500,
        elapsedAtPush: 1_500, pushedSecondsAgo: 45))),
      named: "08b-active-past-deadline-no-push-yet.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 9. Paused mid-block. Elapsed and remaining both exclude paused time.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: pausedSnapshot(planned: 1_500,
        elapsed: 610))),
      named: "09-paused-25m.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 10. Paused inside the three-hour maximum: the case the clock format has
    //     to survive.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: pausedSnapshot(planned: 10_800,
        elapsed: 60))),
      named: "10-paused-3h-maximum.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 11. Recovered after a restart.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: activeSnapshot(planned: 1_500,
        elapsedAtPush: 300, pushedSecondsAgo: 0, recovered: true))),
      named: "11-active-recovered-after-restart.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 12. The reported screenshot: 25m planned, 25m elapsed, a 17-second
    //     longest stretch and five switches over 12% observed coverage.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: completedSnapshot(
        planned: 1_500, elapsed: 1_500, longest: 17, switches: 5,
        coverage: .partial, ratio: 0.12, confidence: .low))),
      named: "12-result-dead-collection.png", outputDirectory: output,
      size: NSSize(width: 400, height: 400))

    // 13. The same card with good coverage, for comparison.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: completedSnapshot(
        planned: 1_500, elapsed: 1_487, longest: 754, switches: 3,
        coverage: .good, ratio: 0.96, confidence: .high))),
      named: "13-result-good-coverage.png", outputDirectory: output,
      size: NSSize(width: 400, height: 400))

    // 14. A ninety-second longest stretch: the value the minute-only rule
    //     used to round away.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: completedSnapshot(
        planned: 300, elapsed: 299, longest: 90, switches: 4,
        coverage: .partial, ratio: 0.44, confidence: .low))),
      named: "14-result-5m-ninety-second-stretch.png", outputDirectory: output,
      size: NSSize(width: 400, height: 400))

    // 15. The three-hour maximum, completed.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: completedSnapshot(
        planned: 10_800, elapsed: 10_800, longest: 4_215, switches: 22,
        coverage: .good, ratio: 0.91, confidence: .high))),
      named: "15-result-3h-maximum.png", outputDirectory: output,
      size: NSSize(width: 400, height: 400))

    // 16. Nothing observed at all.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: completedSnapshot(
        planned: 1_500, elapsed: 1_500, longest: 0, switches: 0,
        coverage: .insufficient, ratio: 0.0, confidence: .low))),
      named: "16-result-no-observation.png", outputDirectory: output,
      size: NSSize(width: 400, height: 400))

    // 17. Abandoned before the service wrote a result row.
    try render(
      WorkBlockView(coordinator: try await coordinator(with: abandonedWithoutResult())),
      named: "17-result-abandoned-without-result.png", outputDirectory: output,
      size: NSSize(width: 400, height: 260))

    // 18-19. The one-line live control on the dashboard tab, fresh and ten
    //        minutes stale.
    try render(
      CompactWorkBlockControl(
        snapshot: activeSnapshot(planned: 1_500, elapsedAtPush: 0, pushedSecondsAgo: 0),
        coordinator: try await coordinator(with: idleSnapshot())),
      named: "18-compact-live-fresh.png", outputDirectory: output,
      size: NSSize(width: 460, height: 90))
    try render(
      CompactWorkBlockControl(
        snapshot: activeSnapshot(planned: 1_500, elapsedAtPush: 0, pushedSecondsAgo: 600),
        coordinator: try await coordinator(with: idleSnapshot())),
      named: "19-compact-live-ten-minutes-in-stale-snapshot.png", outputDirectory: output,
      size: NSSize(width: 460, height: 90))
    try render(
      CompactWorkBlockControl(
        snapshot: pausedSnapshot(planned: 10_800, elapsed: 3_671),
        coordinator: try await coordinator(with: idleSnapshot())),
      named: "20-compact-paused-3h.png", outputDirectory: output,
      size: NSSize(width: 460, height: 90))
  }

  // MARK: Fixtures

  private func coordinator(with snapshot: WorkBlockSnapshot) async throws -> WorkBlockCoordinator {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    messages.send(.workBlockState(snapshot))
    let deadline = ContinuousClock().now.advanced(by: .seconds(1))
    while coordinator.snapshot == nil {
      if ContinuousClock().now >= deadline { XCTFail("snapshot never arrived"); break }
      await Task.yield()
    }
    return coordinator
  }

  private func idleSnapshot() -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1, phase: .idle, blockID: nil, intention: nil, purpose: nil, intensity: nil,
      plannedDurationSeconds: 0, elapsedDurationSeconds: 0, remainingDurationSeconds: 0,
      startedAt: nil, endsAt: nil, pausedAt: nil, recoveredAfterRestart: false,
      currentCategory: nil, classificationStatus: .unclassified, confidence: .none,
      statusLine: "Choose one bounded block to begin.", result: nil, activeIntervention: nil)
  }

  /// An active block whose snapshot was pushed `pushedSecondsAgo` ago with
  /// `elapsedAtPush` on it. `ends_at` is `started_at + planned + paused`, which
  /// is what the service sends, so it stays correct as wall-clock advances.
  private func activeSnapshot(
    planned: Int, elapsedAtPush: Int, pushedSecondsAgo: Int, recovered: Bool = false
  ) -> WorkBlockSnapshot {
    let pushedAt = Date().addingTimeInterval(-TimeInterval(pushedSecondsAgo))
    let startedAt = pushedAt.addingTimeInterval(-TimeInterval(elapsedAtPush))
    return WorkBlockSnapshot(
      stateVersion: 1, phase: .active,
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
      intention: "Draft the report", purpose: .deepWork, intensity: .medium,
      plannedDurationSeconds: planned, elapsedDurationSeconds: elapsedAtPush,
      remainingDurationSeconds: max(0, planned - elapsedAtPush),
      startedAt: startedAt, endsAt: startedAt.addingTimeInterval(TimeInterval(planned)),
      pausedAt: nil, recoveredAfterRestart: recovered,
      currentCategory: "FOCUS_WORK", classificationStatus: .classified, confidence: .high,
      statusLine: "Current category: Focus work.", result: nil, activeIntervention: nil)
  }

  private func pausedSnapshot(planned: Int, elapsed: Int) -> WorkBlockSnapshot {
    let now = Date()
    return WorkBlockSnapshot(
      stateVersion: 1, phase: .paused,
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
      intention: "Draft the report", purpose: .deepWork, intensity: .medium,
      plannedDurationSeconds: planned, elapsedDurationSeconds: elapsed,
      remainingDurationSeconds: max(0, planned - elapsed),
      startedAt: now.addingTimeInterval(-TimeInterval(elapsed) - 120), endsAt: nil,
      pausedAt: now.addingTimeInterval(-120), recoveredAfterRestart: false,
      currentCategory: "FOCUS_WORK", classificationStatus: .classified, confidence: .high,
      statusLine: "Paused. Nothing is being recorded for this block.",
      result: nil, activeIntervention: nil)
  }

  private func completedSnapshot(
    planned: Int, elapsed: Int, longest: Int, switches: Int,
    coverage: WorkBlockCoverage, ratio: Double, confidence: ConfidenceLevel
  ) -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1, phase: .completed,
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
      intention: "Draft the report", purpose: .deepWork, intensity: .medium,
      plannedDurationSeconds: planned, elapsedDurationSeconds: elapsed,
      remainingDurationSeconds: 0,
      startedAt: Date(timeIntervalSince1970: 1_800_000_000),
      endsAt: nil, pausedAt: nil, recoveredAfterRestart: false,
      currentCategory: nil, classificationStatus: .classified, confidence: .high,
      statusLine: "This block is finished.",
      result: WorkBlockResult(
        plannedDurationSeconds: planned, elapsedDurationSeconds: elapsed,
        longestUninterruptedSeconds: longest, switchAwayCount: switches, recoveryCount: 4,
        confidence: confidence, coverage: coverage, coverageRatio: ratio,
        safeEvidenceCategory: "FOCUS_WORK",
        observation: "Velvt observed one switching cluster in this work-block window.",
        nextAction: WorkBlockNextAction(
          actionID: "plan_next", label: "Plan another session", durationSeconds: 1_500)),
      activeIntervention: nil)
  }

  private func abandonedWithoutResult() -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1, phase: .abandoned,
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
      intention: "Draft the report", purpose: .deepWork, intensity: .medium,
      plannedDurationSeconds: 1_500, elapsedDurationSeconds: 412,
      remainingDurationSeconds: 1_088,
      startedAt: Date(timeIntervalSince1970: 1_800_000_000), endsAt: nil, pausedAt: nil,
      recoveredAfterRestart: false, currentCategory: nil,
      classificationStatus: .unclassified, confidence: .none,
      statusLine: "This block ended early.", result: nil, activeIntervention: nil)
  }

  private func render<V: View>(
    _ view: V, named name: String, outputDirectory: String, size: NSSize
  ) throws {
    let root = AnyView(
      view
        .padding(18)
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(Color.velvtSurface)
        .preferredColorScheme(.dark))
    let hostingView = NSHostingView(rootView: root)
    hostingView.frame = NSRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      XCTFail("Unable to create snapshot bitmap")
      return
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
      XCTFail("Unable to encode snapshot PNG")
      return
    }
    let directory = URL(fileURLWithPath: outputDirectory, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try data.write(to: directory.appendingPathComponent(name), options: .atomic)
  }
}

// MARK: - Work-block minutes: the numbers themselves

/// The formatter, the accepted duration window, and the identity the live
/// countdown is derived from. These are the parts of the minutes surface that
/// are arithmetic rather than layout, so they are asserted rather than looked
/// at.
@MainActor
final class WorkBlockMinutesTests: XCTestCase {

  // MARK: The one duration vocabulary

  /// One rule end to end: the two most significant non-zero units, largest
  /// first, trailing zero unit dropped. The defect this pins is the old
  /// largest-unit-only rule, which published a 119-second longest stretch as
  /// `1m` — a 41% understatement on the card whose job is evidence.
  func testCompactDurationKeepsTheSecondUnitWheneverItIsNonZero() {
    let cases: [(Int, String)] = [
      (0, "0s"),
      (1, "1s"),
      (17, "17s"),
      (59, "59s"),
      (60, "1m"),
      (61, "1m 1s"),
      (90, "1m 30s"),
      (119, "1m 59s"),
      (299, "4m 59s"),
      (300, "5m"),
      (1_487, "24m 47s"),
      (1_500, "25m"),
      (3_599, "59m 59s"),
      (3_600, "1h"),
      (3_601, "1h"),
      (3_660, "1h 1m"),
      (4_215, "1h 10m"),
      (10_740, "2h 59m"),
      (10_800, "3h"),
    ]
    for (seconds, expected) in cases {
      XCTAssertEqual(DurationText.compact(seconds), expected, "compact(\(seconds))")
    }
  }

  func testCompactDurationTreatsNegativeSecondsAsZero() {
    XCTAssertEqual(DurationText.compact(-1), "0s")
    XCTAssertEqual(DurationText.compact(Int.min / 2), "0s")
  }

  /// The clock shape has to match what `Text(timerInterval:)` draws beside it,
  /// because the same value is drawn by the timer while a block runs and by
  /// this function the moment it is paused. The defect this pins is the old
  /// `%d:%02d`, which wrote a paused three-hour block's remaining time as
  /// `179:00` one second after the running timer wrote `2:59:00`.
  func testClockDurationRollsIntoHoursRatherThanCountingMinutesPastSixty() {
    let cases: [(Int, String)] = [
      (0, "0:00"),
      (17, "0:17"),
      (59, "0:59"),
      (60, "1:00"),
      (1_500, "25:00"),
      (3_599, "59:59"),
      (3_600, "1:00:00"),
      (5_400, "1:30:00"),
      (10_740, "2:59:00"),
      (10_800, "3:00:00"),
    ]
    for (seconds, expected) in cases {
      XCTAssertEqual(DurationText.clock(seconds), expected, "clock(\(seconds))")
    }
    XCTAssertEqual(DurationText.clock(-30), "0:00")
  }

  // MARK: The accepted duration window

  /// The stepper's bounds and the schema's `CHECK(planned_duration_seconds
  /// BETWEEN 300 AND 10800)` are the same two numbers, read from one place.
  func testTheChoosableMinutesRangeIsExactlyTheRangeTheServiceAccepts() {
    XCTAssertEqual(WorkBlockDurationLimits.minimumSeconds, 300)
    XCTAssertEqual(WorkBlockDurationLimits.maximumSeconds, 10_800)
    XCTAssertEqual(WorkBlockDurationLimits.minutesRange, 5...180)
    for minutes in stride(
      from: WorkBlockDurationLimits.minutesRange.lowerBound,
      through: WorkBlockDurationLimits.minutesRange.upperBound,
      by: WorkBlockDurationLimits.minuteStep)
    {
      XCTAssertTrue(
        WorkBlockDurationLimits.acceptsMinutes(minutes),
        "the stepper can reach \(minutes) minutes, which the service must accept")
    }
  }

  /// A duration authored elsewhere — a cloud insight's `action_minutes`, a
  /// local signal's — must be refused before it becomes a start command the
  /// service answers with `invalid_work_block_request`.
  func testMinutesOutsideTheAcceptedWindowAreRefusedRatherThanClamped() {
    for minutes in [0, 1, 4, 181, 240, -25] {
      XCTAssertFalse(
        WorkBlockDurationLimits.acceptsMinutes(minutes),
        "\(minutes) minutes is outside 300...10800 seconds")
    }
    XCTAssertTrue(WorkBlockDurationLimits.acceptsMinutes(5))
    XCTAssertTrue(WorkBlockDurationLimits.acceptsMinutes(180))
  }

  // MARK: The live countdown's anchor

  /// The identity the live elapsed count is drawn from, stated as arithmetic
  /// rather than as layout.
  ///
  /// The service publishes work-block state on commands and one deadline and
  /// never on a timer, so `elapsed_duration_seconds` is only true at the
  /// instant it was sent. `ends_at` is not: the service defines it as
  /// `started_at + planned + total_paused`, so `ends_at - planned` is the
  /// instant its own elapsed count starts from, across pauses included. A
  /// count-up anchored there agrees with the service at every later instant;
  /// the old anchor of `now - elapsed_duration_seconds` restarted the count
  /// at whatever the last push said.
  func testElapsedAnchorFromEndsAtStaysCorrectAsTheSnapshotGoesStale() {
    let planned = 1_500
    let totalPaused = 240
    let startedAt = Date(timeIntervalSince1970: 1_800_000_000)
    let endsAt = startedAt.addingTimeInterval(TimeInterval(planned + totalPaused))
    let anchor = endsAt.addingTimeInterval(-TimeInterval(planned))

    // Four instants after the one push, all at or past the point where the
    // block had accumulated its paused time — a block cannot have been
    // paused for longer than it has existed.
    for secondsSinceStart in [totalPaused, totalPaused + 60, totalPaused + 600, planned + 239] {
      let now = startedAt.addingTimeInterval(TimeInterval(secondsSinceStart))
      let serviceElapsed = secondsSinceStart - totalPaused
      let renderedElapsed = max(0, Int(now.timeIntervalSince(anchor)))
      XCTAssertEqual(
        renderedElapsed, serviceElapsed,
        "elapsed at t+\(secondsSinceStart) must equal the service's own count")
      let serviceRemaining = max(0, planned - serviceElapsed)
      let renderedRemaining = max(0, Int(endsAt.timeIntervalSince(now)))
      XCTAssertEqual(
        renderedRemaining, serviceRemaining,
        "remaining at t+\(secondsSinceStart) must equal the service's own count")
      XCTAssertEqual(
        renderedElapsed + renderedRemaining, planned,
        "the two columns must always add up to the planned duration")
    }
  }

  // MARK: The coverage sentence

  /// Good coverage has nothing to qualify, so there is no notice. This is
  /// what keeps the sentence from becoming a standing low-coverage warning.
  func testNoCoverageNoticeWhenCoverageIsGood() {
    XCTAssertNil(
      CoverageNotice.sentence(
        isGood: true, isEmpty: false, coverageRatio: 0.96, switchLabel: "switches"))
  }

  /// The reported screenshot: 12% observed, a 17-second longest stretch, five
  /// switches. The sentence names the fraction and which numbers it applies
  /// to, and it says nothing about whether that is bad.
  func testPartialCoverageStatesTheFractionAndWhichNumbersItCovers() throws {
    let sentence = try XCTUnwrap(
      CoverageNotice.sentence(
        isGood: false, isEmpty: false, coverageRatio: 0.12, switchLabel: "switch-aways"))
    XCTAssertEqual(
      sentence,
      "Observed activity covers 12% of this window. Longest stretch and switch-aways count only that part."
    )
    for judgement in ["low", "poor", "problem", "warning", "only 12"] {
      XCTAssertFalse(
        sentence.lowercased().contains(judgement), "the notice reads a fraction, not a verdict")
    }
  }

  func testEmptyCoverageSaysNothingWasObservedRatherThanZeroPercent() {
    XCTAssertEqual(
      CoverageNotice.sentence(
        isGood: false, isEmpty: true, coverageRatio: 0, switchLabel: "switches"),
      "No activity was observed inside this window.")
  }

  /// Both cards that show elapsed must reach the same sentence for the same
  /// numbers; only the name of each card's own switch metric differs.
  func testBothElapsedSurfacesShareOneSentence() throws {
    let dashboard = try XCTUnwrap(
      CoverageNotice.sentence(
        isGood: false, isEmpty: false, coverageRatio: 0.61, switchLabel: "switches"))
    let result = try XCTUnwrap(
      CoverageNotice.sentence(
        isGood: false, isEmpty: false, coverageRatio: 0.61, switchLabel: "switch-aways"))
    XCTAssertEqual(
      dashboard.replacingOccurrences(of: "switches", with: "switch-aways"), result)
    XCTAssertTrue(dashboard.hasPrefix("Observed activity covers 61% of this window."))
  }
}
