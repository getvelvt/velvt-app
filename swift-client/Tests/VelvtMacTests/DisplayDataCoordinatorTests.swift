import AppKit
import Combine
import SwiftUI
import XCTest
@testable import VelvtMac

@MainActor
final class DisplayDataCoordinatorTests: XCTestCase {

    // MARK: - Initial state

    func testInitialDisplayStateIsLoading() {
        let sut = ConcreteDisplayDataCoordinator()
        var receivedLoading = false
        let cancel = sut.displayState.sink { state in
            if case .loading = state { receivedLoading = true }
        }
        XCTAssertTrue(receivedLoading, "displayState should start as .loading")
        _ = cancel
    }

    func testInitialStateIsNotError() {
        let sut = ConcreteDisplayDataCoordinator()
        var isError = false
        let cancel = sut.displayState.sink { state in
            if case .error = state { isError = true }
        }
        XCTAssertFalse(isError)
        _ = cancel
    }

    // MARK: - Direct updateInsight (no IPC — view models testable in isolation)

    func testDirectUpdateInsightTransitionsToPopulated() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateInsight(makeInsightPayload(text: "Context held all morning."))
        var isPopulated = false
        let cancel = sut.displayState.sink { state in
            if case .populated = state { isPopulated = true }
        }
        XCTAssertTrue(isPopulated)
        _ = cancel
    }

    func testDirectUpdateInsightUpdatesInsightViewModel() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateInsight(makeInsightPayload(text: "Sustained context throughout."))
        if case .populated(let vm, _) = sut.state {
            XCTAssertEqual(vm.text, "Sustained context throughout.")
            XCTAssertFalse(vm.isLoading)
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
    }

    func testDirectUpdateInsightLeavesHistoryLoading() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateInsight(makeInsightPayload())
        if case .populated(_, let historyVM) = sut.state {
            XCTAssertTrue(historyVM.isLoading,
                          "History should still be loading when only insight arrived")
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
    }

    // MARK: - Direct updateHistory (no IPC)

    func testDirectUpdateHistoryTransitionsToPopulated() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateHistory(makeHistoryPayload())
        var isPopulated = false
        let cancel = sut.displayState.sink { state in
            if case .populated = state { isPopulated = true }
        }
        XCTAssertTrue(isPopulated)
        _ = cancel
    }

    func testDirectUpdateHistoryUpdatesHistoryViewModel() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateHistory(makeHistoryPayload(dayCount: 3))
        if case .populated(_, let vm) = sut.state {
            XCTAssertEqual(vm.days.count, 3)
            XCTAssertFalse(vm.isLoading)
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
    }

    func testDirectUpdateHistoryLeavesInsightLoading() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateHistory(makeHistoryPayload())
        if case .populated(let insightVM, _) = sut.state {
            XCTAssertTrue(insightVM.isLoading,
                          "Insight should still be loading when only history arrived")
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
    }

    func testEmptyInsightTransitionsToPopulatedWithNotGeneratedAvailability() {
        let sut = ConcreteDisplayDataCoordinator()

        sut.handleCacheEmpty(CacheEmpty(payloadType: "insight_payload"))

        XCTAssertEqual(sut.insightAvailability, .notGenerated)
        if case .populated = sut.state {} else {
            XCTFail("Expected cache-empty insight response to finish loading")
        }
    }

    func testEmptyHistoryTransitionsToPopulatedWithNotGeneratedAvailability() {
        let sut = ConcreteDisplayDataCoordinator()

        sut.handleCacheEmpty(CacheEmpty(payloadType: "history_payload"))

        XCTAssertEqual(sut.historyAvailability, .notGenerated)
        if case .populated = sut.state {} else {
            XCTFail("Expected cache-empty history response to finish loading")
        }
    }

    func testCurrentLocalDateStringUsesRequestedTimeZone() throws {
        let formatter = ISO8601DateFormatter()
        let now = try XCTUnwrap(formatter.date(from: "2026-06-27T02:00:00Z"))

        XCTAssertEqual(
            MenuBarDataLoader.currentLocalDateString(
                now: now,
                timeZone: TimeZone(secondsFromGMT: 0)!
            ),
            "2026-06-27"
        )
    }

    func testCurrentUTCDateStringDoesNotAdvanceAtLocalMidnightBeforeUTCMidnight() throws {
        let formatter = ISO8601DateFormatter()
        let now = try XCTUnwrap(formatter.date(from: "2026-06-26T22:00:00Z"))

        XCTAssertEqual(MenuBarDataLoader.currentUTCDateString(now: now), "2026-06-26")
        XCTAssertEqual(
            MenuBarDataLoader.currentLocalDateString(
                now: now,
                timeZone: TimeZone(secondsFromGMT: 4 * 3600)!
            ),
            "2026-06-27"
        )
    }

    func testDataLoaderRequestsCurrentLocalInsightDateWhenConnectedAndLoggedIn() async {
        let client = FakeIPCClient()
        client.setConnectionStatus(.connected)
        let account = CurrentValueSubject<AccountState, Never>(.loggedIn(userId: "user-1"))
        let sut = MenuBarDataLoader(
            ipcClient: client,
            currentLocalInsightDate: { "2026-06-26" }
        )

        sut.start(accountState: account.eraseToAnyPublisher())

        let messagesSent = expectation(description: "loader sent startup delivery requests")
        Task {
            for _ in 0 ..< 100 {
                if client.sentMessages.count >= 2 {
                    messagesSent.fulfill()
                    return
                }
                try? await Task.sleep(nanoseconds: 10_000_000)
            }
        }
        await fulfillment(of: [messagesSent], timeout: 2)

        guard case .requestLatestInsight(let request)? = client.sentMessages.first else {
            return XCTFail("Expected first startup delivery request to fetch latest insight")
        }
        XCTAssertEqual(request.date, "2026-06-26")
        XCTAssertTrue(client.sentMessages.contains(.requestLatestHistory(RequestLatestHistory(days: 14))))
    }

    func testDataLoaderRetriesStartupDeliveryRequestsAfterSendFailure() async {
        let client = FakeIPCClient()
        client.shouldThrowOnSend = IPCError.notConnected
        client.setConnectionStatus(.connected)
        let account = CurrentValueSubject<AccountState, Never>(.loggedIn(userId: "user-1"))
        let sut = MenuBarDataLoader(
            ipcClient: client,
            currentLocalInsightDate: { "2026-06-26" },
            retryDelayNanoseconds: 10_000_000
        )

        sut.start(accountState: account.eraseToAnyPublisher())

        try? await Task.sleep(nanoseconds: 30_000_000)
        XCTAssertTrue(client.sentMessages.isEmpty)

        client.shouldThrowOnSend = nil

        let retried = expectation(description: "loader retried startup delivery requests")
        Task {
            for _ in 0 ..< 100 {
                if client.sentMessages.count >= 2 {
                    retried.fulfill()
                    return
                }
                try? await Task.sleep(nanoseconds: 10_000_000)
            }
        }
        await fulfillment(of: [retried], timeout: 2)

        guard case .requestLatestInsight(let request)? = client.sentMessages.first else {
            return XCTFail("Expected first retried startup delivery request to fetch latest insight")
        }
        XCTAssertEqual(request.date, "2026-06-26")
        XCTAssertTrue(client.sentMessages.contains(.requestLatestHistory(RequestLatestHistory(days: 14))))
    }

    func testPopulatedStateHoldsNoDataDayCorrectly() {
        let sut = ConcreteDisplayDataCoordinator()
        let payload = HistoryPayload(days: 1, summaries: [
            DailySummary(date: "2026-06-15", status: .noData, eventCount: 0,
                         focusScore: nil, fragmentationScore: nil,
                         confidenceLevel: .low, activeSeconds: 0)
        ])
        sut.updateHistory(payload)
        if case .populated(_, let vm) = sut.state {
            XCTAssertEqual(vm.days.count, 1)
            XCTAssertTrue(vm.days[0].isNoData)
            XCTAssertNil(vm.days[0].focusScore)
            XCTAssertEqual(vm.days[0].activeTime, "—")
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
    }

    // MARK: - Subsequent updates in populated state

    func testSecondInsightUpdateDoesNotChangeStatePhase() {
        let sut = ConcreteDisplayDataCoordinator()
        sut.updateInsight(makeInsightPayload(text: "First."))
        sut.updateInsight(makeInsightPayload(text: "Second."))
        if case .populated(let vm, _) = sut.state {
            XCTAssertEqual(vm.text, "Second.",
                           "View model should reflect the latest push")
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
    }

    // MARK: - IPC push routing via serverMessages

    func testInsightPayloadPushTransitionsToPopulated() async {
        let (sut, client, manager) = makeWiredCoordinator()

        let expectPopulated = expectation(description: "populated")
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { state in
            if case .populated = state { expectPopulated.fulfill(); cancellable?.cancel() }
        }

        client.inject(.insightPayload(makeInsightPayload(text: "Focus held steady.")))

        await fulfillment(of: [expectPopulated], timeout: 1)
        if case .populated(let vm, _) = sut.state {
            XCTAssertEqual(vm.text, "Focus held steady.")
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
        _ = manager
    }

    func testHistoryPayloadPushTransitionsToPopulated() async {
        let (sut, client, manager) = makeWiredCoordinator()

        let expectPopulated = expectation(description: "populated")
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { state in
            if case .populated = state { expectPopulated.fulfill(); cancellable?.cancel() }
        }

        client.inject(.historyPayload(makeHistoryPayload(dayCount: 7)))

        await fulfillment(of: [expectPopulated], timeout: 1)
        if case .populated(_, let vm) = sut.state {
            XCTAssertEqual(vm.days.count, 7)
        } else {
            XCTFail("Expected .populated; got \(describeState(sut.state))")
        }
        _ = manager
    }

    func testUnrelatedServerMessageIsIgnored() async {
        // Manager must stay alive so the message actually reaches the coordinator
        // via serverMessages. Verifies the coordinator's default-break is exercised.
        let (sut, client, manager) = makeWiredCoordinator()

        let noStateChange = expectation(description: "no state change")
        noStateChange.isInverted = true
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { _ in
            noStateChange.fulfill()
            cancellable?.cancel()
        }

        client.inject(.acknowledged(Acknowledged()))
        await fulfillment(of: [noStateChange], timeout: 0.5)

        if case .loading = sut.state {} else {
            XCTFail("State should remain .loading after unrelated message; got \(describeState(sut.state))")
        }
        _ = manager
    }

    // MARK: - Connection status → error state

    func testDisconnectWhileLoadingTransitionsToError() async {
        let (sut, client, _) = makeWiredCoordinator()

        // Simulate a real connection before disconnecting.
        client.setConnectionStatus(.connected)

        let expectError = expectation(description: "error")
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { state in
            if case .error = state { expectError.fulfill(); cancellable?.cancel() }
        }

        client.setConnectionStatus(.disconnected)
        await fulfillment(of: [expectError], timeout: 1)

        if case .error(let msg) = sut.state {
            XCTAssertFalse(msg.isEmpty)
        } else {
            XCTFail("Expected .error; got \(describeState(sut.state))")
        }
    }

    func testInitialDisconnectedStatusIsIgnored() async {
        // FakeIPCClient starts as .disconnected. The coordinator must not treat the
        // pre-connection state as an error — it should remain .loading.
        let (sut, _, _) = makeWiredCoordinator()

        // Give Combine time to deliver the initial CurrentValueSubject value.
        let noError = expectation(description: "no error from initial disconnect")
        noError.isInverted = true
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { state in
            if case .error = state { noError.fulfill(); cancellable?.cancel() }
        }
        await fulfillment(of: [noError], timeout: 0.3)

        if case .loading = sut.state {} else {
            XCTFail("Initial disconnected status should not produce .error; got \(describeState(sut.state))")
        }
    }

    func testReconnectingWhileLoadingTransitionsToError() async {
        let (sut, client, _) = makeWiredCoordinator()

        client.setConnectionStatus(.connected)

        let expectError = expectation(description: "error")
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { state in
            if case .error = state { expectError.fulfill(); cancellable?.cancel() }
        }

        client.setConnectionStatus(.reconnecting(attempt: 1, nextRetryIn: 2))
        await fulfillment(of: [expectError], timeout: 1)

        if case .error = sut.state {} else {
            XCTFail("Expected .error; got \(describeState(sut.state))")
        }
    }

    func testDisconnectAfterPopulatedDoesNotClobberData() async {
        // manager must stay alive — insightPayload is routed through serverMessages.
        let (sut, client, manager) = makeWiredCoordinator()

        // Drive to .populated.
        let expectPopulated = expectation(description: "populated")
        var c1: AnyCancellable?
        c1 = sut.displayState.dropFirst().sink { state in
            if case .populated = state { expectPopulated.fulfill(); c1?.cancel() }
        }
        client.inject(.insightPayload(makeInsightPayload()))
        await fulfillment(of: [expectPopulated], timeout: 1)

        // Now simulate connect → disconnect. The state guard prevents clobbering
        // .populated regardless of hasConnectedAtLeastOnce.
        let noErrorEmitted = expectation(description: "no error while populated")
        noErrorEmitted.isInverted = true
        var c2: AnyCancellable?
        c2 = sut.displayState.dropFirst().sink { state in
            if case .error = state { noErrorEmitted.fulfill(); c2?.cancel() }
        }

        client.setConnectionStatus(.connected)
        client.setConnectionStatus(.disconnected)
        await fulfillment(of: [noErrorEmitted], timeout: 0.5)

        if case .populated = sut.state {} else {
            XCTFail("Disconnect while populated should keep .populated state; got \(describeState(sut.state))")
        }
        _ = manager
    }

    func testReconnectFromErrorResetsToLoading() async {
        let (sut, client, _) = makeWiredCoordinator()

        // Drive to .error.
        client.setConnectionStatus(.connected)
        let expectError = expectation(description: "error")
        var c1: AnyCancellable?
        c1 = sut.displayState.dropFirst().sink { state in
            if case .error = state { expectError.fulfill(); c1?.cancel() }
        }
        client.setConnectionStatus(.disconnected)
        await fulfillment(of: [expectError], timeout: 1)

        // Reconnect → .loading.
        let expectLoading = expectation(description: "loading after reconnect")
        var c2: AnyCancellable?
        c2 = sut.displayState.dropFirst().sink { state in
            if case .loading = state { expectLoading.fulfill(); c2?.cancel() }
        }
        client.setConnectionStatus(.connected)
        await fulfillment(of: [expectLoading], timeout: 1)

        if case .loading = sut.state {} else {
            XCTFail("Expected .loading after reconnect; got \(describeState(sut.state))")
        }
    }

    // MARK: - Skeleton: no data before first push

    func testInsightViewModelIsLoadingBeforeFirstPush() {
        let (sut, _, _) = makeWiredCoordinator()
        XCTAssertTrue(sut.insightViewModel.isLoading)
        XCTAssertTrue(sut.historyViewModel.isLoading)
        if case .loading = sut.state {} else {
            XCTFail("Expected overall .loading state before any push")
        }
    }

    // MARK: - Error state provides a muted message (not an alert)

    func testErrorStateHasNonEmptyMessage() async {
        let (sut, client, _) = makeWiredCoordinator()
        client.setConnectionStatus(.connected)

        let expectError = expectation(description: "error")
        var cancellable: AnyCancellable?
        cancellable = sut.displayState.dropFirst().sink { state in
            if case .error = state { expectError.fulfill(); cancellable?.cancel() }
        }
        client.setConnectionStatus(.disconnected)
        await fulfillment(of: [expectError], timeout: 1)

        if case .error(let msg) = sut.state {
            XCTAssertFalse(msg.isEmpty, "Error message must be non-empty for IPCStatusBanner")
        } else {
            XCTFail("Expected .error")
        }
    }

    // MARK: - Rapid successive IPC pushes

    func testTwoRapidInsightPushesViaIPCShowOnlyLatest() async {
        // manager must stay alive for serverMessages fan-out to work.
        let (sut, client, manager) = makeWiredCoordinator()

        let expectSecondText = expectation(description: "second insight text reflected")
        var cancellable: AnyCancellable?
        cancellable = sut.insightViewModel.$text.dropFirst().sink { text in
            if text == "Second rapid insight." {
                expectSecondText.fulfill()
                cancellable?.cancel()
            }
        }

        client.inject(.insightPayload(makeInsightPayload(text: "First rapid insight.")))
        client.inject(.insightPayload(makeInsightPayload(text: "Second rapid insight.")))

        await fulfillment(of: [expectSecondText], timeout: 1)
        XCTAssertEqual(sut.insightViewModel.text, "Second rapid insight.",
                       "Latest push must win; first must not persist")
        _ = manager
    }

    // MARK: - Full state-transition cycle: loading → error → loading → populated

    func testFullReconnectCycleEndsInPopulatedState() async {
        // loading → .connected → .disconnected (error) → .connected (loading) → insight push (populated)
        let (sut, client, manager) = makeWiredCoordinator()

        // Drive loading → error.
        client.setConnectionStatus(.connected)
        let expectError = expectation(description: "error after disconnect")
        var c1: AnyCancellable?
        c1 = sut.displayState.dropFirst().sink { state in
            if case .error = state { expectError.fulfill(); c1?.cancel() }
        }
        client.setConnectionStatus(.disconnected)
        await fulfillment(of: [expectError], timeout: 1)

        // Reconnect → back to loading.
        let expectLoading = expectation(description: "loading after reconnect")
        var c2: AnyCancellable?
        c2 = sut.displayState.dropFirst().sink { state in
            if case .loading = state { expectLoading.fulfill(); c2?.cancel() }
        }
        client.setConnectionStatus(.connected)
        await fulfillment(of: [expectLoading], timeout: 1)

        // Push insight → populated. Verifies no state inconsistency across full cycle.
        let expectPopulated = expectation(description: "populated after reconnect push")
        var c3: AnyCancellable?
        c3 = sut.displayState.dropFirst().sink { state in
            if case .populated = state { expectPopulated.fulfill(); c3?.cancel() }
        }
        client.inject(.insightPayload(makeInsightPayload(text: "Post-reconnect insight.")))
        await fulfillment(of: [expectPopulated], timeout: 1)

        if case .populated(let vm, _) = sut.state {
            XCTAssertEqual(vm.text, "Post-reconnect insight.",
                           "Insight must reflect data pushed after reconnect cycle")
        } else {
            XCTFail("Expected .populated after full reconnect cycle; got \(describeState(sut.state))")
        }
        _ = manager
    }

    // MARK: - View model identity stability

    func testViewModelInstancesAreStableAcrossUpdates() {
        let sut = ConcreteDisplayDataCoordinator()
        let insightBefore = sut.insightViewModel
        let historyBefore = sut.historyViewModel

        sut.updateInsight(makeInsightPayload())
        sut.updateHistory(makeHistoryPayload())
        sut.updateInsight(makeInsightPayload())

        // Same object identity — SwiftUI @ObservedObject bindings remain valid across pushes.
        XCTAssertTrue(sut.insightViewModel === insightBefore)
        XCTAssertTrue(sut.historyViewModel === historyBefore)
    }

    func testLoggedOutAccountStateClearsVisibleDisplayData() async {
        let client = FakeIPCClient()
        let account = CurrentValueSubject<AccountState, Never>(.loggedIn(userId: "user-1"))
        let sut = ConcreteDisplayDataCoordinator()
        sut.start(
            serverMessages: Empty<ServerMessage, Never>().eraseToAnyPublisher(),
            connectionStatus: client.connectionStatus,
            accountState: account.eraseToAnyPublisher()
        )
        sut.updateInsight(makeInsightPayload(text: "Private insight."))
        sut.updateHistory(makeHistoryPayload(dayCount: 2))

        account.send(.loggedOut)

        let reset = expectation(description: "display data reset")
        Task {
            for _ in 0 ..< 100 {
                if sut.insightViewModel.isLoading,
                   sut.historyViewModel.isLoading,
                   sut.insightViewModel.text.isEmpty,
                   sut.historyViewModel.days.isEmpty {
                    reset.fulfill()
                    return
                }
                try? await Task.sleep(nanoseconds: 10_000_000)
            }
        }
        await fulfillment(of: [reset], timeout: 2)

        XCTAssertEqual(sut.insightAvailability, .loading)
        XCTAssertEqual(sut.historyAvailability, .loading)
        if case .loading = sut.state {} else {
            XCTFail("Expected display state to reset to .loading; got \(describeState(sut.state))")
        }
    }

    // MARK: - Initial data request loader

    func testMenuBarDataLoaderRetriesInitialRequestsAfterSendFailureOnReconnect() async throws {
        let client = InitialDataRequestIPCClient()
        client.sendFailuresRemaining = 1
        let accountState = CurrentValueSubject<AccountState, Never>(.loggedIn(userId: "u1"))
        let sut = MenuBarDataLoader(
            ipcClient: client,
            currentLocalInsightDate: { "2026-07-03" }
        )
        sut.start(accountState: accountState.eraseToAnyPublisher())

        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(client.sendAttempts, 1)
        XCTAssertTrue(client.sentMessages.isEmpty)

        client.setConnectionStatus(.disconnected)
        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(client.sentMessages, [
            .requestLatestInsight(RequestLatestInsight(date: "2026-07-03")),
            .requestLatestHistory(RequestLatestHistory(days: 14)),
        ])
    }

    func testMenuBarDataLoaderDoesNotRepeatInitialRequestsAfterSuccessfulSend() async throws {
        let client = InitialDataRequestIPCClient()
        let accountState = CurrentValueSubject<AccountState, Never>(.loggedIn(userId: "u1"))
        let sut = MenuBarDataLoader(
            ipcClient: client,
            currentLocalInsightDate: { "2026-07-03" }
        )
        sut.start(accountState: accountState.eraseToAnyPublisher())

        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)
        client.setConnectionStatus(.connected)
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(client.sentMessages, [
            .requestLatestInsight(RequestLatestInsight(date: "2026-07-03")),
            .requestLatestHistory(RequestLatestHistory(days: 14)),
        ])
    }

    // MARK: - Helpers

    private func makeWiredCoordinator() -> (
        ConcreteDisplayDataCoordinator,
        FakeIPCClient,
        AccountStateManager
    ) {
        let client = FakeIPCClient()
        let manager = AccountStateManager(keychain: FakeKeychain())
        manager.startListening(to: client)
        let sut = ConcreteDisplayDataCoordinator()
        sut.start(
            serverMessages: manager.serverMessages,
            connectionStatus: client.connectionStatus
        )
        return (sut, client, manager)
    }

    private func makeInsightPayload(text: String = "Default insight.") -> InsightPayload {
        InsightPayload(
            date: "2026-06-15",
            text: text,
            confidenceLevel: .high,
            lowConfidence: false,
            generatedAt: Date(timeIntervalSince1970: 1_750_000_000)
        )
    }

    private func makeHistoryPayload(dayCount: Int = 7) -> HistoryPayload {
        let summaries = (0 ..< dayCount).map { i -> DailySummary in
            let dateStr = "2026-06-\(String(format: "%02d", 9 + i))"
            return DailySummary(date: dateStr, status: .ready, eventCount: 30,
                                focusScore: 68.0, fragmentationScore: 22.0,
                                confidenceLevel: .medium, activeSeconds: 5400)
        }
        return HistoryPayload(days: dayCount, summaries: summaries)
    }

    private func describeState(_ state: DisplayState) -> String {
        switch state {
        case .loading:        return ".loading"
        case .populated:      return ".populated"
        case .error(let msg): return ".error(\(msg))"
        }
    }
}

private final class InitialDataRequestIPCClient: IPCClientProtocol, @unchecked Sendable {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { continuation in
        continuation.finish()
    }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }
    var sentMessages: [ClientMessage] {
        lock.withLock { messages }
    }
    var sendAttempts: Int {
        lock.withLock { attempts }
    }
    var sendFailuresRemaining = 0

    private let lock = NSLock()
    private let statusSubject = CurrentValueSubject<ConnectionStatus, Never>(.disconnected)
    private var messages: [ClientMessage] = []
    private var attempts = 0

    func connect() async throws {
        statusSubject.send(.connected)
    }

    func disconnect() {
        statusSubject.send(.disconnected)
    }

    func send(_ message: ClientMessage) async throws {
        try lock.withLock {
            attempts += 1
            if sendFailuresRemaining > 0 {
                sendFailuresRemaining -= 1
                throw IPCError.notConnected
            }
            messages.append(message)
        }
    }

    func setConnectionStatus(_ status: ConnectionStatus) {
        statusSubject.send(status)
    }
}

