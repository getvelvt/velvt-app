import AppKit
import Combine
import XCTest

@testable import VelvtMac

// MARK: - Menu bar navigation tests

@MainActor
final class MenuBarNavigationTests: XCTestCase {

  func testRestoredWorkspaceNavigationKeepsThePreviousOrderAndTitles() {
    XCTAssertEqual(
      MenuBarWorkspaceTab.allCases.map(\.title),
      ["Now", "Patterns", "Settings"]
    )
  }

  func testRestoredWorkspaceNavigatorResetsToToday() {
    var navigator = MenuBarPopoverNavigator()

    navigator.selectWorkspaceTab(.history)
    XCTAssertEqual(navigator.selectedWorkspaceTab, .history)

    navigator.resetForPopoverOpening()
    XCTAssertEqual(navigator.selectedWorkspaceTab, .workBlock)
  }

    func testConnectionPresentationUsesRequestedLabelsAndColors() {
    XCTAssertEqual(
      PopoverConnectionPresentation(status: .connected).label, "Local service connected")
        XCTAssertEqual(PopoverConnectionPresentation(status: .connecting).label, "Connecting")
        XCTAssertEqual(PopoverConnectionPresentation(status: .disconnected).label, "Disconnected")
    }

    func testServiceConnectionStatusModelReflectsSocketUpdates() async {
        let client = FakeIPCClient()
        let model = ServiceConnectionStatusModel(connectionStatus: client.connectionStatus)

        XCTAssertEqual(model.status, .disconnected)

        client.setConnectionStatus(.reconnecting(attempt: 2, nextRetryIn: 1))
        await Task.yield()

        XCTAssertEqual(model.status, .reconnecting(attempt: 2, nextRetryIn: 1))
    }

    func testCollectionSettingsDefaultOfflineCollectionEnabled() {
        let defaults = UserDefaults(suiteName: "CollectionSettingsModel.default.\(UUID().uuidString)")!

        let model = CollectionSettingsModel(defaults: defaults)

        XCTAssertTrue(model.offlineEventCollectionEnabled)
    }

    func testCollectionSettingsPersistsOfflineCollectionPreference() {
        let defaults = UserDefaults(suiteName: "CollectionSettingsModel.persist.\(UUID().uuidString)")!
        let first = CollectionSettingsModel(defaults: defaults)

        first.offlineEventCollectionEnabled = false
        let second = CollectionSettingsModel(defaults: defaults)

        XCTAssertFalse(second.offlineEventCollectionEnabled)
    }

    func testServiceAlertModelSurfacesMalformedMessage() async {
        let messages = PassthroughSubject<ServerMessage, Never>()
        let model = ServiceAlertModel(messages: messages)

        messages.send(.malformedMessage(MalformedMessage(code: .invalidMessage)))
        await Task.yield()

        XCTAssertEqual(model.alert?.severity, .warning)
        XCTAssertEqual(model.alert?.title, "Message rejected")
    }

    func testServiceAlertModelSurfacesPrivacyViolationAlert() async {
        let messages = PassthroughSubject<ServerMessage, Never>()
        let model = ServiceAlertModel(messages: messages)

    messages.send(
      .privacyViolationAlert(
        PrivacyViolationAlert(
                    code: "raw_content_detected",
                    message: "Sensitive content was blocked."
                )))
        await Task.yield()

        XCTAssertEqual(model.alert?.severity, .error)
        XCTAssertEqual(model.alert?.title, "Privacy guard blocked data")
        XCTAssertEqual(model.alert?.message, "Sensitive content was blocked.")
    }

    func testServiceAlertModelSurfacesShuttingDown() async {
        let messages = PassthroughSubject<ServerMessage, Never>()
        let model = ServiceAlertModel(messages: messages)

        messages.send(.shuttingDown(ShuttingDown(reason: "sigterm")))
        await Task.yield()

        XCTAssertEqual(model.alert?.severity, .warning)
        XCTAssertEqual(model.alert?.title, "Service restarting")
    }

    func testServiceAlertModelSurfacesGenericErrorResponseAndDismisses() async {
        let messages = PassthroughSubject<ServerMessage, Never>()
        let model = ServiceAlertModel(messages: messages)

    messages.send(
      .errorResponse(
        ErrorResponse(
                    code: "unexpected",
                    message: "Something went wrong.",
                    relatedEventID: nil
                )))
        await Task.yield()

        XCTAssertEqual(model.alert?.severity, .error)
        XCTAssertEqual(model.alert?.message, "Something went wrong.")

        model.dismiss()

        XCTAssertNil(model.alert)
    }

    func testReducedMotionDisablesPopoverRouteAnimation() {
        XCTAssertFalse(MenuBarMotionPolicy.shouldAnimate(reduceMotion: true))
        XCTAssertTrue(MenuBarMotionPolicy.shouldAnimate(reduceMotion: false))
    }

    func testOnboardingWindowClampsToTheVisibleScreen() {
        XCTAssertEqual(
            OnboardingWindowLayout.contentSize(
                for: CGRect(x: 0, y: 0, width: 640, height: 480)
            ),
            CGSize(width: 592, height: 432)
        )
        XCTAssertEqual(
            OnboardingWindowLayout.contentSize(for: nil),
            CGSize(width: 720, height: 520)
        )
    }

