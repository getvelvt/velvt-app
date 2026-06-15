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
            Date(timeIntervalSince1970: 20)
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
                RawEvent(appName: "One", windowTitle: "First", occurredAt: Date(timeIntervalSince1970: 10)),
                RawEvent(appName: "Two", windowTitle: "Second", occurredAt: Date(timeIntervalSince1970: 20))
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
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "One"))
        accessibility.emitError(.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue))
        workspace.activate(.init(processIdentifier: 20, appName: "Two"))

        XCTAssertTrue(statuses.contains(.error("ax_observer_failed:\(AXError.invalidUIElement.rawValue)")))
        XCTAssertEqual(sink.events.map(\.appName), ["One", "Two"])
        XCTAssertEqual(accessibility.maximumActiveObserverCount, 1)
    }

    func testNilAndEmptyTitleNotificationsEmitEmptyRawEvents() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Initial"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 1),
            Date(timeIntervalSince1970: 2),
            Date(timeIntervalSince1970: 3)
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
                RawEvent(appName: "One", windowTitle: "Initial", occurredAt: Date(timeIntervalSince1970: 1)),
                RawEvent(appName: "One", windowTitle: "", occurredAt: Date(timeIntervalSince1970: 2)),
                RawEvent(appName: "One", windowTitle: "", occurredAt: Date(timeIntervalSince1970: 3))
            ]
        )
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
        XCTAssertEqual(sink.events.map(\.appName), ["App 1", "App 2", "App 3", "App 4", "App 5"])
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
    var startError: CollectionError?
    private(set) var operations: [Operation] = []
    private(set) var stopCallCount = 0
    private(set) var startCallCount = 0
    private(set) var activeObserverCount = 0
    private(set) var maximumActiveObserverCount = 0
    private var titleHandler: ((String?) -> Void)?
    private var errorHandler: ((CollectionError) -> Void)?

    func start(
        observing application: RunningApplication,
        titleHandler: @escaping (String?) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> String? {
        operations.append(.start(application.processIdentifier))
        startCallCount += 1
        if let startError {
            throw startError
        }
        activeObserverCount += 1
        maximumActiveObserverCount = max(maximumActiveObserverCount, activeObserverCount)
        self.titleHandler = titleHandler
        self.errorHandler = errorHandler
        return initialTitles[application.processIdentifier]
    }

    func stop() {
        operations.append(.stop)
        stopCallCount += 1
        if titleHandler != nil {
            activeObserverCount -= 1
        }
        titleHandler = nil
        errorHandler = nil
    }

    func emitTitle(_ title: String?) {
        titleHandler?(title)
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