// MARK: - Work-block evidence timeline layout

/// Geometry for the work-block evidence timeline, tested against the shapes
/// real collection produces rather than the evenly-spread shape the surface
/// was designed for.
///
/// The case that produced the bug report: a 25-minute block whose collection
/// died three minutes in, leaving five transitions inside the first ~1% of a
/// 1500-second window and nothing after. Every one of those five mapped to
/// within a couple of points of x=0 and stacked, and the switching-cluster
/// glyph's hard-coded -6 centring offset put it outside the track's leading
/// edge on top of the pile.
@MainActor
final class TimelineMarkerLayoutTests: XCTestCase {
  private let windowStart = Date(timeIntervalSince1970: 1_800_000_000)
  /// The reported block: 25 minutes planned.
  private var windowEnd: Date { windowStart.addingTimeInterval(1_500) }
  /// Roughly the evidence card's inner width inside the 600pt popover.
  private let trackWidth: CGFloat = 300

  // MARK: Degenerate case 1 — zero transitions

  func testZeroTransitionsProducesNoTicks() {
    let layout = makeLayout(transitions: [], clusters: [])
    XCTAssertTrue(layout.ticks.isEmpty)
    XCTAssertTrue(layout.clusterRails.isEmpty)
  }

  func testZeroWidthTrackProducesEmptyLayout() {
    let layout = TimelineMarkerLayout.make(
      transitions: [transition("a", at: 10)],
      clusters: [cluster("c", from: 0, to: 60, count: 3)],
      windowStartedAt: windowStart,
      windowEndedAt: windowEnd,
      width: 0
    )
    XCTAssertTrue(layout.ticks.isEmpty, "A zero-width track cannot place a mark anywhere honest")
    XCTAssertTrue(layout.clusterRails.isEmpty)
  }