    /// Both numbers are measured, not preferences. 600pt is the narrowest
    /// width at which the Now tab's content stops rewrapping (325pt tall at
    /// 660, 620 and 600; 354pt at 560; 383pt at 500), and 480pt clears the
    /// tallest realistic Now tab (358pt with an active block) inside the
    /// header and bottom-bar chrome.
    func testPopoverIsSizedToTheWidestContentAndNotWider() {
        XCTAssertEqual(MenuBarPopoverLayout.preferredContentSize, CGSize(width: 600, height: 480))
    }

    /// The guided-tour bar measures 86pt plus a 1pt divider at every width the
    /// popover can take, so the walkthrough must grow the popover by exactly
    /// that. The previous 450 -> 600 step added 150pt, 63pt more than the bar
    /// needs, and the surplus went into the content pane: every row moved when
    /// the tour opened and moved back when it closed.
    func testWalkthroughGrowsByExactlyTheTourBarSoContentDoesNotMove() {
        XCTAssertEqual(
            MenuBarPopoverLayout.walkthroughContentSize.height
                - MenuBarPopoverLayout.preferredContentSize.height,
            MenuBarPopoverLayout.guidedTourBarHeight
        )
        XCTAssertEqual(
            MenuBarPopoverLayout.walkthroughContentSize.width,
            MenuBarPopoverLayout.preferredContentSize.width
        )
    }

    /// The clamp used to be `max(1, visibleFrame - inset)`. A 1pt popover is
    /// not a graceful degradation; there is no way back out of it.
    func testSmallScreenClampNeverProducesAnUnusablePopover() {
        for height in stride(from: CGFloat(1), through: 400, by: 7) {
            let frame = CGRect(x: 0, y: 0, width: 320, height: height)
            let size = MenuBarPopoverLayout.contentSize(for: frame)

            XCTAssertLessThanOrEqual(size.height, frame.height)
            XCTAssertLessThanOrEqual(size.width, frame.width)
            XCTAssertGreaterThanOrEqual(
                size.height,
                min(MenuBarPopoverLayout.minimumContentSize.height, frame.height),
                "A \(height)pt screen produced a \(size.height)pt popover"
            )
        }
    }

    func testSettingsRetainsEveryDestination() {
        #if DEBUG
      XCTAssertEqual(
        SettingsSubmenu.allCases.map(\.title),
        [
            "App Info", "Teach Velvt Your Apps", "Collection Settings", "Onboarding & Tour",
            "Debug/Testing",
        ])
        #else
      XCTAssertEqual(
        SettingsSubmenu.allCases.map(\.title),
        [
            "App Info", "Teach Velvt Your Apps", "Collection Settings", "Onboarding & Tour",
        ])
        #endif
    }

    /// The workbench is the only destination with a text field, a picker and a
    /// button row on one line. Measured at 300pt the inline correction editor
    /// needs 97pt against 84pt at pane width, because the controls wrap.
    func testTheCorrectionWorkbenchGetsMoreWidthThanTheOtherDestinations() {
        XCTAssertEqual(SettingsSubmenu.teachApps.preferredWidth, 380)
        for submenu in SettingsSubmenu.allCases where submenu != .teachApps {
            XCTAssertEqual(submenu.preferredWidth, 300)
        }
        XCTAssertLessThan(
            MenuBarPopoverLayout.preferredContentSize.width
                + SettingsSubmenu.teachApps.preferredWidth,
            1_280,
            "The popover and its widest submenu must fit a 1280pt laptop screen"
        )
    }

    func testGuidedTourCoversOnlyLiveDestinationsAndMovesDeterministically() {
        let tour = GuidedTourModel()

        tour.start()
        XCTAssertTrue(tour.isPresented)
        XCTAssertEqual(tour.step, .today)
        XCTAssertEqual(
            GuidedTourStep.allCases,
            [
                .today, .earlySignal, .focusFragmentation, .dailyActivity, .statusAndRecovery,
                .settings,
            ])

        tour.advance()
        XCTAssertEqual(tour.step, .earlySignal)
        tour.goBack()
        XCTAssertEqual(tour.step, .today)
        tour.dismiss()
        XCTAssertFalse(tour.isPresented)
    }

    func testGuidedTourDoneDismissesFromSettings() {
        let tour = GuidedTourModel()
        tour.start()
        for _ in 1..<GuidedTourStep.allCases.count { tour.advance() }

        XCTAssertEqual(tour.step, .settings)
        XCTAssertTrue(tour.isLastStep)

        tour.advance()
        XCTAssertFalse(tour.isPresented)
    }

    func testEarlyLocalSignalAppearsOnlyWithSufficientEvidence() {
        XCTAssertEqual(
            TodayObservationResolver.resolve(
                cloudAvailable: false,
                cloudSourceDate: "",
                currentLocalDate: "2026-07-18",
                earlySignalStatus: .insufficientEvidence
            ),
            .progress
        )
        XCTAssertEqual(
            TodayObservationResolver.resolve(
                cloudAvailable: false,
                cloudSourceDate: "",
                currentLocalDate: "2026-07-18",
                earlySignalStatus: .ready
            ),
            .earlyLocal
        )
    }

    func testCurrentDayCloudInsightReplacesEarlySignalWithoutLoadingState() {
        XCTAssertEqual(
            TodayObservationResolver.resolve(
                cloudAvailable: true,
                cloudSourceDate: "2026-07-18",
                currentLocalDate: "2026-07-18",
                earlySignalStatus: .ready
            ),
            .cloud
        )
    }

    func testOlderCloudInsightCannotReplaceTodayEarlySignal() {
        XCTAssertEqual(
            TodayObservationResolver.resolve(
                cloudAvailable: true,
                cloudSourceDate: "2026-07-17",
                currentLocalDate: "2026-07-18",
                earlySignalStatus: .ready
            ),
            .earlyLocal
        )
    }

