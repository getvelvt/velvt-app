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
      ["Today", "Your Week", "Settings"]
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

    func testTourSafePopoverHeightIs450Points() {
        XCTAssertEqual(MenuBarPopoverLayout.preferredContentSize, CGSize(width: 660, height: 450))
    }

    func testSettingsRetainsEveryDestination() {
        #if DEBUG
      XCTAssertEqual(
        SettingsSubmenu.allCases.map(\.title),
        [
            "App Info", "Queued Events", "Collection Settings", "Onboarding & Tour",
            "Debug/Testing",
        ])
        #else
      XCTAssertEqual(
        SettingsSubmenu.allCases.map(\.title),
        [
            "App Info", "Queued Events", "Collection Settings", "Onboarding & Tour",
        ])
        #endif
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