  // MARK: Degenerate case 2 — exactly one transition

  func testSingleTransitionProducesOneUncollapsedTick() {
    let layout = makeLayout(transitions: [transition("a", at: 750)], clusters: [])
    XCTAssertEqual(layout.ticks.count, 1)
    XCTAssertFalse(layout.ticks[0].isCollapsed)
    XCTAssertEqual(layout.ticks[0].transitionCount, 1)
    // Halfway through the window is halfway along the track, centred.
    XCTAssertEqual(layout.ticks[0].center, 150, accuracy: 0.01)
  }

  // MARK: Degenerate case 3 — the reported bug

  func testFiveTransitionsInsideOnePercentOfWindowCollapseIntoOneTick() {
    // Five switches in the first 15 seconds of 1500 — 1% of the window, and
    // 3pt of a 300pt track. Every one of these used to draw a separate 2pt
    // bar within 3pt of x=0.
    let dense = [
      transition("s1", at: 0),
      transition("s2", at: 3),
      transition("s3", at: 6),
      transition("s4", at: 11),
      transition("s5", at: 15),
    ]
    let layout = makeLayout(transitions: dense, clusters: [])
    XCTAssertEqual(layout.ticks.count, 1, "Five marks inside 3pt must not draw as five marks")
    XCTAssertTrue(layout.ticks[0].isCollapsed)
    XCTAssertEqual(layout.ticks[0].transitionCount, 5)
    XCTAssertEqual(layout.ticks[0].transitionIDs, ["s1", "s2", "s3", "s4", "s5"])
  }