    func testTemporaryReconnectKeepsConfirmedConnectedPresentationDuringGrace() async {
        let client = FakeIPCClient()
        let scheduler = ManualConnectionGraceScheduler()
        let notifications = NotificationCenter()
        let model = ServiceConnectionStatusModel(
            connectionStatus: client.connectionStatus,
            scheduler: scheduler,
            graceInterval: 4,
            workspaceNotifications: notifications
        )

        client.setConnectionStatus(.connected)
        await Task.yield()
        client.setConnectionStatus(.reconnecting(attempt: 1, nextRetryIn: 1))
        await Task.yield()

        XCTAssertEqual(model.phase, .connected)

        scheduler.fireLatest()
        XCTAssertEqual(model.phase, .unavailable)
    }

    func testReconnectHandshakeBeforeGraceExpiresNeverShowsFailure() async {
        let client = FakeIPCClient()
        let scheduler = ManualConnectionGraceScheduler()
        let model = ServiceConnectionStatusModel(
            connectionStatus: client.connectionStatus,
            scheduler: scheduler,
            workspaceNotifications: NotificationCenter()
        )

        client.setConnectionStatus(.connected)
        await Task.yield()
        client.setConnectionStatus(.disconnected)
        await Task.yield()
        client.setConnectionStatus(.connected)
        await Task.yield()
        scheduler.fireAll()

        XCTAssertEqual(model.phase, .connected)
    }

    func testSleepWakeUsesRecoverableWakingStateUntilHandshake() async {
        let client = FakeIPCClient()
        let scheduler = ManualConnectionGraceScheduler()
        let notifications = NotificationCenter()
        let model = ServiceConnectionStatusModel(
            connectionStatus: client.connectionStatus,
            scheduler: scheduler,
            workspaceNotifications: notifications
        )
        client.setConnectionStatus(.connected)
        await Task.yield()

        notifications.post(name: NSWorkspace.willSleepNotification, object: nil)
        notifications.post(name: NSWorkspace.didWakeNotification, object: nil)
        XCTAssertEqual(model.phase, .waking)

        client.setConnectionStatus(.connected)
        await Task.yield()
        XCTAssertEqual(model.phase, .connected)
    }

    func testAuthenticationPresentationShowsLoggedOutState() {
        let presentation = AuthenticationStatusPresentation(
            accountState: .loggedOut,
            email: "user@example.com"
        )

        XCTAssertEqual(presentation.text, "Not Authenticated")
        XCTAssertEqual(presentation.indicatorColor, .red)
    }

    func testAuthenticationPresentationShowsReauthenticationRecovery() {
        let presentation = AuthenticationStatusPresentation(
            accountState: .loggedOut,
            email: nil,
            requiresReauthentication: true
        )

        XCTAssertEqual(presentation.text, "Sign in required")
        XCTAssertEqual(presentation.indicatorColor, .red)
    }

    func testAuthenticationPresentationShowsEmailForLoggedInState() {
        let presentation = AuthenticationStatusPresentation(
            accountState: .loggedIn(userId: "u1"),
            email: "user@example.com"
        )

        XCTAssertEqual(presentation.text, "user@example.com")
        XCTAssertEqual(presentation.indicatorColor, .green)
    }

    func testShortSettingsSubmenuIsCenteredOnSourceRow() {
        let sourceFrame = CGRect(x: 100, y: 400, width: 300, height: 44)
        let submenuSize = CGSize(width: 280, height: 88)

        let frame = SubmenuPopoverPlacement.frame(
            sourceFrameInScreen: sourceFrame,
            submenuContentSize: submenuSize
        )

        XCTAssertEqual(frame.midY, sourceFrame.midY, accuracy: 0.001)
        XCTAssertEqual(frame.minX, sourceFrame.maxX, accuracy: 0.001)
    }

    func testTallSettingsSubmenuDoesNotMoveAboveSourceMenuTop() {
        let sourceMenuFrame = CGRect(x: 100, y: 300, width: 300, height: 220)
        let sourceFrame = CGRect(x: 100, y: 390, width: 300, height: 44)
        let submenuSize = CGSize(width: 280, height: 260)

        let frame = SubmenuPopoverPlacement.frame(
            sourceFrameInScreen: sourceFrame,
            submenuContentSize: submenuSize,
            sourceMenuFrameInScreen: sourceMenuFrame
        )

        XCTAssertEqual(frame.maxY, sourceMenuFrame.maxY, accuracy: 0.001)
        XCTAssertLessThan(frame.midY, sourceFrame.midY)
    }

    func testShortSettingsSubmenuIgnoresStaleTallWindowHeight() {
        let sourceMenuFrame = CGRect(x: 100, y: 300, width: 300, height: 220)
        let sourceFrame = CGRect(x: 100, y: 390, width: 300, height: 44)
        let staleTallWindowFrame = CGRect(x: 410, y: 260, width: 280, height: 260)
        let debugSubmenuSize = CGSize(width: 280, height: 88)

        let frame = SubmenuPopoverPlacement.frame(
            sourceFrameInScreen: sourceFrame,
            submenuContentSize: debugSubmenuSize,
            sourceMenuFrameInScreen: sourceMenuFrame,
            currentWindowFrame: staleTallWindowFrame
        )

        XCTAssertEqual(frame.midY, sourceFrame.midY, accuracy: 0.001)
        XCTAssertEqual(frame.height, debugSubmenuSize.height, accuracy: 0.001)
    }

}
@MainActor
private final class ManualConnectionGraceScheduler: ConnectionGraceScheduling {
    private final class Entry {
        var isCancelled = false
        let action: @MainActor () -> Void

