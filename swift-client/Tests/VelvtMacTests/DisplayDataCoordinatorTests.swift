import Combine
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

    func testLatestClosedUTCDateStringUsesPreviousUTCDate() throws {
        let formatter = ISO8601DateFormatter()
        let now = try XCTUnwrap(formatter.date(from: "2026-06-27T02:00:00Z"))

        XCTAssertEqual(MenuBarDataLoader.latestClosedUTCDateString(now: now), "2026-06-26")
    }

    func testDataLoaderRequestsLatestClosedInsightDateWhenConnectedAndLoggedIn() async {
        let client = FakeIPCClient()
        client.setConnectionStatus(.connected)
        let account = CurrentValueSubject<AccountState, Never>(.loggedIn(userId: "user-1"))
        let sut = MenuBarDataLoader(
            ipcClient: client,
            latestClosedInsightDate: { "2026-06-26" }
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
        XCTAssertTrue(client.sentMessages.contains(.requestLatestHistory(RequestLatestHistory(days: 7))))
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
