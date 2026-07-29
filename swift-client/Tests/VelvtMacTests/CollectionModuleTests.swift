import Combine
import Darwin
import XCTest
@testable import VelvtMac

final class CollectionModuleTests: XCTestCase {
    private var cancellables: Set<AnyCancellable> = []

    func testFakeCollectionAgentInjectsEventIntoDownstreamSink() throws {
        let sink = RecordingEventSink()
        let fakeAgent = FakeCollectionAgent(eventSink: sink)
        let agent: any CollectionAgentProtocol = fakeAgent
        let event = RawEvent(appName: "Editor", windowTitle: "Draft", occurredAt: Date(timeIntervalSince1970: 1))

        try agent.start()
        fakeAgent.injectEvent(event)

        XCTAssertEqual(sink.events, [event])
    }

    func testEventSinkFanoutForwardsEventsToEverySink() {
        let first = RecordingEventSink()
        let second = RecordingEventSink()
        let fanout = EventSinkFanout([first, second])
        let event = RawEvent(appName: "Editor", windowTitle: "Draft", occurredAt: Date(timeIntervalSince1970: 1))

        fanout.receive(event)

        XCTAssertEqual(first.events, [event])
        XCTAssertEqual(second.events, [event])
    }

    func testFakeCollectionAgentStartIsIdempotent() throws {
        let sink = RecordingEventSink()
        let agent = FakeCollectionAgent(eventSink: sink)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        try agent.start()

        XCTAssertEqual(statuses.filter { $0 == .running }.count, 1)
    }

    func testStopIsIdempotent() throws {
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let agent = makeAgent(workspace: workspace, accessibility: accessibility)

        try agent.start()
        agent.stop()
        agent.stop()
        agent.stop()

        XCTAssertEqual(workspace.stopCallCount, 1)
        XCTAssertEqual(accessibility.stopCallCount, 1)
    }

    func testStartWhileAlreadyRunningDoesNotDoubleRegister() throws {
        let permission = FakePermissionChecker(isTrusted: true)
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let agent = makeAgent(
            permission: permission,
            workspace: workspace,
            accessibility: accessibility
        )
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        permission.isTrusted = false
        try agent.start()

        XCTAssertEqual(workspace.startCallCount, 1)
        XCTAssertTrue(accessibility.operations.isEmpty)
        XCTAssertEqual(statuses.last, .running)
    }