        init(action: @escaping @MainActor () -> Void) {
            self.action = action
        }
    }

    private var entries: [Entry] = []

    func schedule(
        after interval: TimeInterval,
        action: @escaping @MainActor () -> Void
    ) -> AnyCancellable {
        let entry = Entry(action: action)
        entries.append(entry)
        return AnyCancellable { entry.isCancelled = true }
    }

    func fireLatest() {
        guard let entry = entries.last, !entry.isCancelled else { return }
        entry.action()
    }

    func fireAll() {
        entries.filter { !$0.isCancelled }.forEach { $0.action() }
    }
}

// MARK: - DeviceRevoked integration tests

@MainActor
final class DeviceRevokedUITests: XCTestCase {

    func testDeviceRevokedPushSetsIsDeviceRevokedFlag() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)

        XCTAssertFalse(manager.isDeviceRevoked)

        let flagSet = expectation(description: "isDeviceRevoked set to true")
        var cancellable: AnyCancellable?
        cancellable = manager.$isDeviceRevoked.dropFirst().sink { revoked in
      if revoked {
        flagSet.fulfill()
        cancellable?.cancel()
      }
        }

        client.inject(.deviceRevoked(DeviceRevoked(message: "Your device was revoked")))

        await fulfillment(of: [flagSet], timeout: 1)
        XCTAssertTrue(manager.isDeviceRevoked)
        XCTAssertEqual(manager.accountState, .loggedOut)
    }

    func testClearingFlagAllowsReauth() async throws {
        let client = FakeIPCClient()
        let keychain = FakeKeychain()
        try keychain.store(token: "u1", for: .userId)
        let manager = AccountStateManager(keychain: keychain)
        manager.startListening(to: client)

        let flagSet = expectation(description: "isDeviceRevoked set")
        var cancellable: AnyCancellable?
    cancellable = manager.$isDeviceRevoked.dropFirst().sink {
      if $0 {
        flagSet.fulfill()
        cancellable?.cancel()
      }
    }
        client.inject(.deviceRevoked(DeviceRevoked(message: "revoked")))
        await fulfillment(of: [flagSet], timeout: 1)

        manager.clearDeviceRevokedFlag()
        XCTAssertFalse(manager.isDeviceRevoked, "Flag must be cleared so re-auth screen can appear")
    }
}

// MARK: - Correction workbench

/// The constraint these tests exist for: a surface may not present itself as a
/// live readout of what Velvt thinks unless a correction visibly changes what
/// it shows.
///
/// Before this pass it did not. Two independent mechanisms, both reproduced
/// below against the fixtures the service actually emits:
///
///  1. The selection was keyed on `LocalDailyActivitySegment.id`, which the
///     service builds as `{date}-segment-{index}-{category}`. A correction
///     changes the category and re-sorts the day by duration, so both halves
///     of that id move and the corrected row is simply gone from the next
///     snapshot. The panel holding the correction controls vanished at the
///     moment the user used it.
///  2. The evidence sentence was a `String` captured when the row was clicked,
///     so it went on asserting the classification the user had just replaced
///     for as long as the popover stayed open.
@MainActor
final class LocalActivityCorrectionListTests: XCTestCase {

  private let stableID = "abs_editor_local"

  /// What the service sends before the correction: the activity is
  /// unclassified, so it sorts last and its id carries `unclassified`.
  private func beforeCorrection() -> LocalDashboardSnapshot {
    snapshot(segments: [
      segment(id: "2026-08-16-segment-0-focus_work", stableID: "abs_other", label: "Xcode",
        category: "FOCUS_WORK", seconds: 5_400, percentage: 75, confidence: .high),
      segment(id: "2026-08-16-segment-1-unclassified", stableID: stableID, label: "Unclassified",
        suggestedName: "Sketch Companion", category: "UNCLASSIFIED", seconds: 1_800,
        percentage: 25, confidence: .none),
    ])
  }

  /// What the service sends after it: the same activity, now classified, so it
  /// re-buckets under a new category and the day is re-sorted. Same
  /// `stableID`, different `id`, different index.
  private func afterCorrection() -> LocalDashboardSnapshot {
    snapshot(segments: [
      segment(id: "2026-08-16-segment-0-focus_work", stableID: "abs_other", label: "Xcode",
        category: "FOCUS_WORK", seconds: 5_400, percentage: 75, confidence: .high),
      segment(id: "2026-08-16-segment-1-creative", stableID: stableID, label: "Sketch Companion",
        suggestedName: "Sketch Companion", aliasConfirmed: true, category: "CREATIVE",
        seconds: 1_800, percentage: 25, confidence: .high),
    ])
  }

  func testTheSegmentIdentityTheServiceSendsDoesNotSurviveACorrection() {
    let before = try! XCTUnwrap(
      LocalActivityCorrectionList.correctableDay(in: beforeCorrection())
    ).segments.first(where: { $0.stableID == stableID })!
    let after = try! XCTUnwrap(
      LocalActivityCorrectionList.correctableDay(in: afterCorrection())
    ).segments.first(where: { $0.stableID == stableID })!

    XCTAssertNotEqual(
      before.id, after.id,
      "The service's segment id embeds the category, so a correction changes it")
    XCTAssertEqual(before.stableID, after.stableID)
  }

