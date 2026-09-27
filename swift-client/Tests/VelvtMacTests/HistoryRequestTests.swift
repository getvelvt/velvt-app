import Combine
import XCTest

@testable import VelvtMac

/// When the Patterns card's history is asked for (protocol 33): only once the
/// connection has the stored session, signed in or not, and again when the
/// account settles, when Patterns appears, and on the menu status's cadence
/// while the last one came from this Mac.
@MainActor
final class HistoryRequestTests: XCTestCase {

    // MARK: - Request ordering

    /// A stored session goes to the service before anything that depends on
    /// it. The service reads one message at a time, in order, so a request
    /// that overtook `AuthSession` on a fresh connection met a service with no
    /// session; the insight request lost that race at every launch.
    func testDataRequestsGoOutOnlyAfterTheStoredSessionIsHandedOver() async throws {
        let client = OrderRecordingIPCClient(authSessionDelayNanoseconds: 80_000_000)
        let manager = AccountStateManager(keychain: try keychainWithStoredSession())
        manager.startListening(to: client)
        let loader = MenuBarDataLoader(
            ipcClient: client,
            currentLocalInsightDate: { "2026-09-27" },
            utcOffsetSeconds: { -14_400 }
        )
        loader.start(
            accountState: manager.$accountState.eraseToAnyPublisher(),
            sessionHandedOver: manager.$isSessionHandedOver.eraseToAnyPublisher(),
            messages: manager.serverMessages,
            historyRefreshRequests: Empty<Void, Never>(),
            cadence: Empty<Void, Never>()
        )

        client.setConnectionStatus(.connected)
        try await waitUntil { client.sentMessages.count >= 3 }

        let sent = client.sentMessages
        guard case .authSession = sent.first else {
            return XCTFail("the first message on the connection must be the session, got \(sent)")
        }
        XCTAssertEqual(
            Array(sent.dropFirst()),
            [
                .requestLatestInsight(RequestLatestInsight(date: "2026-09-27")),
                .requestLatestHistory(RequestLatestHistory(days: 14, utcOffsetSeconds: -14_400)),
            ])
    }

    /// Each connection hands the session over afresh: a reconnect holds the
    /// requests back again until its own `AuthSession` has gone.
    func testTheHandoverIsPerConnection() async throws {
        let client = OrderRecordingIPCClient(authSessionDelayNanoseconds: 0)
        let manager = AccountStateManager(keychain: try keychainWithStoredSession())
        manager.startListening(to: client)

        client.setConnectionStatus(.connected)
        try await waitUntil { manager.isSessionHandedOver }

        client.setConnectionStatus(.disconnected)
        try await waitUntil { !manager.isSessionHandedOver }
        client.setConnectionStatus(.connected)
        try await waitUntil { manager.isSessionHandedOver }

        let sessions = client.sentMessages.filter {
            if case .authSession = $0 { return true }
            return false
        }
        XCTAssertEqual(sessions.count, 2)
    }

    /// With no stored session there is nothing to hand over, and nothing to
    /// wait for.
    func testASignedOutConnectionIsReadyAtOnce() async throws {
        let client = OrderRecordingIPCClient(authSessionDelayNanoseconds: 0)
        let manager = AccountStateManager(keychain: FakeKeychain())
        manager.startListening(to: client)

        client.setConnectionStatus(.connected)
        try await waitUntil { manager.isSessionHandedOver }

        XCTAssertTrue(client.sentMessages.isEmpty)
    }

    // MARK: - Signed out

    /// A signed-out Mac is asked for its history, which the service builds
    /// on this Mac, and never for the insight, which only the cloud has. Until
    /// protocol 33 it was asked for neither, and the card said "Loading"
    /// forever.
    func testASignedOutMacAsksForItsHistoryAndNotForTheInsight() async throws {
        let harness = LoaderHarness(account: .loggedOut)

        try await waitUntil { harness.client.sentMessages.count >= 1 }
        try await Task.sleep(nanoseconds: 30_000_000)

        XCTAssertEqual(
            harness.client.sentMessages,
            [.requestLatestHistory(RequestLatestHistory(days: 14, utcOffsetSeconds: 3_600))])
    }

    /// Signing in asks again, for both; the history the signed-out Mac had
    /// may now come from the cloud.
    func testSigningInAsksAgain() async throws {
        let harness = LoaderHarness(account: .loggedOut)
        try await waitUntil { harness.client.sentMessages.count == 1 }

        harness.account.send(.loggingIn)
        harness.account.send(.loggedIn(userId: "u1"))
        try await waitUntil { harness.client.sentMessages.count == 3 }

        XCTAssertEqual(
            Array(harness.client.sentMessages.dropFirst()),
            [
                .requestLatestInsight(RequestLatestInsight(date: "2026-09-27")),
                .requestLatestHistory(RequestLatestHistory(days: 14, utcOffsetSeconds: 3_600)),
            ])
    }