  func testCollapsedTickLabelStatesHowManyTransitionsItStandsFor() {
    let dense = (0..<5).map { transition("s\($0)", at: Double($0) * 3) }
    let layout = makeLayout(transitions: dense, clusters: [])
    let label = tickEvidenceLabel(
      layout.ticks[0], transitions: dense, windowStartedAt: windowStart)
    XCTAssertTrue(
      label.hasPrefix("5 observed category switches"),
      "A collapsed mark that says '1 transition' is a lie; got: \(label)")
  }

  func testSingleTickLabelIsTheOrdinaryTransitionSentence() {
    let only = [transition("s1", at: 600)]
    let layout = makeLayout(transitions: only, clusters: [])
    XCTAssertEqual(
      tickEvidenceLabel(layout.ticks[0], transitions: only, windowStartedAt: windowStart),
      transitionEvidenceLabel(only[0]))
  }

  func testDenseTransitionsAreNeverSilentlyDropped() {
    let dense = (0..<5).map { transition("s\($0)", at: Double($0) * 3) }
    let layout = makeLayout(transitions: dense, clusters: [])
    let represented = layout.ticks.flatMap(\.transitionIDs)
    XCTAssertEqual(Set(represented), Set(dense.map(\.id)))
    XCTAssertEqual(represented.count, dense.count, "No transition may be represented twice")
  }