    func testApplicationSwitchTearsDownPreviousObserverAndEmitsEvents() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "First", 20: "Second"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 10),
            Date(timeIntervalSince1970: 20),
            Date(timeIntervalSince1970: 30)
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "One"))
        workspace.activate(.init(processIdentifier: 20, appName: "Two"))

        XCTAssertEqual(accessibility.operations, [.stop, .start(10), .stop, .start(20)])
        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "One",
                    windowTitle: "First",
                    occurredAt: Date(timeIntervalSince1970: 10),
                    durationSeconds: 10
                )
            ]
        )
    }

    func testPermissionRevocationStopsCollectionAndSuppressesLaterEvents() throws {
        let sink = RecordingEventSink()
        let permission = FakePermissionChecker(isTrusted: true)
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Before", 20: "After"]
        let agent = makeAgent(
            sink: sink,
            permission: permission,
            workspace: workspace,
            accessibility: accessibility
        )
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "One"))
        permission.isTrusted = false
        workspace.activate(.init(processIdentifier: 20, appName: "Two"))
        accessibility.emitTitle("Ignored")

        XCTAssertEqual(statuses.last, .permissionRevoked)
        XCTAssertEqual(sink.events.count, 1)
        XCTAssertEqual(workspace.stopCallCount, 1)
        XCTAssertEqual(accessibility.stopCallCount, 2)
    }

    func testAbruptAppQuitPublishesSafeErrorAndAllowsNextAppActivation() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Before", 20: "After"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 10),
            Date(timeIntervalSince1970: 25),
            Date(timeIntervalSince1970: 30),
            Date(timeIntervalSince1970: 40)
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "One"))
        accessibility.emitError(.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue))
        workspace.activate(.init(processIdentifier: 20, appName: "Two"))

        XCTAssertTrue(statuses.contains(.error("ax_observer_failed:\(AXError.invalidUIElement.rawValue)")))
        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "One",
                    windowTitle: "Before",
                    occurredAt: Date(timeIntervalSince1970: 10),
                    durationSeconds: 15
                )
            ]
        )
        XCTAssertEqual(accessibility.maximumActiveObserverCount, 1)
    }

    func testNilAndEmptyTitleNotificationsAreDeduplicated() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Initial"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 1),
            Date(timeIntervalSince1970: 2),
            Date(timeIntervalSince1970: 3),
            Date(timeIntervalSince1970: 4)
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "One"))
        accessibility.emitTitle(nil)
        accessibility.emitTitle("")

        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "One",
                    windowTitle: "Initial",
                    occurredAt: Date(timeIntervalSince1970: 1),
                    durationSeconds: 1
                )
            ]
        )
    }

    func testFocusedDocumentChangeClosesPreviousBrowserDwellEvenWhenTitleIsUnchanged() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Dashboard"]
        accessibility.initialDocumentURLs = [10: "https://first.example/path"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 10),
            Date(timeIntervalSince1970: 25),
            Date(timeIntervalSince1970: 30)
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(
            .init(processIdentifier: 10, appName: "Browser", bundleIdentifier: "com.apple.Safari")
        )
        accessibility.emitActivity(title: "Dashboard", documentURL: "https://second.example/other")

        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "Browser",
                    bundleIdentifier: "com.apple.Safari",
                    windowTitle: "Dashboard",
                    focusedDocumentURL: "https://first.example/path",
                    occurredAt: Date(timeIntervalSince1970: 10),
                    durationSeconds: 15
                )
            ]
        )
    }

    func testBrowserCapabilityRegistryCoversSupportedFamiliesAndReleaseChannels() {
        let supported = [
            "com.apple.Safari",
            "com.google.Chrome",
            "com.google.Chrome.beta",
            "com.google.Chrome.dev",
            "com.google.Chrome.canary",
            "org.chromium.Chromium",
            "com.microsoft.edgemac",
            "com.microsoft.edgemac.Beta",
            "com.microsoft.edgemac.Dev",
            "com.microsoft.edgemac.Canary",
            "com.brave.Browser",
            "com.brave.Browser.beta",
            "com.brave.Browser.nightly",
            "company.thebrowser.Browser",
            "company.thebrowser.dia",
            "org.mozilla.firefox",
            "org.mozilla.firefox.developer",
            "com.operasoftware.Opera",
            "com.operasoftware.OperaGX",
            "com.vivaldi.Vivaldi",
            "com.kagi.kagimacOS"
        ]

        for bundleIdentifier in supported {
            XCTAssertTrue(
                AXApplicationObserver.isSupportedBrowser(
                    bundleIdentifier: bundleIdentifier),
                bundleIdentifier
            )
        }
        XCTAssertFalse(
            AXApplicationObserver.isSupportedBrowser(bundleIdentifier: "com.apple.TextEdit")
        )
        XCTAssertFalse(AXApplicationObserver.isSupportedBrowser(bundleIdentifier: nil))
    }

    func testDuplicateActivityNotificationDoesNotSplitDwell() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Same"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 10),
            Date(timeIntervalSince1970: 20),
            Date(timeIntervalSince1970: 30),
            Date(timeIntervalSince1970: 40)
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Browser"))
        accessibility.emitActivity(title: "Same", documentURL: nil)
        accessibility.emitActivity(title: "Changed", documentURL: nil)

        XCTAssertEqual(sink.events.first?.occurredAt, Date(timeIntervalSince1970: 10))
        XCTAssertEqual(sink.events.first?.durationSeconds, 20)
    }

    func testRapidAppSwitchesKeepOneObserverAndSuppressDuplicateActivation() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [1: "One", 2: "Two", 3: "Three", 4: "Four", 5: "Five"]
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility)

        try agent.start()
        let start = ContinuousClock.now
        for processIdentifier in 1 ... 5 {
            workspace.activate(.init(processIdentifier: pid_t(processIdentifier), appName: "App \(processIdentifier)"))
        }
        let elapsed = ContinuousClock.now - start
        workspace.activate(.init(processIdentifier: 5, appName: "App 5"))

        XCTAssertLessThan(elapsed, .milliseconds(100))
        XCTAssertEqual(accessibility.startCallCount, 5)
        XCTAssertEqual(accessibility.maximumActiveObserverCount, 1)
        XCTAssertEqual(accessibility.activeObserverCount, 1)
        XCTAssertEqual(sink.events.map(\.appName), ["App 1", "App 2", "App 3", "App 4"])
    }

    func testStopFlushesTheCurrentDwellIntervalWithTheConfiguredCap() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Long task"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 10),
            Date(timeIntervalSince1970: 10_000)
        ])
        let agent = AXCollectionAgent(
            eventSink: sink,
            permissionChecker: FakePermissionChecker(isTrusted: true),
            workspaceObserver: workspace,
            accessibilityObserver: accessibility,
            now: dates.next,
            maximumDwellDuration: 30 * 60
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        agent.stop()

        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "Editor",
                    windowTitle: "Long task",
                    occurredAt: Date(timeIntervalSince1970: 10),
                    durationSeconds: 30 * 60
                )
            ]
        )
    }

    func testAccessibilityPermissionErrorAfterStartPublishesPermissionRevoked() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.startError = .permissionRevoked
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "One"))

        XCTAssertEqual(statuses.last, .permissionRevoked)
        XCTAssertTrue(sink.events.isEmpty)
        XCTAssertEqual(workspace.stopCallCount, 1)
    }

    func testCurrentApplicationPermissionErrorDuringStartTearsDownAndPublishesRevoked() throws {
        let workspace = FakeWorkspaceObserver()
        workspace.currentApplication = .init(processIdentifier: 10, appName: "One")
        let accessibility = FakeAccessibilityObserver()
        accessibility.startError = .permissionRevoked
        let agent = makeAgent(workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()

        XCTAssertEqual(statuses.last, .permissionRevoked)
        XCTAssertEqual(workspace.stopCallCount, 1)
    }

    func testUnobservableCurrentApplicationDuringStartKeepsListeningForNextActivation() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        workspace.currentApplication = .init(processIdentifier: 10, appName: "Unobservable")
        let accessibility = FakeAccessibilityObserver()
        accessibility.startErrors = [
            10: .observerRegistrationFailed(code: AXError.noValue.rawValue)
        ]
        accessibility.initialTitles = [20: "Next Window"]
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 20, appName: "Editor"))

        XCTAssertTrue(statuses.contains(.error("ax_observer_registration_failed:\(AXError.noValue.rawValue)")))
        XCTAssertEqual(statuses.filter { $0 == .running }.count, 1)
        XCTAssertEqual(workspace.stopCallCount, 0)
        XCTAssertTrue(sink.events.isEmpty)
    }

    func testNoEventsAreGeneratedWithoutExplicitNotification() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility)

        try agent.start()
        let noEvent = expectation(description: "No polling event")
        noEvent.isInverted = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            if !sink.events.isEmpty {
                noEvent.fulfill()
            }
        }

        wait(for: [noEvent], timeout: 0.5)
        XCTAssertTrue(sink.events.isEmpty)
    }

    func testDeniedPermissionPublishesPermissionRevokedWithoutRegisteringObservers() throws {
        let permission = FakePermissionChecker(isTrusted: false)
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let agent = makeAgent(permission: permission, workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()

        XCTAssertEqual(statuses.last, .permissionRevoked)
        XCTAssertEqual(workspace.startCallCount, 0)
        XCTAssertTrue(accessibility.operations.isEmpty)
    }

    func testAdditionalWorkspaceNotificationHandlerDoesNotEnterCoreAgentLoop() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility)
        var additionalNotificationCount = 0
        workspace.addAdditionalNotificationHandler {
            additionalNotificationCount += 1
        }

        try agent.start()
        workspace.fireAdditionalNotification()

        XCTAssertEqual(additionalNotificationCount, 1)
        XCTAssertTrue(accessibility.operations.isEmpty)
        XCTAssertTrue(sink.events.isEmpty)
    }

    private func makeAgent(
        sink: RecordingEventSink = RecordingEventSink(),
        permission: FakePermissionChecker = FakePermissionChecker(isTrusted: true),
        workspace: FakeWorkspaceObserver = FakeWorkspaceObserver(),
        accessibility: FakeAccessibilityObserver = FakeAccessibilityObserver(),
        now: @escaping () -> Date = Date.init
    ) -> AXCollectionAgent {
        AXCollectionAgent(
            eventSink: sink,
            permissionChecker: permission,
            workspaceObserver: workspace,
            accessibilityObserver: accessibility,
            now: now
        )
    }
}