  func testASelectedActivityStaysSelectedThroughItsOwnCorrection() {
    let selected = LocalActivityCorrectionList.selectedSegment(
      stableID: stableID, in: beforeCorrection())
    XCTAssertEqual(selected?.label, "Unclassified")

    let stillSelected = LocalActivityCorrectionList.selectedSegment(
      stableID: stableID, in: afterCorrection())
    XCTAssertNotNil(
      stillSelected,
      "The correction panel must not disappear the moment the correction lands")
    XCTAssertEqual(stillSelected?.label, "Sketch Companion")
    XCTAssertEqual(stillSelected?.category, "CREATIVE")
  }

  func testTheEvidenceLineIsRereadFromTheSnapshotRatherThanCaptured() {
    let before = LocalActivityCorrectionList.selectedSegment(
      stableID: stableID, in: beforeCorrection())!
    let after = LocalActivityCorrectionList.selectedSegment(
      stableID: stableID, in: afterCorrection())!

    XCTAssertTrue(LocalActivityCorrectionList.detail(for: before).hasPrefix("Unclassified"))
    XCTAssertTrue(LocalActivityCorrectionList.detail(for: after).hasPrefix("Sketch Companion"))
    XCTAssertNotEqual(
      LocalActivityCorrectionList.detail(for: before),
      LocalActivityCorrectionList.detail(for: after))
  }

  /// 05 § 2: "A percentage of your week is a report; a duration next to a
  /// correctable label is a workbench."
  func testTheWorkbenchStatesADurationAndNeverAPercentage() {
    let segment = LocalActivityCorrectionList.selectedSegment(
      stableID: stableID, in: beforeCorrection())!
    let detail = LocalActivityCorrectionList.detail(for: segment)

    XCTAssertTrue(detail.contains("30m"), detail)
    XCTAssertFalse(detail.contains("%"), detail)
    XCTAssertFalse(detail.lowercased().contains("7 day"), detail)
    XCTAssertFalse(detail.lowercased().contains("week"), detail)
  }

  func testDurationsReadAsMinutesAndHours() {
    XCTAssertEqual(LocalActivityCorrectionList.plainDuration(0), "0m")
    XCTAssertEqual(LocalActivityCorrectionList.plainDuration(1_800), "30m")
    XCTAssertEqual(LocalActivityCorrectionList.plainDuration(3_600), "1h 0m")
    XCTAssertEqual(LocalActivityCorrectionList.plainDuration(5_400), "1h 30m")
  }

  /// Picking the most recent day that has activity, rather than today, keeps
  /// the workbench usable first thing in the morning. It is a filter over the
  /// delivered payload, not a computation on it.
  func testTheWorkbenchFallsBackToTheMostRecentDayThatHasActivity() {
    let populated = beforeCorrection()
    var days = populated.dailyActivity
    days.append(
      LocalDailyActivityDay(
        id: "2026-08-17", date: "2026-08-17", state: .noData, activeSeconds: 0,
        coverage: .noData, segments: []))
    let withEmptyToday = LocalDashboardSnapshot(
      generatedAt: populated.generatedAt, windowStart: populated.windowStart,
      windowEnd: populated.windowEnd, switchCount: populated.switchCount,
      switchesPerHour: populated.switchesPerHour, coverage: populated.coverage,
      earlySignal: populated.earlySignal, segments: populated.segments,
      focusFragmentation: nil, dailyActivity: days)

    XCTAssertEqual(
      LocalActivityCorrectionList.correctableDay(in: withEmptyToday)?.date, "2026-08-16")
    XCTAssertNil(LocalActivityCorrectionList.correctableDay(in: nil))
  }

  func testAnUnknownCategoryFallsBackToAnEditableOne() {
    XCTAssertEqual(LocalActivityCorrectionList.correctionCategory("CREATIVE"), "UNLOGGED")
    XCTAssertEqual(LocalActivityCorrectionList.correctionCategory("FOCUS_WORK"), "FOCUS_WORK")
  }

  // MARK: Fixtures

  private func segment(
    id: String, stableID: String, label: String, suggestedName: String? = nil,
    aliasConfirmed: Bool = false, category: String, seconds: Int, percentage: Int,
    confidence: ClassificationConfidence
  ) -> LocalDailyActivitySegment {
    LocalDailyActivitySegment(
      id: id, label: label, representativeEventID: UUID(), stableID: stableID,
      suggestedName: suggestedName, aliasConfirmed: aliasConfirmed, category: category,
      durationSeconds: seconds, percentage: percentage, confidence: confidence,
      explanation: nil)
  }

  private func snapshot(segments: [LocalDailyActivitySegment]) -> LocalDashboardSnapshot {
    let base = Date(timeIntervalSince1970: 1_800_000_000)
    return LocalDashboardSnapshot(
      generatedAt: base, windowStart: base, windowEnd: base.addingTimeInterval(3_600),
      switchCount: 3, switchesPerHour: 3, coverage: .good,
      earlySignal: LocalEarlySignal(
        status: .ready, observedFrom: base, observedThrough: base.addingTimeInterval(3_600),
        observedSeconds: 3_600, requiredSeconds: 0, evidenceEventCount: 9, focusedSeconds: 2_100,
        meaningfulSwitchCount: 3, longestUninterruptedSeconds: 1_080,
        observation: "One change of direction in the last 60 minutes.",
        suggestedAction: "Want 25 minutes on it, uninterrupted?", actionMinutes: 25),
      segments: [], focusFragmentation: nil,
      dailyActivity: [
        LocalDailyActivityDay(
          id: "2026-08-16", date: "2026-08-16", state: .ready, activeSeconds: 7_200,
          coverage: .good, segments: segments)
      ])
  }
}