  // MARK: Degenerate case 4 — transitions spanning the full width

  func testWellSpreadTransitionsEachKeepTheirOwnTick() {
    let spread = (0..<5).map { transition("s\($0)", at: Double($0) * 300) }
    let layout = makeLayout(transitions: spread, clusters: [])
    XCTAssertEqual(layout.ticks.count, 5)
    XCTAssertTrue(layout.ticks.allSatisfy { !$0.isCollapsed })
  }

  func testSpreadTicksAreOrderedAndNonOverlapping() {
    let spread = (0..<5).map { transition("s\($0)", at: Double($0) * 300) }
    let layout = makeLayout(transitions: spread, clusters: [])
    for (previous, next) in zip(layout.ticks, layout.ticks.dropFirst()) {
      XCTAssertGreaterThanOrEqual(
        next.offset - previous.offset,
        TimelineMarkerLayout.tickGlyphWidth,
        "Adjacent ticks must not share pixels")
    }
  }

  // MARK: Degenerate case 5 — the clamp's boundary conditions

  func testTransitionAtWindowStartStaysInsideTheTrack() {
    let layout = makeLayout(transitions: [transition("start", at: 0)], clusters: [])
    XCTAssertGreaterThanOrEqual(layout.ticks[0].offset, 0)
  }

  func testTransitionAtWindowEndStaysInsideTheTrack() {
    let layout = makeLayout(transitions: [transition("end", at: 1_500)], clusters: [])
    XCTAssertLessThanOrEqual(
      layout.ticks[0].offset + TimelineMarkerLayout.tickGlyphWidth, trackWidth)
  }

  func testTransitionsAtBothBoundariesBothStayInsideTheTrack() {
    let layout = makeLayout(
      transitions: [transition("start", at: 0), transition("end", at: 1_500)], clusters: [])
    XCTAssertEqual(layout.ticks.count, 2)
    for tick in layout.ticks {
      XCTAssertGreaterThanOrEqual(tick.offset, 0)
      XCTAssertLessThanOrEqual(tick.offset + TimelineMarkerLayout.tickGlyphWidth, trackWidth)
    }
  }

