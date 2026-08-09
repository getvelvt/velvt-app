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
    try await waitUntil { client.sentMessages.count == 4 }

    XCTAssertEqual(
      client.sentMessages,
      [
        .workBlockLifecycle(.init(event: .sleep)),
        .workBlockLifecycle(.init(event: .wake)),
        .workBlockLifecycle(.init(event: .clockChanged)),
        .workBlockLifecycle(.init(event: .timeZoneChanged)),
      ])
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
        "title": "Your work block is still running",
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
      title: "Your work block is still running",
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