// MARK: - Correction workbench wiring

import SwiftUI

@MainActor
final class CorrectionWorkbenchViewTests: XCTestCase {

  /// The framing sentence is the whole point of this surface: not a report on
  /// the user, a place where the user corrects the software.
  func testTheWorkbenchExplainsItselfAsSomethingYouFixNotSomethingYouRead() {
    let copy = CorrectionWorkbenchView.explanationCopy

    XCTAssertTrue(copy.contains("Velvt gets these wrong sometimes"), copy)
    XCTAssertTrue(copy.contains("this Mac only"), copy)
    XCTAssertFalse(copy.lowercased().contains("7 day"), copy)
    XCTAssertFalse(copy.lowercased().contains("percentage"), copy)
  }

  /// The correction and the dashboard request travel over one actor-isolated
  /// socket client on two unstructured tasks, so refreshing the rows straight
  /// after the click can read the dashboard back before the correction has
  /// been applied. Refreshing off the service's acknowledgement instead cannot
  /// race it: the acknowledgement is written by the same handler that applied
  /// the correction.
  func testCorrectedRowsRefreshOffTheServiceAcknowledgementNotTheClick() async throws {
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let menuStatus = MenuStatusViewModel(ipcClient: client, messages: messages)
    let dashboard = LocalDashboardCoordinator(ipcClient: client)
    dashboard.start(messages: messages, connectionStatus: client.connectionStatus)

    let host = NSHostingView(
      rootView: CorrectionWorkbenchView(
        menuStatus: menuStatus,
        localDashboard: dashboard,
        title: "Teach Velvt Your Apps"
      )
      .frame(width: 380, height: 560)
    )
    host.frame = NSRect(x: 0, y: 0, width: 380, height: 560)
    host.layoutSubtreeIfNeeded()
    try await waitUntil("the workbench asks for the rows when it opens") {
      self.dashboardRequestCount(client) > 0
    }

    let before = dashboardRequestCount(client)
    messages.send(.menuStatus(acknowledgingStatus()))

    try await waitUntil("a correction the service confirmed redraws the rows it changed") {
      self.dashboardRequestCount(client) > before
    }
  }

  private func waitUntil(
    _ description: String,
    timeout: TimeInterval = 3,
    _ condition: @escaping () -> Bool
  ) async throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
      if condition() { return }
      try await Task.sleep(nanoseconds: 20_000_000)
    }
    XCTFail("Timed out waiting until \(description)")
  }

  func testAWorkbenchWithNoServiceBehindItSaysSoRatherThanShowingAnEmptyList() {
    let host = NSHostingView(
      rootView: CorrectionWorkbenchUnavailableView(title: "Teach Velvt Your Apps")
        .frame(width: 380)
    )
    host.frame = NSRect(x: 0, y: 0, width: 380, height: 200)
    host.layoutSubtreeIfNeeded()

    XCTAssertGreaterThan(host.fittingSize.height, 0)
  }

  private func dashboardRequestCount(_ client: FakeIPCClient) -> Int {
    client.sentMessages.filter {
      if case .requestLocalDashboard = $0 { return true }
      return false
    }.count
  }

  private func acknowledgingStatus() -> MenuStatus {
    MenuStatus(
      deviceID: "device",
      cloudReady: true,
      uploadStatus: "idle",
      lastUploadErrorCode: nil,
      nextUploadAttemptAt: nil,
      lastSuccessfulSyncAt: nil,
      pendingUploadBatchCount: 0,
      failedUploadBatchCount: 0,
      rejectedUploadBatchCount: 0,
      queuedEventCount: 0,
      queuedEvents: [],
      correctionHistory: [],
      correctionAcknowledgment: "Sketch Companion is creative work from now on."
    )
  }
}