  func testTimestampsOutsideTheWindowAreStillClampedIntoTheTrack() {
    let layout = makeLayout(
      transitions: [transition("before", at: -600), transition("after", at: 9_000)], clusters: [])
    for tick in layout.ticks {
      XCTAssertGreaterThanOrEqual(tick.offset, 0)
      XCTAssertLessThanOrEqual(tick.offset + TimelineMarkerLayout.tickGlyphWidth, trackWidth)
    }
  }

  // MARK: Cluster rails

  func testClusterStartingAtWindowStartDoesNotRenderOutsideTheLeadingEdge() {
    // The reported symptom: the cluster glyph appeared to sit outside the
    // bar's left edge, because its offset was `position - 6` with no clamp.
    let layout = makeLayout(
      transitions: [], clusters: [cluster("c", from: 0, to: 180, count: 5)])
    XCTAssertEqual(layout.clusterRails.count, 1)
    XCTAssertGreaterThanOrEqual(layout.clusterRails[0].offset, 0)
  }

  func testClusterEndingAtWindowEndStaysInsideTheTrailingEdge() {
    let layout = makeLayout(
      transitions: [], clusters: [cluster("c", from: 1_440, to: 1_500, count: 3)])
    let rail = layout.clusterRails[0]
    XCTAssertLessThanOrEqual(rail.offset + rail.width, trackWidth)
  }

  func testClusterRailSpansItsOwnDurationRatherThanASinglePoint() {
    // 300 of 1500 seconds is a fifth of a 300pt track.
    let layout = makeLayout(
      transitions: [], clusters: [cluster("c", from: 300, to: 600, count: 4)])
    XCTAssertEqual(layout.clusterRails[0].width, 60, accuracy: 0.01)
    XCTAssertEqual(layout.clusterRails[0].offset, 60, accuracy: 0.01)
  }

  func testZeroDurationClusterStillGetsAVisibleRail() {
    let layout = makeLayout(
      transitions: [], clusters: [cluster("c", from: 750, to: 750, count: 3)])
    XCTAssertEqual(
      layout.clusterRails[0].width, TimelineMarkerLayout.minimumClusterRailWidth, accuracy: 0.01)
  }

  func testClusterLaneSitsBelowTheBarSoItCannotCoverATick() {
    XCTAssertGreaterThanOrEqual(
      TimelineMarkerLayout.clusterLaneTop, TimelineMarkerLayout.tickHeight)
    XCTAssertLessThan(
      TimelineMarkerLayout.clusterRailHeight, TimelineMarkerLayout.tickHeight,
      "The cluster mark must be subordinate to the ticks it is made of")
  }

  func testClusterLabelStatesItsTransitionCount() {
    let label = clusterEvidenceLabel(cluster("c", from: 0, to: 180, count: 5))
    XCTAssertTrue(label.contains("5 transitions"), label)
  }

  // MARK: Collapse invariants

  func testACollapsedRunNeverStandsForMoreOfTheTimelineThanOneTickWidth() {
    // Evenly dense across the whole window: 200 transitions over 25 minutes.
    // Chaining on gap-to-previous would fold all of them into one mark
    // spanning the bar. Closing runs on extent cannot.
    let many = (0..<200).map { transition("s\($0)", at: Double($0) * 7.5) }
    let layout = makeLayout(transitions: many, clusters: [])
    XCTAssertGreaterThan(layout.ticks.count, 1)
    for tick in layout.ticks {
      let members = tick.transitionIDs.compactMap { id in many.first { $0.id == id } }
      let first = TimelineMarkerLayout.position(
        members[0].occurredAt, windowStartedAt: windowStart, windowEndedAt: windowEnd,
        width: trackWidth)
      let last = TimelineMarkerLayout.position(
        members[members.count - 1].occurredAt, windowStartedAt: windowStart,
        windowEndedAt: windowEnd, width: trackWidth)
      XCTAssertLessThan(last - first, TimelineMarkerLayout.minimumTickSpacing)
    }
    XCTAssertEqual(layout.ticks.flatMap(\.transitionIDs).count, many.count)
  }

  func testEveryTickIsInsideTheTrackForEveryDegenerateShape() {
    let shapes: [(String, [LocalTransitionMarker])] = [
      ("clustered at origin", (0..<5).map { transition("s\($0)", at: Double($0) * 3) }),
      ("clustered at end", (0..<5).map { transition("s\($0)", at: 1_500 - Double($0) * 3) }),
      ("both boundaries", [transition("a", at: 0), transition("b", at: 1_500)]),
      ("evenly dense", (0..<120).map { transition("s\($0)", at: Double($0) * 12.5) }),
    ]
    for (name, transitions) in shapes {
      for width in [CGFloat(120), 300, 536] {
        let layout = TimelineMarkerLayout.make(
          transitions: transitions, clusters: [], windowStartedAt: windowStart,
          windowEndedAt: windowEnd, width: width)
        for tick in layout.ticks {
          XCTAssertGreaterThanOrEqual(tick.offset, 0, "\(name) at \(width)")
          XCTAssertLessThanOrEqual(
            tick.offset + TimelineMarkerLayout.tickGlyphWidth, width, "\(name) at \(width)")
        }
      }
    }
  }

  // MARK: Segment bars

  func testShortAdjacentSegmentsAreNotWidenedOverEachOther() {
    // Twelve ~13-second segments across the first three minutes of a
    // 25-minute window: each is under a point wide on a 300pt track, and the
    // old `max(5, ...)` floor drew every one of them five points wide over
    // the top of the next.
    let segments = (0..<12).map { segment("g\($0)", Double($0) * 13, 13, "FOCUS_WORK") }
    let bars = TimelineMarkerLayout.segmentBars(
      segments, windowStartedAt: windowStart, windowEndedAt: windowEnd, width: trackWidth)
    for (previous, next) in zip(bars, bars.dropFirst()) {
      XCTAssertLessThanOrEqual(
        previous.offset + previous.width, next.offset + 0.001,
        "A segment must not draw over the next segment's start")
    }
  }

  func testAnIsolatedShortSegmentStillGetsItsVisibleMinimumWidth() {
    let bars = TimelineMarkerLayout.segmentBars(
      [segment("only", 0, 4, "FOCUS_WORK")],
      windowStartedAt: windowStart, windowEndedAt: windowEnd, width: trackWidth)
    XCTAssertEqual(bars[0].width, TimelineMarkerLayout.minimumSegmentWidth, accuracy: 0.001)
  }

