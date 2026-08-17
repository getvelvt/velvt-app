import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

/// Renders the 0.1.6 Scope 4 surfaces from synthetic, Rust-shaped payloads
/// for the handoff. Skipped unless `VELVT_SCOPE4_SCREENSHOT_DIR` names an
/// output directory, exactly like the earlier synthetic snapshot tests.
@MainActor
final class Scope4SyntheticSnapshotTests: XCTestCase {
  func testRenderSyntheticScope4SurfacesWhenRequested() async throws {
    guard let output = ProcessInfo.processInfo.environment["VELVT_SCOPE4_SCREENSHOT_DIR"]
    else {
      throw XCTSkip("Set VELVT_SCOPE4_SCREENSHOT_DIR to render synthetic handoff screenshots")
    }

    // 1. The demotion disclosure above the idle start form.
    let demotionClient = FakeIPCClient()
    let demotionMessages = PassthroughSubject<ServerMessage, Never>()
    let demotionCoordinator = WorkBlockCoordinator(ipcClient: demotionClient)
    demotionCoordinator.start(
      messages: demotionMessages, connectionStatus: demotionClient.connectionStatus)
    demotionMessages.send(
      .demotionState(
        DemotionState(
          state: .demoted,
          wrongCount: 4,
          deliveredCount: 16,
          thresholdPercent: 15,
          minimumSample: 10,
          windowDays: 14,
          thresholdPolicyVersion: 1,
          repromotionPolicyVersion: 1,
          demotedAt: Date(timeIntervalSince1970: 1_800_000_000),
          disclosure:
            "Velvt is getting these nudges wrong too often, so it has gone quiet: no nudges will be sent for now, and you can resume them at any time."
        )))
    demotionMessages.send(.workBlockState(idleSnapshot()))
    try await waitUntil {
      demotionCoordinator.demotionState != nil && demotionCoordinator.snapshot != nil
    }
    try render(
      WorkBlockView(coordinator: demotionCoordinator),
      named: "scope4-demotion-disclosure-synthetic.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 620)
    )

    // 2. The weekly receipts digest above the idle start form.
    let digestClient = FakeIPCClient()
    let digestMessages = PassthroughSubject<ServerMessage, Never>()
    let digestCoordinator = WorkBlockCoordinator(ipcClient: digestClient)
    digestCoordinator.start(
      messages: digestMessages, connectionStatus: digestClient.connectionStatus)
    digestMessages.send(
      .weeklyDigest(
        WeeklyDigest(
          weekStartLocalDate: "2026-07-27",
          blocksDeclared: 5,
          blocksCompleted: 3,
          recoveries: 4,
          wrongInterventions: 1,
          invitationsAccepted: 2,
          withheld: 1,
          headline: "You returned 4 times and completed 3 of 5 blocks this week.",
          digestVersion: 1
        )))
    digestMessages.send(.workBlockState(idleSnapshot()))
    try await waitUntil {
      digestCoordinator.weeklyDigest != nil && digestCoordinator.snapshot != nil
    }
    try render(
      WorkBlockView(coordinator: digestCoordinator),
      named: "scope4-weekly-receipts-synthetic.png",
      outputDirectory: output,
      size: NSSize(width: 420, height: 680)
    )

    // 3. The intervention card with the explanation sentence shown.
    let explainClient = FakeIPCClient()
    let explainMessages = PassthroughSubject<ServerMessage, Never>()
    let explainCoordinator = WorkBlockCoordinator(ipcClient: explainClient)
    explainCoordinator.start(
      messages: explainMessages, connectionStatus: explainClient.connectionStatus)
    let blockID = UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!
    explainMessages.send(.workBlockState(interventionSnapshot(blockID: blockID)))
    try await waitUntil { explainCoordinator.snapshot?.activeIntervention != nil }
    explainMessages.send(
      .interventionExplanation(
        InterventionExplanation(
          blockID: blockID,
          sentence:
            "Velvt offered this nudge because it observed 4 switches away from deep work in the 10 minutes before the offer."
        )))
    try await waitUntil { explainCoordinator.explanation != nil }
    try render(
      WorkBlockView(coordinator: explainCoordinator),
      named: "scope4-explain-this-nudge-synthetic.png",
      outputDirectory: output,
      size: NSSize(width: 460, height: 460)
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

  private func interventionSnapshot(blockID: UUID) -> WorkBlockSnapshot {
    WorkBlockSnapshot(
      stateVersion: 1,
      phase: .active,
      blockID: blockID,
      intention: "Draft the report",
      purpose: .deepWork,
      intensity: .medium,
      plannedDurationSeconds: 1_500,
      elapsedDurationSeconds: 600,
      remainingDurationSeconds: 900,
      startedAt: Date(timeIntervalSince1970: 1_800_000_000),
      endsAt: Date(timeIntervalSince1970: 1_800_001_500),
      pausedAt: nil,
      recoveredAfterRestart: false,
      currentCategory: "COMMUNICATION",
      classificationStatus: .classified,
      confidence: .high,
      statusLine: "Current safe category: Communication.",
      result: nil,
      activeIntervention: ActiveIntervention(
        actionID: "protect_next_10",
        title: "Your work block is running",
        body:
          "Velvt observed 4 switches away from deep work in the last 10 minutes. "
          + "Protect the next 10 minutes for the work you chose.",
        anchorCategory: "DEEP_WORK",
        switchCount: 4,
        windowSeconds: 600,
        offeredAt: Date(timeIntervalSince1970: 1_800_000_600)
      )
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
