import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

/// Renders the 0.1.6 Scope 3 surfaces from synthetic, Rust-shaped payloads
/// for the handoff. Skipped unless `VELVT_INITIATION_SCREENSHOT_DIR` names
/// an output directory, exactly like the 0.1.5 synthetic snapshot tests.
@MainActor
final class InitiationSyntheticSnapshotTests: XCTestCase {
  func testRenderSyntheticInitiationSurfacesWhenRequested() async throws {
    guard let output = ProcessInfo.processInfo.environment["VELVT_INITIATION_SCREENSHOT_DIR"]
    else {
      throw XCTSkip("Set VELVT_INITIATION_SCREENSHOT_DIR to render synthetic handoff screenshots")
    }

    // 1. The invitation card above the idle start form.
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let coordinator = WorkBlockCoordinator(ipcClient: client)
    coordinator.start(messages: messages, connectionStatus: client.connectionStatus)
    messages.send(
      .initiationInvitation(
        InitiationInvitation(
          invitationID: UUID(uuidString: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")!,
          actionID: "soft_start_25",
          body: "You usually focus well around now — want a 25-minute soft start?",
          durationSeconds: 1_500,
          policyVersion: 1
        )))
    messages.send(.workBlockState(idleSnapshot()))
    try await waitUntil { coordinator.invitation != nil && coordinator.snapshot != nil }
    try render(
      WorkBlockView(coordinator: coordinator),
      named: "initiation-invitation-card-synthetic.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 560)
    )

    // 2. The gentle re-entry result after an invited block ended early.
    let restartClient = FakeIPCClient()
    let restartMessages = PassthroughSubject<ServerMessage, Never>()
    let restartCoordinator = WorkBlockCoordinator(ipcClient: restartClient)
    restartCoordinator.start(
      messages: restartMessages, connectionStatus: restartClient.connectionStatus)
    restartMessages.send(.workBlockState(softRestartSnapshot()))
    try await waitUntil { restartCoordinator.snapshot?.result != nil }
    try render(
      WorkBlockView(coordinator: restartCoordinator),
      named: "initiation-soft-restart-result-synthetic.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 520)
    )
  }

  private func idleSnapshot() -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1,
      phase: .idle,
      blockID: nil,
      intention: nil,
      purpose: nil,
      intensity: nil,
      plannedDurationSeconds: 0,
      elapsedDurationSeconds: 0,
      remainingDurationSeconds: 0,
      startedAt: nil,
      endsAt: nil,
      pausedAt: nil,
      recoveredAfterRestart: false,
      currentCategory: nil,
      classificationStatus: .unclassified,
      confidence: .none,
      statusLine: "Choose one bounded block to begin.",
      result: nil,
      activeIntervention: nil
    )
  }

  private func softRestartSnapshot() -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1,
      phase: .abandoned,
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
      intention: nil,
      purpose: nil,
      intensity: .medium,
      plannedDurationSeconds: 1_500,
      elapsedDurationSeconds: 420,
      remainingDurationSeconds: 0,
      startedAt: Date(timeIntervalSince1970: 1_800_000_000),
      endsAt: nil,
      pausedAt: nil,
      recoveredAfterRestart: false,
      currentCategory: nil,
      classificationStatus: .unclassified,
      confidence: .none,
      statusLine: "This block ended early; the result uses only observed activity.",
      result: WorkBlockResult(
        plannedDurationSeconds: 1_500,
        elapsedDurationSeconds: 420,
        longestUninterruptedSeconds: 300,
        switchAwayCount: 1,
        recoveryCount: 1,
        confidence: .low,
        coverage: .partial,
        coverageRatio: 0.62,
        safeEvidenceCategory: "FOCUS_WORK",
        observation:
          "Velvt observed 1 switch-away transitions across 6 minutes of covered activity; switching alone does not show distraction.",
        nextAction: .init(
          actionID: "soft_restart_10",
          label: "Want back in? 10-minute soft restart.",
          durationSeconds: 600
        ),
        dndOutcomes: [],
        reconciliation: nil
      ),
      activeIntervention: nil
    )
  }

  private func render<V: View>(
    _ view: V,
    named name: String,
    outputDirectory: String,
    size: NSSize
  ) throws {
    let root = AnyView(
      view
        .padding(18)
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(Color.velvtSurface)
        .preferredColorScheme(.dark)
    )
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
