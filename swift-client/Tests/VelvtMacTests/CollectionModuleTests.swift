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
            Date(timeIntervalSince1970: 30),
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

    /// A departure is reported while it is happening. Reported only when it
    /// ended, it reached the service's drift gate at the moment the person
    /// came back, and the offer it produced was withdrawn before anyone saw
    /// it. The dwell being replaced is always handed over first.
    func testEachActivityIsReportedAsItBeginsRightAfterTheDwellItReplaces() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Draft", 20: "Feed"]
        let dates = DateQueue([10, 20, 25, 30, 40].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        workspace.activate(.init(processIdentifier: 20, appName: "Browser"))
        accessibility.emitTitle("Feed")
        accessibility.emitTitle("Video")

        let at = { (seconds: TimeInterval) in Date(timeIntervalSince1970: seconds) }
        XCTAssertEqual(
            sink.journal,
            [
                .began(RawEvent(appName: "Editor", windowTitle: "Draft", occurredAt: at(10))),
                .closed(
                    RawEvent(appName: "Editor", windowTitle: "Draft", occurredAt: at(10), durationSeconds: 10)),
                .began(RawEvent(appName: "Browser", windowTitle: "Feed", occurredAt: at(20))),
                .closed(
                    RawEvent(appName: "Browser", windowTitle: "Feed", occurredAt: at(20), durationSeconds: 10)),
                .began(RawEvent(appName: "Browser", windowTitle: "Video", occurredAt: at(30))),
            ],
            "the repeated notification at 25 is the same activity and reports nothing"
        )
    }

    /// Splitting a dwell or ending collection starts no new activity, so
    /// neither reports one.
    func testFlushAndStopReportNoActivityBeginning() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Draft"]
        let dates = DateQueue([10, 40].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        XCTAssertTrue(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 25)))
        agent.stop()

        let began = sink.journal.filter {
            if case .began = $0 { return true }
            return false
        }
        let opened = RawEvent(
            appName: "Editor", windowTitle: "Draft", occurredAt: Date(timeIntervalSince1970: 10))
        XCTAssertEqual(began, [.began(opened)])
        XCTAssertEqual(sink.events.count, 2, "the flushed span and the final one")
    }

    func testEventSinkFanoutForwardsBeginningsToEverySink() {
        let first = RecordingEventSink()
        let second = RecordingEventSink()
        let fanout = EventSinkFanout([first, second])
        let event = RawEvent(appName: "Editor", windowTitle: "Draft", occurredAt: Date(timeIntervalSince1970: 1))

        fanout.activityBegan(event)

        XCTAssertEqual(first.journal, [.began(event)])
        XCTAssertEqual(second.journal, [.began(event)])
        XCTAssertTrue(first.events.isEmpty, "a beginning is not a measured dwell")
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
            Date(timeIntervalSince1970: 40),
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

        XCTAssertTrue(statuses.contains(.limited("ax_observer_failed:\(AXError.invalidUIElement.rawValue)")))
        XCTAssertEqual(statuses.last, .running)
        // The window dwell closes where the observer failed. Until the next
        // activation the application is still the one in front, so the rest
        // of its time is its own at application level, not unobserved.
        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "One",
                    windowTitle: "Before",
                    occurredAt: Date(timeIntervalSince1970: 10),
                    durationSeconds: 15
                ),
                RawEvent(
                    appName: "One",
                    windowTitle: "",
                    occurredAt: Date(timeIntervalSince1970: 25),
                    durationSeconds: 5
                ),
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
            Date(timeIntervalSince1970: 4),
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
            Date(timeIntervalSince1970: 30),
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
            "com.kagi.kagimacOS",
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
            Date(timeIntervalSince1970: 40),
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
        for processIdentifier in 1...5 {
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
            Date(timeIntervalSince1970: 10_000),
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
        let dates = DateQueue([100, 130, 160].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility, now: dates.next)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 20, appName: "Editor"))

        XCTAssertEqual(
            statuses,
            [
                .idle,
                .running,
                .limited("ax_observer_registration_failed:\(AXError.noValue.rawValue)"),
                .running,
            ]
        )
        XCTAssertEqual(workspace.stopCallCount, 0)
        // The application in front at start has its dwell from the start, at
        // application level, like one activated later.
        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "Unobservable",
                    windowTitle: "",
                    occurredAt: Date(timeIntervalSince1970: 100),
                    durationSeconds: 30
                )
            ]
        )
        withExtendedLifetime(agent) {}
    }

    /// The founder's Mac on 2026-09-26: collection ran all day and the window
    /// said "Collection paused". One application could not be observed at
    /// window level, and the status that failure published was never replaced,
    /// because only `start()` ever reported `.running`.
    func testStatusReturnsToRunningWhenTheNextActivationIsObserved() throws {
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.startErrors = [
            10: .observerRegistrationFailed(code: AXError.noValue.rawValue)
        ]
        accessibility.initialTitles = [20: "Draft"]
        let agent = makeAgent(workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Windowless"))
        workspace.activate(.init(processIdentifier: 20, appName: "Editor"))

        XCTAssertEqual(
            statuses,
            [
                .idle,
                .running,
                .limited("ax_observer_registration_failed:\(AXError.noValue.rawValue)"),
                .running,
            ]
        )
        XCTAssertTrue(agent.isRunning)
    }

    /// The other failure path: an observer that was registered and later
    /// failed. The agent drops that application's observer and must register
    /// afresh on the next activation, even when it is the same application.
    func testObserverFailureIsLimitedAndTheNextActivationRegistersAgain() throws {
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Draft", 20: "Inbox"]
        let agent = makeAgent(workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)
        let failure = "ax_observer_failed:\(AXError.invalidUIElement.rawValue)"

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        accessibility.emitError(.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue))

        XCTAssertEqual(statuses.last, .limited(failure))
        XCTAssertTrue(agent.isRunning)
        XCTAssertEqual(accessibility.activeObserverCount, 0)

        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))

        XCTAssertEqual(accessibility.operations.filter { $0 == .start(10) }.count, 2)
        XCTAssertEqual(accessibility.activeObserverCount, 1)
        XCTAssertEqual(statuses, [.idle, .running, .limited(failure), .running])

        accessibility.emitError(.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue))
        workspace.activate(.init(processIdentifier: 20, appName: "Mail"))

        XCTAssertEqual(statuses.last, .running)
        XCTAssertEqual(accessibility.maximumActiveObserverCount, 1)
    }

    /// Each transition is published once and logged once, and a report that
    /// changes nothing is neither: a second application that fails the same
    /// way, or a second one that registers, is not a transition.
    func testEveryStatusTransitionIsPublishedAndLoggedExactlyOnce() throws {
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let noWindow = CollectionError.observerRegistrationFailed(code: AXError.noValue.rawValue)
        accessibility.startErrors = [10: noWindow, 30: noWindow]
        accessibility.initialTitles = [20: "Draft", 40: "Notes"]
        var transitions: [StatusTransition] = []
        let agent = makeAgent(
            workspace: workspace,
            accessibility: accessibility,
            reportStatusTransition: { transitions.append(StatusTransition(from: $0, to: $1)) }
        )
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)
        let limited = CollectionStatus.limited("ax_observer_registration_failed:\(AXError.noValue.rawValue)")

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Windowless"))
        workspace.activate(.init(processIdentifier: 30, appName: "Also Windowless"))
        workspace.activate(.init(processIdentifier: 20, appName: "Editor"))
        workspace.activate(.init(processIdentifier: 40, appName: "Notes"))
        agent.stop()

        XCTAssertEqual(statuses, [.idle, .running, limited, .running, .idle])
        XCTAssertEqual(
            transitions,
            [
                StatusTransition(from: .idle, to: .running),
                StatusTransition(from: .running, to: limited),
                StatusTransition(from: limited, to: .running),
                StatusTransition(from: .running, to: .idle),
            ]
        )
    }

    func testPermissionRevocationAndRefusedStartAreLoggedTransitions() throws {
        let permission = FakePermissionChecker(isTrusted: false)
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Draft"]
        var transitions: [StatusTransition] = []
        let agent = makeAgent(
            permission: permission,
            workspace: workspace,
            accessibility: accessibility,
            reportStatusTransition: { transitions.append(StatusTransition(from: $0, to: $1)) }
        )

        XCTAssertThrowsError(try agent.start())
        permission.isTrusted = true
        try agent.start()
        permission.isTrusted = false
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))

        XCTAssertEqual(
            transitions,
            [
                StatusTransition(from: .idle, to: .permissionRevoked),
                StatusTransition(from: .permissionRevoked, to: .running),
                StatusTransition(from: .running, to: .permissionRevoked),
            ]
        )
    }

    /// A registration can succeed and the agent still stop before it reports:
    /// the first activity it reads finds the permission gone. That stop is the
    /// truth, and the registration's `.running` must not overwrite it.
    func testRegistrationThatEndsInRevocationDoesNotReportRunning() throws {
        let permission = FakePermissionChecker(isTrusted: true)
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Draft"]
        accessibility.onStart = { _ in permission.isTrusted = false }
        let agent = makeAgent(permission: permission, workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))

        XCTAssertEqual(statuses, [.idle, .running, .permissionRevoked])
        XCTAssertFalse(agent.isRunning)
    }

    /// The observer's first callback runs on its own queue and can fail before
    /// the activation that registered it has reported. The failure is the
    /// newer fact, and the registration's `.running` must not overwrite it:
    /// that would say "Collection active" for an application nothing observes
    /// until the next switch.
    func testObserverThatFailsBeforeItsRegistrationReportsStaysLimited() throws {
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "Draft", 20: "Inbox"]
        let failure = CollectionError.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue)
        accessibility.onRegistered = { processIdentifier in
            if processIdentifier == 10 {
                accessibility.emitError(failure)
            }
        }
        let agent = makeAgent(workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)
        let limited = CollectionStatus.limited("ax_observer_failed:\(AXError.invalidUIElement.rawValue)")

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))

        XCTAssertEqual(statuses, [.idle, .running, limited])
        XCTAssertTrue(agent.isRunning)

        workspace.activate(.init(processIdentifier: 20, appName: "Mail"))

        XCTAssertEqual(statuses, [.idle, .running, limited, .running])
    }

    // MARK: - Applications that cannot be observed at window level
    //
    // Found by PR #59's review. When the window-level observer could not be
    // registered for the application just activated (most often -25212, no
    // focused or main window yet), the agent reported nothing for it. The dwell
    // before it stayed open and absorbed its time at the next switch, so the
    // time went to the wrong application, and a departure from the anchor to it
    // never reached the service's drift gate.

    func testAnApplicationThatCannotBeObservedAtWindowLevelGetsADwellOfItsOwn() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "main.swift"]
        accessibility.startErrors = [20: .observerRegistrationFailed(code: AXError.noValue.rawValue)]
        let declared = DeclaredAppMetadata(
            declaredAppCategory: "public.app-category.social-networking",
            documentTypeIDs: ["public.vcard"]
        )
        let metadata = StubMetadataProvider(["com.apple.MobileSMS": declared])
        let dates = DateQueue([100, 320, 368, 400].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            metadata: metadata,
            now: dates.next
        )
        let xcode = RunningApplication(processIdentifier: 10, appName: "Xcode", bundleIdentifier: "com.apple.dt.Xcode")
        let messages = RunningApplication(
            processIdentifier: 20, appName: "Messages", bundleIdentifier: "com.apple.MobileSMS")

        try agent.start()
        workspace.activate(xcode)
        workspace.activate(messages)
        workspace.activate(xcode)

        let at = { (seconds: TimeInterval) in Date(timeIntervalSince1970: seconds) }
        let anchor = RawEvent(
            appName: "Xcode", bundleIdentifier: "com.apple.dt.Xcode", windowTitle: "main.swift", occurredAt: at(100))
        // Application-level identity only: the name, the bundle identifier and
        // what the application declares about itself. No title is invented,
        // and there is no document URL to report.
        let away = RawEvent(
            appName: "Messages",
            bundleIdentifier: "com.apple.MobileSMS",
            declaredAppCategory: "public.app-category.social-networking",
            documentTypeIDs: ["public.vcard"],
            windowTitle: "",
            focusedDocumentURL: nil,
            occurredAt: at(320)
        )
        let back = anchor.reanchored(at: at(368))
        XCTAssertEqual(
            sink.journal,
            [
                .began(anchor),
                .closed(anchor.withDuration(seconds: 220)),
                .began(away),
                .closed(away.withDuration(seconds: 48)),
                .began(back),
            ],
            "the anchor closes at the activation instant and the unobservable application's dwell is reported "
                + "in progress when it begins and closed at the return"
        )
        withExtendedLifetime(agent) {}
    }

    /// An application in front with no window yet is registered all the same,
    /// and its first window to gain focus is the later retry: no polling, and
    /// no wait for the next activation.
    func testAnApplicationWithNoWindowYetIsObservedAtWindowLevelOnceOneGainsFocus() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "main.swift"]
        accessibility.awaitingWindow = [30]
        let dates = DateQueue([100, 200, 230, 300].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility, now: dates.next)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)
        let noWindow = CollectionStatus.limited("ax_observer_registration_failed:\(AXError.noValue.rawValue)")

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Xcode"))
        workspace.activate(.init(processIdentifier: 30, appName: "Finder", bundleIdentifier: "com.apple.finder"))

        XCTAssertEqual(statuses, [.idle, .running, noWindow])
        XCTAssertEqual(accessibility.activeObserverCount, 1, "registered for the application, waiting for a window")

        accessibility.emitTitle("Downloads")

        let at = { (seconds: TimeInterval) in Date(timeIntervalSince1970: seconds) }
        let anchor = RawEvent(appName: "Xcode", windowTitle: "main.swift", occurredAt: at(100))
        let finder = RawEvent(
            appName: "Finder", bundleIdentifier: "com.apple.finder", windowTitle: "", occurredAt: at(200))
        let window = RawEvent(
            appName: "Finder", bundleIdentifier: "com.apple.finder", windowTitle: "Downloads", occurredAt: at(230))
        XCTAssertEqual(
            sink.journal,
            [
                .began(anchor),
                .closed(anchor.withDuration(seconds: 100)),
                .began(finder),
                .closed(finder.withDuration(seconds: 30)),
                .began(window),
            ]
        )
        XCTAssertEqual(statuses, [.idle, .running, noWindow, .running])
        XCTAssertEqual(accessibility.operations.filter { $0 == .start(30) }.count, 1)
        withExtendedLifetime(agent) {}
    }

    /// Another activation of an application that could not be observed
    /// registers again. When that reaches a window, the window-level dwell
    /// takes over from the activation instant; when it does not, the dwell
    /// already open carries on and nothing is reported twice.
    func testReactivatingAnApplicationThatCouldNotBeObservedRetriesTheRegistration() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.startErrors = [20: .observerRegistrationFailed(code: AXError.cannotComplete.rawValue)]
        let dates = DateQueue([200, 210, 250, 300].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility, now: dates.next)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)
        let mail = RunningApplication(processIdentifier: 20, appName: "Mail")

        try agent.start()
        workspace.activate(mail)
        workspace.activate(mail)

        let at = { (seconds: TimeInterval) in Date(timeIntervalSince1970: seconds) }
        let applicationLevel = RawEvent(appName: "Mail", windowTitle: "", occurredAt: at(200))
        XCTAssertEqual(sink.journal, [.began(applicationLevel)], "a failed retry reports nothing new")

        accessibility.startErrors = [:]
        accessibility.initialTitles = [20: "Inbox"]
        workspace.activate(mail)

        let inbox = RawEvent(appName: "Mail", windowTitle: "Inbox", occurredAt: at(250))
        XCTAssertEqual(
            sink.journal,
            [
                .began(applicationLevel),
                .closed(applicationLevel.withDuration(seconds: 50)),
                .began(inbox),
            ]
        )
        XCTAssertEqual(accessibility.operations.filter { $0 == .start(20) }.count, 3)
        XCTAssertEqual(
            statuses,
            [.idle, .running, .limited("ax_observer_registration_failed:\(AXError.cannotComplete.rawValue)"), .running]
        )

        workspace.activate(mail)
        XCTAssertEqual(
            accessibility.operations.filter { $0 == .start(20) }.count, 3,
            "observed at window level: a repeated activation registers nothing")
        withExtendedLifetime(agent) {}
    }

    /// A window can gain focus, and its callback report, before the
    /// registration that found no window has reported. The window is the
    /// newer fact: it is not replaced by an application-level dwell, and the
    /// status is not set back to limited.
    func testAWindowReachedBeforeTheRegistrationReportsIsNotReplacedByLess() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "main.swift"]
        accessibility.awaitingWindow = [30]
        accessibility.onRegistered = { processIdentifier in
            if processIdentifier == 30 {
                accessibility.emitTitle("Downloads")
            }
        }
        let dates = DateQueue([100, 200, 201, 300].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility, now: dates.next)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Xcode"))
        workspace.activate(.init(processIdentifier: 30, appName: "Finder"))

        let at = { (seconds: TimeInterval) in Date(timeIntervalSince1970: seconds) }
        let anchor = RawEvent(appName: "Xcode", windowTitle: "main.swift", occurredAt: at(100))
        let window = RawEvent(appName: "Finder", windowTitle: "Downloads", occurredAt: at(201))
        XCTAssertEqual(
            sink.journal,
            [
                .began(anchor),
                .closed(anchor.withDuration(seconds: 101)),
                .began(window),
            ]
        )
        XCTAssertEqual(statuses, [.idle, .running])
        withExtendedLifetime(agent) {}
    }

    /// Velvt's own window is observed like any other application's, and the
    /// service classifies Velvt as SYSTEM, which the drift gate never counts.
    /// When its panel is activated before it is key, it gets an
    /// application-level dwell like any other application, where it used to
    /// be folded into the dwell before it.
    func testVelvtActivatedBeforeItsPanelIsKeyIsAnApplicationLevelDwellLikeAnyOther() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "main.swift"]
        accessibility.awaitingWindow = [99]
        let dates = DateQueue([100, 150, 152, 300].map { Date(timeIntervalSince1970: $0) })
        let agent = makeAgent(sink: sink, workspace: workspace, accessibility: accessibility, now: dates.next)

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Xcode"))
        workspace.activate(.init(processIdentifier: 99, appName: "Velvt", bundleIdentifier: "com.velvt.mac"))
        accessibility.emitTitle("Velvt")

        let at = { (seconds: TimeInterval) in Date(timeIntervalSince1970: seconds) }
        let velvt = RawEvent(appName: "Velvt", bundleIdentifier: "com.velvt.mac", windowTitle: "", occurredAt: at(150))
        XCTAssertEqual(
            Array(sink.journal.suffix(4)),
            [
                .closed(
                    RawEvent(appName: "Xcode", windowTitle: "main.swift", occurredAt: at(100), durationSeconds: 50)),
                .began(velvt),
                .closed(velvt.withDuration(seconds: 2)),
                .began(
                    RawEvent(
                        appName: "Velvt", bundleIdentifier: "com.velvt.mac", windowTitle: "Velvt", occurredAt: at(152))),
            ]
        )
        withExtendedLifetime(agent) {}
    }

    func testStatusLogLinesArePersistedAndCarryOnlyFixedCodes() {
        let noWindow = "ax_observer_registration_failed:\(AXError.noValue.rawValue)"
        let transitions: [StatusTransition] = [
            StatusTransition(from: .idle, to: .running),
            StatusTransition(from: .running, to: .limited(noWindow)),
            StatusTransition(from: .limited(noWindow), to: .running),
            StatusTransition(from: .running, to: .permissionRevoked),
            StatusTransition(from: .permissionRevoked, to: .idle),
            StatusTransition(from: .running, to: .error("ax_observer_failed")),
        ]

        let lines = transitions.map { CollectionStatusLog.line(from: $0.from, to: $0.to) }

        XCTAssertTrue(lines.allSatisfy { $0.level == .default })
        XCTAssertEqual(
            lines.map(\.message),
            [
                "collection_status_changed from=idle to=running",
                "collection_status_changed from=running to=limited reason=\(noWindow)",
                "collection_status_changed from=limited to=running",
                "collection_status_changed from=running to=permission_revoked",
                "collection_status_changed from=permission_revoked to=idle",
                "collection_status_changed from=running to=error reason=ax_observer_failed",
            ]
        )
    }

    /// The codes a transition carries come from the agent, never from the
    /// application it was observing.
    func testLoggedTransitionsNeverCarryApplicationNamesOrTitles() throws {
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.startErrors = [10: .observerRegistrationFailed(code: AXError.noValue.rawValue)]
        accessibility.initialTitles = [20: "Quarterly Secret Plan"]
        var messages: [String] = []
        let agent = makeAgent(
            workspace: workspace,
            accessibility: accessibility,
            reportStatusTransition: { messages.append(CollectionStatusLog.line(from: $0, to: $1).message) }
        )

        try agent.start()
        workspace.activate(
            .init(processIdentifier: 10, appName: "Private Diary", bundleIdentifier: "com.example.diary"))
        workspace.activate(.init(processIdentifier: 20, appName: "Secret Editor"))
        accessibility.emitError(.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue))

        XCTAssertEqual(messages.count, 4)
        for message in messages {
            for forbidden in ["Private Diary", "com.example.diary", "Secret Editor", "Quarterly Secret Plan"] {
                XCTAssertFalse(message.contains(forbidden), message)
            }
        }
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

    /// The caller has to be able to tell a start from a refusal to start. This
    /// path published `.permissionRevoked` and then returned normally out of a
    /// `throws` function, so `PermissionCollectionCoordinator` counted it as a
    /// start and reported collection running against an agent that never began.
    func testDeniedPermissionThrowsAndPublishesPermissionRevokedWithoutRegisteringObservers() {
        let permission = FakePermissionChecker(isTrusted: false)
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        let agent = makeAgent(permission: permission, workspace: workspace, accessibility: accessibility)
        var statuses: [CollectionStatus] = []
        agent.status.sink { statuses.append($0) }.store(in: &cancellables)

        XCTAssertThrowsError(try agent.start()) { error in
            XCTAssertEqual(error as? CollectionError, .permissionRevoked)
        }

        XCTAssertEqual(statuses.last, .permissionRevoked)
        XCTAssertFalse(agent.isRunning)
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

    // MARK: - Block-end dwell flush
    //
    // The defect these cover: a dwell is start-stamped and end-delivered, so
    // the agent cannot know its length until the user leaves. The only flush
    // paths are `stop()`, AX-observer failure, and permission revocation, so a
    // dwell still in progress when a work block ends is never emitted at all.
    // `flushPendingDwell(at:)` is the primitive that closes it; the tests below
    // pin the conservation property that makes splitting a dwell safe.

    func testDwellStillInProgressIsNeverEmittedWithoutAFlush() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "code", 20: "chat"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 0),
            Date(timeIntervalSince1970: 182),
            Date(timeIntervalSince1970: 263),
            // Consumed by `deinit`'s `stop()` once the agent goes out of scope.
            Date(timeIntervalSince1970: 1_598),
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        workspace.activate(.init(processIdentifier: 20, appName: "Slack"))
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        // The user now stays in Editor past the end of the block. No switch
        // happens, so nothing further is emitted and the last 1,237 seconds of
        // the block are invisible to every consumer of the event stream.

        XCTAssertEqual(sink.events.count, 2)
        XCTAssertEqual(sink.events.map(\.appName), ["Editor", "Slack"])
        XCTAssertEqual(
            sink.events.map(\.durationSeconds).reduce(0, +),
            263,
            "Only the two closed dwells are accounted for; the open one is not."
        )
    }

    func testFlushEmitsTheDwellMeasuredSoFarAndDoesNotDoubleCountTheLaterSwitch() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "code", 20: "chat"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 0),
            Date(timeIntervalSince1970: 182),
            Date(timeIntervalSince1970: 263),
            Date(timeIntervalSince1970: 1_598),
            // Consumed by `deinit`'s `stop()` once the agent goes out of scope.
            Date(timeIntervalSince1970: 1_700),
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        workspace.activate(.init(processIdentifier: 20, appName: "Slack"))
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))

        // The block's last second. The dwell is still open and has run 1,232s.
        XCTAssertTrue(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 1_495)))
        // The user genuinely leaves 103 seconds later.
        workspace.activate(.init(processIdentifier: 20, appName: "Slack"))

        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "Editor",
                    windowTitle: "code",
                    occurredAt: Date(timeIntervalSince1970: 0),
                    durationSeconds: 182
                ),
                RawEvent(
                    appName: "Slack",
                    windowTitle: "chat",
                    occurredAt: Date(timeIntervalSince1970: 182),
                    durationSeconds: 81
                ),
                RawEvent(
                    appName: "Editor",
                    windowTitle: "code",
                    occurredAt: Date(timeIntervalSince1970: 263),
                    durationSeconds: 1_232
                ),
                RawEvent(
                    appName: "Editor",
                    windowTitle: "code",
                    occurredAt: Date(timeIntervalSince1970: 1_495),
                    durationSeconds: 103
                ),
            ]
        )

        // Conservation: the two Editor spans abut at 1495 and do not overlap,
        // so the split adds no seconds and loses none.
        let editorSeconds = sink.events
            .filter { $0.appName == "Editor" }
            .map(\.durationSeconds)
            .reduce(0, +)
        XCTAssertEqual(editorSeconds, 182 + 1_335)
        XCTAssertEqual(sink.events.map(\.durationSeconds).reduce(0, +), 1_598)
    }

    func testFlushIsIdempotentAndEmitsNothingWhenNothingHasBeenMeasured() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "code"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 0),
            // Consumed by `deinit`'s `stop()` once the agent goes out of scope.
            Date(timeIntervalSince1970: 900),
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))

        // A flush at the dwell's own anchor has measured nothing.
        XCTAssertFalse(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 0)))
        XCTAssertTrue(sink.events.isEmpty)

        XCTAssertTrue(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 600)))
        // A repeat at the same instant re-measures zero and stays silent, so a
        // duplicated trigger cannot inflate the ledger.
        XCTAssertFalse(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 600)))

        XCTAssertEqual(sink.events.count, 1)
        XCTAssertEqual(sink.events[0].durationSeconds, 600)
    }

    func testFlushEmitsNothingBeforeStartOrAfterStop() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "code"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 0),
            Date(timeIntervalSince1970: 100),
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        XCTAssertFalse(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 50)))

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        agent.stop()

        XCTAssertFalse(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 999)))
        XCTAssertEqual(sink.events.count, 1, "Only stop()'s own flush.")
        XCTAssertEqual(sink.events[0].durationSeconds, 100)
    }

    func testFlushPreservesActivityIdentitySoTheDwellIsNotSplitAgain() throws {
        let sink = RecordingEventSink()
        let workspace = FakeWorkspaceObserver()
        let accessibility = FakeAccessibilityObserver()
        accessibility.initialTitles = [10: "code"]
        let dates = DateQueue([
            Date(timeIntervalSince1970: 0),
            Date(timeIntervalSince1970: 700),
            // Consumed by `deinit`'s `stop()` once the agent goes out of scope.
            Date(timeIntervalSince1970: 800),
        ])
        let agent = makeAgent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            now: dates.next
        )

        try agent.start()
        workspace.activate(.init(processIdentifier: 10, appName: "Editor"))
        XCTAssertTrue(agent.flushPendingDwell(at: Date(timeIntervalSince1970: 600)))
        // A repeat notification for the unchanged activity must still be
        // recognised as a continuation of the re-opened dwell, not a switch.
        accessibility.emitTitle("code")

        XCTAssertEqual(sink.events.count, 1)
        XCTAssertEqual(sink.events[0].occurredAt, Date(timeIntervalSince1970: 0))
        XCTAssertEqual(sink.events[0].durationSeconds, 600)
    }

    private func makeAgent(
        sink: RecordingEventSink = RecordingEventSink(),
        permission: FakePermissionChecker = FakePermissionChecker(isTrusted: true),
        workspace: FakeWorkspaceObserver = FakeWorkspaceObserver(),
        accessibility: FakeAccessibilityObserver = FakeAccessibilityObserver(),
        metadata: any DeclaredAppMetadataReading = StubMetadataProvider([:]),
        now: @escaping () -> Date = Date.init,
        reportStatusTransition: @escaping (CollectionStatus, CollectionStatus) -> Void = { _, _ in }
    ) -> AXCollectionAgent {
        AXCollectionAgent(
            eventSink: sink,
            permissionChecker: permission,
            workspaceObserver: workspace,
            accessibilityObserver: accessibility,
            metadataProvider: metadata,
            now: now,
            reportStatusTransition: reportStatusTransition
        )
    }
}