    /// A history built on this Mac while signed out is not thrown away when
    /// a sign-in starts or fails; only leaving a signed-in account clears it.
    func testASignedOutHistorySurvivesASignInThatDoesNotHappen() async throws {
        let account = CurrentValueSubject<AccountState, Never>(.loggedOut)
        let sut = ConcreteDisplayDataCoordinator()
        sut.start(
            serverMessages: Empty<ServerMessage, Never>(),
            connectionStatus: Empty<ConnectionStatus, Never>(),
            accountState: account.eraseToAnyPublisher()
        )
        try await waitUntil { !sut.isSignedIn }
        sut.updateHistory(localHistoryWeeks(readyRecent: 5, readyPrior: 0))

        account.send(.loggingIn)
        account.send(.loggedOut)
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(sut.historyAvailability, .available)
        XCTAssertEqual(sut.historyViewModel.days.count, 14)
    }

    // MARK: - Asking again

    /// While the last history came from this Mac, the menu status's cadence
    /// asks again, but no more than every ten minutes; once the cloud's
    /// arrives it stops.
    func testTheCadenceAsksAgainAtMostEveryTenMinutesWhileTheHistoryIsFromThisMac() async throws {
        let harness = LoaderHarness(account: .loggedIn(userId: "u1"))
        try await waitUntil { harness.historyRequests == 1 }
        harness.messages.send(.historyPayload(localHistoryWeeks(readyRecent: 3, readyPrior: 0)))
        try await settle()

        harness.clock.advance(by: 9 * 60)
        harness.cadence.send()
        try await settle()
        XCTAssertEqual(harness.historyRequests, 1, "asked again before ten minutes had passed")

        harness.clock.advance(by: 60)
        harness.cadence.send()
        try await waitUntil { harness.historyRequests == 2 }
        harness.messages.send(.historyPayload(localHistoryWeeks(readyRecent: 3, readyPrior: 0)))
        try await settle()

        // Ten minutes are counted from the last request, not the first.
        harness.clock.advance(by: 5 * 60)
        harness.cadence.send()
        try await settle()
        XCTAssertEqual(harness.historyRequests, 2)

        harness.messages.send(.historyPayload(HistoryPayload(days: 7, summaries: [], source: .cloud)))
        try await settle()
        harness.clock.advance(by: 60 * 60)
        harness.cadence.send()
        try await settle()
        XCTAssertEqual(harness.historyRequests, 2, "a synced history is not asked for on the cadence")
        XCTAssertEqual(harness.insightRequests, 1, "the cadence never asks for the insight")
    }

    /// The Patterns tab appearing asks for the history again, once per
    /// answer: a second appearance while the first request is unanswered adds
    /// nothing.
    func testAppearingAsksForTheHistoryAgainOncePerAnswer() async throws {
        let harness = LoaderHarness(account: .loggedIn(userId: "u1"))
        try await waitUntil { harness.historyRequests == 1 }
        harness.messages.send(.historyPayload(HistoryPayload(days: 7, summaries: [], source: .cloud)))
        try await settle()

        harness.appearances.send()
        try await waitUntil { harness.historyRequests == 2 }
        harness.appearances.send()
        try await settle()
        XCTAssertEqual(harness.historyRequests, 2)

        harness.messages.send(
            .cacheEmpty(CacheEmpty(payloadType: "history_payload", reason: "local_history_unavailable")))
        try await settle()
        harness.appearances.send()
        try await waitUntil { harness.historyRequests == 3 }
    }

    /// Nothing is asked for before the connection has its session.
    func testNothingIsAskedForBeforeTheHandover() async throws {
        let harness = LoaderHarness(account: .loggedIn(userId: "u1"), handedOver: false)

        harness.appearances.send()
        harness.cadence.send()
        try await settle()
        XCTAssertTrue(harness.client.sentMessages.isEmpty)

        harness.handedOver.send(true)
        try await waitUntil { harness.historyRequests == 1 }
    }

    // MARK: - Helpers

    private func keychainWithStoredSession() throws -> FakeKeychain {
        let keychain = FakeKeychain()
        let snapshot: [String: Any] = [
            "userId": "u1",
            "pendingDeletion": false,
            "session": try JSONSerialization.jsonObject(
                with: JSONEncoder().encode(
                    AuthSession(
                        deviceId: "device-1",
                        accessToken: "access-token",
                        refreshToken: "refresh-token",
                        expiresAt: Date(timeIntervalSinceNow: 3_600)
                    ))),
        ]
        let data = try JSONSerialization.data(withJSONObject: snapshot)
        try keychain.store(token: String(decoding: data, as: UTF8.self), for: .authSnapshot)
        return keychain
    }