/// Renders the correction workbench at the width the settings submenu gives
/// it, so the next person can see the layout instead of reasoning about it.
/// Skipped unless `VELVT_WORKBENCH_SCREENSHOT_DIR` names an output directory,
/// following the existing synthetic snapshot tests. No app launch, no
/// Accessibility permission.
@MainActor
final class CorrectionWorkbenchSnapshotTests: XCTestCase {
  func testRenderWorkbenchWhenRequested() async throws {
    guard
      let output = ProcessInfo.processInfo.environment["VELVT_WORKBENCH_SCREENSHOT_DIR"]
    else {
      throw XCTSkip("Set VELVT_WORKBENCH_SCREENSHOT_DIR to render the workbench")
    }

    let width = SettingsSubmenu.teachApps.preferredWidth
    let height = SettingsSubmenu.teachApps.preferredHeight
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    let menuStatus = MenuStatusViewModel(ipcClient: client, messages: messages)
    let dashboard = LocalDashboardCoordinator(ipcClient: client)
    dashboard.start(messages: messages, connectionStatus: client.connectionStatus)
    messages.send(.localDashboard(Self.syntheticSnapshot()))
    for _ in 0..<100 where dashboard.snapshot == nil {
      try await Task.sleep(nanoseconds: 20_000_000)
    }

    let view = ScrollView {
      CorrectionWorkbenchView(
        menuStatus: menuStatus, localDashboard: dashboard, title: "Teach Velvt Your Apps")
    }
    .frame(width: width, height: height, alignment: .top)
    .background(Color.velvtSurface)
    .preferredColorScheme(.dark)

    let host = NSHostingView(rootView: AnyView(view))
    host.frame = NSRect(x: 0, y: 0, width: width, height: height)
    host.layoutSubtreeIfNeeded()
    guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) else {
      return XCTFail("Unable to create snapshot bitmap")
    }
    host.cacheDisplay(in: host.bounds, to: bitmap)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
      return XCTFail("Unable to encode snapshot PNG")
    }
    let directory = URL(fileURLWithPath: output, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try data.write(
      to: directory.appendingPathComponent("correction-workbench.png"), options: .atomic)
    print("workbench_snapshot_size=\(Int(width))x\(Int(height))")
  }

  static func syntheticSnapshot() -> LocalDashboardSnapshot {
    let base = Date(timeIntervalSince1970: 1_800_000_000)
    func segment(
      _ id: String, _ stableID: String, _ label: String, _ category: String, _ seconds: Int,
      _ percentage: Int, _ confidence: ClassificationConfidence, _ suggestion: String? = nil
    ) -> LocalDailyActivitySegment {
      LocalDailyActivitySegment(
        id: id, label: label, representativeEventID: UUID(), stableID: stableID,
        suggestedName: suggestion, aliasConfirmed: false, category: category,
        durationSeconds: seconds, percentage: percentage, confidence: confidence,
        explanation: "One sustained focus work block lasted 45 minutes.")
    }
    return LocalDashboardSnapshot(
      generatedAt: base, windowStart: base, windowEnd: base.addingTimeInterval(3_600),
      switchCount: 3, switchesPerHour: 3, coverage: .good,
      earlySignal: LocalEarlySignal(
        status: .ready, observedFrom: base, observedThrough: base.addingTimeInterval(3_600),
        observedSeconds: 3_600, requiredSeconds: 0, evidenceEventCount: 9,
        focusedSeconds: 2_100, meaningfulSwitchCount: 3, longestUninterruptedSeconds: 1_080,
        observation: "One change of direction in the last 60 minutes.",
        suggestedAction: "Want 25 minutes on it, uninterrupted?", actionMinutes: 25),
      segments: [], focusFragmentation: nil,
      dailyActivity: [
        LocalDailyActivityDay(
          id: "2026-08-16", date: "2026-08-16", state: .ready, activeSeconds: 11_700,
          coverage: .good,
          segments: [
            segment("d-segment-0-focus_work", "abs_a", "Xcode", "FOCUS_WORK", 5_400, 45, .high),
            segment(
              "d-segment-1-communication", "abs_b", "Slack", "COMMUNICATION", 2_700, 24, .medium),
            segment("d-segment-2-reference", "abs_c", "Safari", "REFERENCE", 1_800, 16, .medium),
            segment(
              "d-segment-3-unclassified", "abs_d", "Unclassified", "UNCLASSIFIED", 1_200, 10,
              .none, "Sketch Companion"),
            segment("d-segment-4-other", "abs_e", "Other", "OTHER", 600, 5, .none),
          ])
      ])
  }
}

/// Renders the menu bar surface at the sizes a resizable window can actually
/// take — including through the real `NSPanel`, chrome and all — so the window
/// architecture and the header can be looked at instead of reasoned about.
///
/// Skipped unless `VELVT_WINDOW_SCREENSHOT_DIR` names an output directory,
/// following the existing synthetic snapshot tests. No app launch, no
/// Accessibility permission.
@MainActor
final class MenuBarWindowSnapshotTests: XCTestCase {
  func testRenderMenuBarSurfaceWhenRequested() throws {
    let output = try outputDirectory()
    registerWordmark()

    var sizes: [(String, CGSize)] = [
      ("preferred", MenuBarPopoverLayout.preferredContentSize),
      ("minimum", MenuBarPopoverLayout.minimumContentSize),
      ("wide", CGSize(width: 900, height: 620)),
    ]
    // Width ladder at a fixed height, then a height ladder at the preferred
    // width. `minimumContentSize` is read off these, not guessed.
    for width in stride(from: CGFloat(340), through: 620, by: 20) {
      sizes.append(("w\(Int(width))", CGSize(width: width, height: 480)))
    }
    for height in stride(from: CGFloat(240), through: 480, by: 40) {
      sizes.append(("h\(Int(height))", CGSize(width: 600, height: height)))
    }
    for (name, size) in sizes {
      try render(makeView(), named: "menu-bar-\(name).png", outputDirectory: output, size: size)
    }
    print("window_snapshot_count=\(sizes.count)")
  }

