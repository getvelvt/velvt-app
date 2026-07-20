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

  private func activeSnapshot() -> WorkBlockSnapshot {
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
      result: nil
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