    private func settle() async throws {
        try await Task.sleep(nanoseconds: 30_000_000)
    }

    private func waitUntil(
        _ condition: @escaping @MainActor () -> Bool,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async throws {
        for _ in 0..<200 {
            if condition() { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTFail("condition never held", file: file, line: line)
    }
}

/// Fourteen local days ending 2026-09-27, as a history built on this Mac:
/// `readyRecent` of the last seven and `readyPrior` of the seven before them
/// observed.
func localHistoryWeeks(readyRecent: Int, readyPrior: Int) -> HistoryPayload {
    let summaries = (0..<14).map { index -> DailySummary in
        let recent = index >= 7
        let position = recent ? index - 7 : index
        let ready = position < (recent ? readyRecent : readyPrior)
        let date = String(format: "2026-09-%02d", 14 + index)
        guard ready else {
            return DailySummary(
                date: date, status: .noData, eventCount: 0, focusScore: nil,
                fragmentationScore: nil, confidenceLevel: .none, activeSeconds: 0,
                baselineStatus: "unavailable")
        }
        return DailySummary(
            date: date, status: .ready, eventCount: 40, focusScore: nil,
            fragmentationScore: nil, confidenceLevel: .low, activeSeconds: 7_200,
            focusedSeconds: recent ? 4_500 : 3_600, meaningfulSwitchCount: recent ? 6 : 9,
            longestUninterruptedSeconds: 1_500, baselineStatus: "unavailable")
    }
    return HistoryPayload(days: 14, summaries: summaries, source: .thisMac)
}

/// A loader wired to subjects a test drives, over a recording client, at
/// UTC+01:00 and a clock the test moves.
@MainActor
private final class LoaderHarness {
    let client: OrderRecordingIPCClient
    let account: CurrentValueSubject<AccountState, Never>
    let handedOver: CurrentValueSubject<Bool, Never>
    let messages = PassthroughSubject<ServerMessage, Never>()
    let appearances = PassthroughSubject<Void, Never>()
    let cadence = PassthroughSubject<Void, Never>()
    let clock: TestClock
    let loader: MenuBarDataLoader

    init(account: AccountState, handedOver: Bool = true) {
        let client = OrderRecordingIPCClient(authSessionDelayNanoseconds: 0)
        let clock = TestClock()
        self.client = client
        self.clock = clock
        self.account = CurrentValueSubject(account)
        self.handedOver = CurrentValueSubject(handedOver)
        loader = MenuBarDataLoader(
            ipcClient: client,
            currentLocalInsightDate: { "2026-09-27" },
            utcOffsetSeconds: { 3_600 },
            now: { clock.now }
        )
        loader.start(
            accountState: self.account.eraseToAnyPublisher(),
            sessionHandedOver: self.handedOver.eraseToAnyPublisher(),
            messages: messages,
            historyRefreshRequests: appearances,
            cadence: cadence
        )
    }

    var historyRequests: Int {
        client.sentMessages.filter {
            if case .requestLatestHistory = $0 { return true }
            return false
        }.count
    }

    var insightRequests: Int {
        client.sentMessages.filter {
            if case .requestLatestInsight = $0 { return true }
            return false
        }.count
    }
}

private final class TestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var current = Date(timeIntervalSince1970: 1_790_000_000)

    var now: Date { lock.withLock { current } }

    func advance(by seconds: TimeInterval) {
        lock.withLock { current = current.addingTimeInterval(seconds) }
    }
}

/// Records sends in the order they complete. An `AuthSession` can be made to
/// take a while, which is what let a request overtake it.
private final class OrderRecordingIPCClient: IPCClientProtocol, @unchecked Sendable {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { _ in }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }
    var sentMessages: [ClientMessage] {
        lock.withLock { messages }
    }

    private let authSessionDelayNanoseconds: UInt64
    private let lock = NSLock()
    private let statusSubject = CurrentValueSubject<ConnectionStatus, Never>(.disconnected)
    private var messages: [ClientMessage] = []

    init(authSessionDelayNanoseconds: UInt64) {
        self.authSessionDelayNanoseconds = authSessionDelayNanoseconds
    }

    func connect() async throws {
        statusSubject.send(.connected)
    }

    func disconnect() {
        statusSubject.send(.disconnected)
    }

    func send(_ message: ClientMessage) async throws {
        if case .authSession = message, authSessionDelayNanoseconds > 0 {
            try await Task.sleep(nanoseconds: authSessionDelayNanoseconds)
        }
        lock.withLock { messages.append(message) }
    }

    func setConnectionStatus(_ status: ConnectionStatus) {
        statusSubject.send(status)
    }
}