private struct StatusTransition: Equatable {
    let from: CollectionStatus
    let to: CollectionStatus
}

private final class RecordingEventSink: EventSink {
    enum Entry: Equatable {
        case closed(RawEvent)
        case began(RawEvent)
    }

    /// Closed dwells only, which is all a ledger sink ever sees.
    private(set) var events: [RawEvent] = []
    /// Everything, in the order the agent handed it over.
    private(set) var journal: [Entry] = []

    func receive(_ event: RawEvent) {
        events.append(event)
        journal.append(.closed(event))
    }

    func activityBegan(_ event: RawEvent) {
        journal.append(.began(event))
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
    /// Applications registered without a window: in front, but with no
    /// focused or main window yet.
    var awaitingWindow: Set<pid_t> = []
    var startError: CollectionError?
    var startErrors: [pid_t: CollectionError] = [:]
    var onStart: ((pid_t) -> Void)?
    /// Runs once the observer is registered and its handlers are live, before
    /// `start` returns: the moment a real observer's first callback can land.
    var onRegistered: ((pid_t) -> Void)?
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
    ) throws -> AccessibilityRegistration {
        operations.append(.start(application.processIdentifier))
        startCallCount += 1
        onStart?(application.processIdentifier)
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
        onRegistered?(application.processIdentifier)
        if awaitingWindow.contains(application.processIdentifier) {
            return .awaitingWindow
        }
        return .window(
            FocusedActivity(
                windowTitle: initialTitles[application.processIdentifier],
                focusedDocumentURL: initialDocumentURLs[application.processIdentifier]
            ))
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

private final class StubMetadataProvider: DeclaredAppMetadataReading {
    private let declared: [String: DeclaredAppMetadata]

    init(_ declared: [String: DeclaredAppMetadata]) {
        self.declared = declared
    }

    func metadata(for application: RunningApplication) -> DeclaredAppMetadata {
        application.bundleIdentifier.flatMap { declared[$0] } ?? .absent
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