  func testSegmentsNeverDrawPastTheTrailingEdge() {
    // A segment whose recorded end runs past the window end.
    let bars = TimelineMarkerLayout.segmentBars(
      [segment("over", 1_200, 900, "FOCUS_WORK")],
      windowStartedAt: windowStart, windowEndedAt: windowEnd, width: trackWidth)
    XCTAssertLessThanOrEqual(bars[0].offset + bars[0].width, trackWidth + 0.001)
  }

  func testFullWindowSegmentFillsExactlyTheTrack() {
    let bars = TimelineMarkerLayout.segmentBars(
      [segment("all", 0, 1_500, "FOCUS_WORK")],
      windowStartedAt: windowStart, windowEndedAt: windowEnd, width: trackWidth)
    XCTAssertEqual(bars[0].offset, 0, accuracy: 0.001)
    XCTAssertEqual(bars[0].width, trackWidth, accuracy: 0.001)
  }

  func testZeroSegmentsProducesNoBars() {
    XCTAssertTrue(
      TimelineMarkerLayout.segmentBars(
        [], windowStartedAt: windowStart, windowEndedAt: windowEnd, width: trackWidth
      ).isEmpty)
  }

  // MARK: Helpers

  private func segment(
    _ id: String, _ start: TimeInterval, _ seconds: TimeInterval, _ category: String
  ) -> LocalTimelineSegment {
    LocalTimelineSegment(
      id: id,
      startedAt: windowStart.addingTimeInterval(start),
      endedAt: windowStart.addingTimeInterval(start + seconds),
      category: category,
      confidence: .medium
    )
  }

  private func makeLayout(
    transitions: [LocalTransitionMarker], clusters: [LocalSwitchingCluster]
  ) -> TimelineMarkerLayout {
    TimelineMarkerLayout.make(
      transitions: transitions,
      clusters: clusters,
      windowStartedAt: windowStart,
      windowEndedAt: windowEnd,
      width: trackWidth
    )
  }

  private func transition(_ id: String, at offset: TimeInterval) -> LocalTransitionMarker {
    LocalTransitionMarker(
      id: id,
      occurredAt: windowStart.addingTimeInterval(offset),
      fromCategory: "FOCUS_WORK",
      toCategory: "COMMUNICATION",
      confidence: .medium
    )
  }

  private func cluster(
    _ id: String, from start: TimeInterval, to end: TimeInterval, count: Int
  ) -> LocalSwitchingCluster {
    LocalSwitchingCluster(
      id: id,
      ruleVersion: 1,
      startedAt: windowStart.addingTimeInterval(start),
      endedAt: windowStart.addingTimeInterval(end),
      transitionCount: count,
      categories: ["FOCUS_WORK", "COMMUNICATION"],
      confidence: .medium,
      explanation: "\(count) switches in 3 minutes between focus work and communication."
    )
  }
}

// MARK: - Work-block evidence timeline, rendered

/// Renders the evidence card for every timeline shape the layout has to
/// survive, so the marks can be looked at rather than reasoned about.
/// Skipped unless `VELVT_TIMELINE_SCREENSHOT_DIR` names an output directory,
/// exactly like the other synthetic snapshot tests. No app launch and no
/// Accessibility permission.
@MainActor
final class TimelineEvidenceSnapshotTests: XCTestCase {
  private let windowStart = Date(timeIntervalSince1970: 1_800_000_000)

