import AppKit
import Combine
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