private final class RecordingEventSink: EventSink {
    private(set) var events: [RawEvent] = []

    func receive(_ event: RawEvent) {
        events.append(event)
    }
}

private final class FakePermissionChecker: AccessibilityPermissionChecking {
    var isTrusted: Bool

    init(isTrusted: Bool) {
        self.isTrusted = isTrusted
    }

    func hasPermission() -> Bool {
        isTrusted
    }
}

private final class FakeWorkspaceObserver: WorkspaceActivationObserving {
    var currentApplication: RunningApplication?
    private var handler: ((RunningApplication) -> Void)?
    private var additionalNotificationHandler: (() -> Void)?
    private(set) var startCallCount = 0
    private(set) var stopCallCount = 0

    func start(activationHandler: @escaping (RunningApplication) -> Void) -> RunningApplication? {
        startCallCount += 1
        handler = activationHandler
        return currentApplication
    }

    func stop() {
        guard handler != nil else {
            return
        }
        stopCallCount += 1
        handler = nil
    }

    func activate(_ application: RunningApplication) {
        handler?(application)
    }

    func addAdditionalNotificationHandler(_ handler: @escaping () -> Void) {
        additionalNotificationHandler = handler
    }

    func fireAdditionalNotification() {
        additionalNotificationHandler?()
    }
}