  func testRenderTimelineDegenerateCasesWhenRequested() throws {
    guard let output = ProcessInfo.processInfo.environment["VELVT_TIMELINE_SCREENSHOT_DIR"]
    else {
      throw XCTSkip("Set VELVT_TIMELINE_SCREENSHOT_DIR to render evidence timeline screenshots")
    }

    // 0. Nothing switched at all.
    try render(
      card(
        transitions: [], clusters: [],
        segments: [segment("only", 0, 1_500, "FOCUS_WORK", .high)],
        observation: "Velvt observed one category across this work-block window.",
        longestUninterrupted: 1_500, switches: 0, coverage: .good, coverageRatio: 0.98),
      named: "timeline-00-zero-transitions.png", outputDirectory: output)

    // 1. Exactly one switch, mid-window.
    try render(
      card(
        transitions: [transition("s1", 750, "FOCUS_WORK", "COMMUNICATION")], clusters: [],
        segments: [
          segment("a", 0, 750, "FOCUS_WORK", .high),
          segment("b", 750, 750, "COMMUNICATION", .medium),
        ],
        observation: "Velvt observed one category switch in this work-block window.",
        longestUninterrupted: 750, switches: 1, coverage: .good, coverageRatio: 0.96),
      named: "timeline-01-single-transition.png", outputDirectory: output)

    // 2. The reported bug, reconstructed from the numbers on the screenshot:
    //    25m/25m, longest stretch 17s, switches 5, coverage 12%. A 17-second
    //    longest *meaningful* stretch across 12% of 1500 seconds means many
    //    short segments inside the first three minutes, not a handful of long
    //    ones — the switch count is lower than the segment count because
    //    idle, system, duplicate and unclassified movement are excluded from
    //    it. Then collection died and the remaining 22 minutes are empty.
    let deadCollectionSegments: [LocalTimelineSegment] = {
      let categories = ["FOCUS_WORK", "REFERENCE", "FOCUS_WORK", "COMMUNICATION", "UNCLASSIFIED"]
      let lengths: [TimeInterval] = [11, 17, 9, 14, 6, 13, 17, 8, 15, 12, 16, 9, 14, 11, 17, 11]
      var start: TimeInterval = 0
      return lengths.enumerated().map { index, length in
        defer { start += length }
        return segment(
          "g\(index)", start, length, categories[index % categories.count],
          index % 3 == 0 ? .medium : .low)
      }
    }()
    let deadCollection = card(
      transitions: [
        transition("s1", 11, "FOCUS_WORK", "REFERENCE"),
        transition("s2", 28, "REFERENCE", "FOCUS_WORK"),
        transition("s3", 37, "FOCUS_WORK", "COMMUNICATION"),
        transition("s4", 64, "COMMUNICATION", "FOCUS_WORK"),
        transition("s5", 81, "FOCUS_WORK", "REFERENCE"),
      ],
      clusters: [
        clusterFixture("c1", 11, 81, 5,
          "5 switches in 2 minutes between focus work, reference, and communication.")
      ],
      segments: deadCollectionSegments,
      observation: "Velvt observed one switching cluster in this work-block window.",
      longestUninterrupted: 17, switches: 5, coverage: .partial, coverageRatio: 0.12)
    try render(
      deadCollection, named: "timeline-02-dead-collection-600pt.png", outputDirectory: output)
    try render(
      deadCollection, named: "timeline-02-dead-collection-420pt.png", outputDirectory: output,
      size: NSSize(width: 420, height: 300))

    // 3. Transitions spanning the full width.
    try render(
      card(
        transitions: (0..<6).map {
          transition("s\($0)", Double($0) * 300, "FOCUS_WORK", "REFERENCE")
        },
        clusters: [],
        segments: (0..<5).map {
          segment("g\($0)", Double($0) * 300, 300, $0 % 2 == 0 ? "FOCUS_WORK" : "REFERENCE", .high)
        },
        observation: "Velvt observed six category switches across this work-block window.",
        longestUninterrupted: 300, switches: 6, coverage: .good, coverageRatio: 0.94),
      named: "timeline-03-full-width-spread.png", outputDirectory: output)

    // 4. The clamp's boundary conditions: one transition at exactly t=0 and
    //    one at exactly t=end, plus a cluster that starts at t=0 (the case
    //    that used to draw the cluster glyph outside the leading edge) and
    //    one that ends at t=end.
    try render(
      card(
        transitions: [
          transition("first", 0, "REFERENCE", "FOCUS_WORK"),
          transition("last", 1_500, "FOCUS_WORK", "COMMUNICATION"),
        ],
        clusters: [
          clusterFixture("c-start", 0, 90, 3, "3 switches in 2 minutes at the start of the block."),
          clusterFixture(
            "c-end", 1_410, 1_500, 3, "3 switches in 2 minutes at the end of the block."),
        ],
        segments: [segment("all", 0, 1_500, "FOCUS_WORK", .high)],
        observation: "Velvt observed two switching clusters in this work-block window.",
        longestUninterrupted: 1_410, switches: 2, coverage: .good, coverageRatio: 0.99),
      named: "timeline-04-boundaries-t0-and-tend.png", outputDirectory: output)

    // 5. Evenly dense: 120 switches over 25 minutes, which is what the
    //    collapse rule has to survive without folding the bar into one mark.
    try render(
      card(
        transitions: (0..<120).map {
          transition("s\($0)", Double($0) * 12.5, "FOCUS_WORK", "COMMUNICATION")
        },
        clusters: [
          clusterFixture("c1", 100, 400, 24, "24 switches in 5 minutes."),
          clusterFixture("c2", 900, 1_200, 24, "24 switches in 5 minutes."),
        ],
        segments: (0..<10).map {
          segment("g\($0)", Double($0) * 150, 150, $0 % 2 == 0 ? "FOCUS_WORK" : "COMMUNICATION",
            .medium)
        },
        observation: "Velvt observed two switching clusters in this work-block window.",
        longestUninterrupted: 62, switches: 120, coverage: .partial, coverageRatio: 0.61),
      named: "timeline-05-evenly-dense.png", outputDirectory: output)

    // 6. No evidence at all in the window.
    try render(
      card(
        transitions: [], clusters: [], segments: [],
        observation: "Velvt recorded no classified activity in this work-block window.",
        longestUninterrupted: 0, switches: 0, coverage: .noData, coverageRatio: 0.0),
      named: "timeline-06-no-evidence.png", outputDirectory: output)
  }

  // MARK: Fixtures

  private func card(
    transitions: [LocalTransitionMarker],
    clusters: [LocalSwitchingCluster],
    segments: [LocalTimelineSegment],
    observation: String,
    longestUninterrupted: Int,
    switches: Int,
    coverage: LocalDashboardCoverage,
    coverageRatio: Double
  ) -> FocusFragmentationView {
    FocusFragmentationView(
      focus: LocalFocusFragmentation(
        blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
        phase: .completed,
        windowLabel: "Most recent 25 work-block minutes",
        windowStartedAt: windowStart,
        windowEndedAt: windowStart.addingTimeInterval(1_500),
        plannedDurationSeconds: 1_500,
        elapsedDurationSeconds: 1_500,
        longestUninterruptedSeconds: longestUninterrupted,
        observedSwitchCount: switches,
        recoveryCount: 1,
        coverage: coverage,
        coverageRatio: coverageRatio,
        comparison: nil,
        observation: observation,
        nextAction: "Protect the next 10 minutes for the work you chose.",
        segments: segments,
        transitions: transitions,
        clusters: clusters
      ),
      errorMessage: nil,
      onStartWorkBlock: {},
      header: "Ship the evidence timeline fix"
    )
  }

  private func segment(
    _ id: String, _ start: TimeInterval, _ seconds: TimeInterval, _ category: String,
    _ confidence: ClassificationConfidence
  ) -> LocalTimelineSegment {
    LocalTimelineSegment(
      id: id,
      startedAt: windowStart.addingTimeInterval(start),
      endedAt: windowStart.addingTimeInterval(start + seconds),
      category: category,
      confidence: confidence
    )
  }

  private func transition(
    _ id: String, _ offset: TimeInterval, _ from: String, _ to: String
  ) -> LocalTransitionMarker {
    LocalTransitionMarker(
      id: id,
      occurredAt: windowStart.addingTimeInterval(offset),
      fromCategory: from,
      toCategory: to,
      confidence: .medium
    )
  }

  private func clusterFixture(
    _ id: String, _ start: TimeInterval, _ end: TimeInterval, _ count: Int, _ explanation: String
  ) -> LocalSwitchingCluster {
    LocalSwitchingCluster(
      id: id,
      ruleVersion: 1,
      startedAt: windowStart.addingTimeInterval(start),
      endedAt: windowStart.addingTimeInterval(end),
      transitionCount: count,
      categories: ["FOCUS_WORK", "REFERENCE", "COMMUNICATION"],
      confidence: .medium,
      explanation: explanation
    )
  }

  private func render<V: View>(
    _ view: V,
    named name: String,
    outputDirectory: String,
    size: NSSize = NSSize(width: 600, height: 300)
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
}