  /// The long-label case: a denied Accessibility permission puts "Collection
  /// paused: Accessibility permission required" in the header, which is the
  /// string most likely to be cut at the right edge when the window narrows.
  func testRenderLongHeaderLabelsWhenRequested() throws {
    let output = try outputDirectory()
    registerWordmark()
    let permissions = FakePermissionManager()
    let presentation = PermissionPresentationModel(
      permissionManager: permissions,
      onboardingStateStore: InMemoryOnboardingStateStore()
    )
    permissions.setStatus(.denied, for: .accessibility)
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))

    for width in [CGFloat(500), 600, 900] {
      try render(
        makeView(presentation: presentation),
        named: "menu-bar-denied-w\(Int(width)).png",
        outputDirectory: output,
        size: CGSize(width: width, height: 480)
      )
    }
  }

  /// The guided tour bar carries `layoutPriority(2)`, the highest in the
  /// surface. With the window short and the tour open, the header is the thing
  /// the layout would otherwise squeeze — and a squeezed fixed 30pt image box
  /// clips rather than shrinks. These shots are the check that it does not.
  func testRenderGuidedTourAtShortHeightsWhenRequested() throws {
    let output = try outputDirectory()
    registerWordmark()
    for height in [CGFloat(320), 400, 567] {
      let tour = GuidedTourModel()
      tour.start()
      try render(
        makeView(guidedTour: tour),
        named: "menu-bar-tour-h\(Int(height)).png",
        outputDirectory: output,
        size: CGSize(width: 600, height: height)
      )
    }
  }

  /// Renders through the real `NSPanel`, capturing the window's frame view so
  /// the title bar strip is in the picture. This is the shot that shows the
  /// header is not drawn under the chrome.
  func testRenderTheRealPanelWindowWhenRequested() throws {
    let output = try outputDirectory()
    registerWordmark()
    let presenter = MenuBarPanelPresenter()
    defer { presenter.close() }
    let hosting = NSHostingController(rootView: makeView())
    hosting.sizingOptions = []
    presenter.contentViewController = hosting
    presenter.maximumContentSize = CGSize(width: 2_000, height: 1_500)

    for (name, size) in [
      ("panel-preferred", MenuBarPopoverLayout.preferredContentSize),
      ("panel-minimum", MenuBarPopoverLayout.minimumContentSize),
      ("panel-resized-larger", CGSize(width: 860, height: 640)),
    ] {
      presenter.contentSize = size
      presenter.panel.orderFront(nil)
      presenter.panel.layoutIfNeeded()
      guard let frameView = presenter.panel.contentView?.superview else {
        return XCTFail("panel has no frame view")
      }
      frameView.layoutSubtreeIfNeeded()
      print(
        "panel \(name) content=\(presenter.contentSize) frame=\(presenter.panel.frame.size) "
          + "safeAreaTop=\(presenter.panel.contentView?.safeAreaInsets.top ?? -1)"
      )
      try write(frameView, named: "menu-bar-\(name).png", outputDirectory: output)
    }
  }

  private func outputDirectory() throws -> String {
    guard let output = ProcessInfo.processInfo.environment["VELVT_WINDOW_SCREENSHOT_DIR"] else {
      throw XCTSkip("Set VELVT_WINDOW_SCREENSHOT_DIR to render the menu bar surface")
    }
    return output
  }

  /// SwiftPM does not compile `Assets.xcassets`, so `Image("VelvtWordmark")`
  /// has no artwork to find under `swift test`. Registering the shipped SVG
  /// under the same name is attempted here for completeness; SwiftUI resolves
  /// `Image(_:)` through the asset catalog rather than through AppKit's named
  /// image table, so the wordmark renders as its empty 76x30 box. The box is
  /// what the clipping bug is about, and the box is measurable.
  private func registerWordmark() {
    guard NSImage(named: "VelvtWordmark") == nil else { return }
    let repoRoot = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .deletingLastPathComponent()
    let svg = repoRoot
      .appendingPathComponent("Assets.xcassets/VelvtWordmark.imageset/VelvtWordmark.svg")
    guard let image = NSImage(contentsOf: svg) else { return }
    image.isTemplate = true
    image.setName("VelvtWordmark")
  }

  private func makeView(
    presentation: PermissionPresentationModel? = nil,
    guidedTour: GuidedTourModel = GuidedTourModel()
  ) -> MenuBarPopoverView {
    let resolved =
      presentation
      ?? PermissionPresentationModel(
        permissionManager: FakePermissionManager(),
        onboardingStateStore: InMemoryOnboardingStateStore()
      )
    let client = FakeIPCClient()
    let messages = PassthroughSubject<ServerMessage, Never>()
    return MenuBarPopoverView(
      presentation: resolved,
      coordinator: ConcreteDisplayDataCoordinator(),
      serviceConnectionStatus: ServiceConnectionStatusModel(
        connectionStatus: Just(.connected).eraseToAnyPublisher()
      ),
      collectionActivityStatus: CollectionActivityStatusModel(
        collectionStatus: Just(.running).eraseToAnyPublisher()
      ),
      currentActivity: CurrentActivityModel(),
      serviceAlertModel: ServiceAlertModel(messages: Empty<ServerMessage, Never>()),
      accountStateManager: AccountStateManager(keychain: FakeKeychain()),
      ipcClient: client,
      menuStatusViewModel: MenuStatusViewModel(ipcClient: client, messages: messages),
      updateController: .disabled(),
      guidedTour: guidedTour,
      onEscape: {}
    )
  }

  private func render<V: View>(
    _ view: V,
    named name: String,
    outputDirectory: String,
    size: CGSize
  ) throws {
    let root = AnyView(
      view
        .frame(width: size.width, height: size.height, alignment: .top)
        .background(Color.velvtSurface)
        .preferredColorScheme(.dark)
    )
    let host = NSHostingView(rootView: root)
    host.frame = NSRect(origin: .zero, size: size)
    host.layoutSubtreeIfNeeded()
    try write(host, named: name, outputDirectory: outputDirectory)
  }

  private func write(_ view: NSView, named name: String, outputDirectory: String) throws {
    guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
      return XCTFail("Unable to create snapshot bitmap for \(name)")
    }
    view.cacheDisplay(in: view.bounds, to: bitmap)
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
      return XCTFail("Unable to encode snapshot PNG for \(name)")
    }
    let directory = URL(fileURLWithPath: outputDirectory, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    try data.write(to: directory.appendingPathComponent(name), options: .atomic)
  }
}