private final class FakeAccessibilityObserver: AccessibilityObserving {
    enum Operation: Equatable {
        case start(pid_t)
        case stop
    }

    var initialTitles: [pid_t: String] = [:]
    var initialDocumentURLs: [pid_t: String] = [:]
    var startError: CollectionError?
    var startErrors: [pid_t: CollectionError] = [:]
    private(set) var operations: [Operation] = []
    private(set) var stopCallCount = 0
    private(set) var startCallCount = 0
    private(set) var activeObserverCount = 0
    private(set) var maximumActiveObserverCount = 0
    private var activityHandler: ((FocusedActivity) -> Void)?
    private var errorHandler: ((CollectionError) -> Void)?

    func start(
        observing application: RunningApplication,
        activityHandler: @escaping (FocusedActivity) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> FocusedActivity {
        operations.append(.start(application.processIdentifier))
        startCallCount += 1
        if let startError = startErrors[application.processIdentifier] {
            throw startError
        }
        if let startError {
            throw startError
        }
        activeObserverCount += 1
        maximumActiveObserverCount = max(maximumActiveObserverCount, activeObserverCount)
        self.activityHandler = activityHandler
        self.errorHandler = errorHandler
        return FocusedActivity(
            windowTitle: initialTitles[application.processIdentifier],
            focusedDocumentURL: initialDocumentURLs[application.processIdentifier]
        )
    }

    func stop() {
        operations.append(.stop)
        stopCallCount += 1
        if activityHandler != nil {
            activeObserverCount -= 1
        }
        activityHandler = nil
        errorHandler = nil
    }

    func emitTitle(_ title: String?) {
        activityHandler?(FocusedActivity(windowTitle: title))
    }

    func emitActivity(title: String?, documentURL: String?) {
        activityHandler?(FocusedActivity(windowTitle: title, focusedDocumentURL: documentURL))
    }

    func emitError(_ error: CollectionError) {
        errorHandler?(error)
    }
}

private final class DateQueue {
    private var dates: [Date]

    init(_ dates: [Date]) {
        self.dates = dates
    }

    func next() -> Date {
        dates.removeFirst()
    }
}
