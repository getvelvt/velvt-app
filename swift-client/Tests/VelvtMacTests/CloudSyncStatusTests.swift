import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

// MARK: - The header's sync line, one state at a time

final class CloudSyncStatusPresentationTests: XCTestCase {
    private let locale = Locale(identifier: "en_US")
    private let utc = TimeZone(identifier: "UTC")!
    private let now = Date(timeIntervalSince1970: 1_790_000_000)
    private let signedIn = AccountState.loggedIn(userId: "u1")
    private static let dropRule =
        "Events that still can't upload after about three days of retrying, or within 30 days, "
        + "are dropped; your history on this Mac is not affected."

    private func presentation(
        _ accountState: AccountState?,
        reauth: Bool = false,
        _ status: MenuStatus?
    ) -> CloudSyncStatusPresentation {
        CloudSyncStatusPresentation(
            accountState: accountState,
            requiresReauthentication: reauth,
            status: status,
            now: now,
            locale: locale,
            timeZone: utc
        )
    }

    private func status(
        cloudReady: Bool = true,
        uploadStatus: String = "ready",
        pending: Int = 0,
        failed: Int = 0,
        queued: Int = 0,
        nextAttempt: Date? = nil
    ) -> MenuStatus {
        MenuStatus(
            deviceID: "device",
            cloudReady: cloudReady,
            uploadStatus: uploadStatus,
            lastUploadErrorCode: nil,
            nextUploadAttemptAt: nextAttempt,
            pendingUploadBatchCount: pending,
            failedUploadBatchCount: failed,
            rejectedUploadBatchCount: 0,
            queuedEventCount: queued,
            queuedEvents: []
        )
    }

    /// The case the founder reported: the server is down, the Mac is signed
    /// in, and about 14,600 already-classified events are queued.
    func testOfflineWhileSignedInIsCalmAndKeepsTheCountForTheDetail() {
        let sut = presentation(
            signedIn,
            status(cloudReady: false, uploadStatus: "network_unavailable", pending: 580, queued: 14_632))

        XCTAssertEqual(sut.state, .offline)
        XCTAssertEqual(sut.headline, "Offline · everything local still works")
        XCTAssertEqual(
            sut.detail,
            "Uploads resume when Velvt can reach its server. 14,632 events are waiting. " + Self.dropRule
        )
    }

    func testOfflineWithNothingQueuedSaysNothingAboutDropping() {
        let sut = presentation(signedIn, status(cloudReady: false, uploadStatus: "network_unavailable"))

        XCTAssertEqual(sut.state, .offline)
        XCTAssertEqual(sut.headline, "Offline · everything local still works")
        XCTAssertEqual(sut.detail, "Uploads resume when Velvt can reach its server.")
    }

    func testOneQueuedEventIsSingular() {
        let sut = presentation(signedIn, status(cloudReady: false, uploadStatus: "network_unavailable", queued: 1))

        XCTAssertEqual(
            sut.detail,
            "Uploads resume when Velvt can reach its server. 1 event is waiting. " + Self.dropRule
        )
    }

    /// Offline is about the server, whatever the queue's last error says.
    func testAnUnreachableServerWinsOverTheLastUploadError() {
        for uploadStatus in ["auth_required", "rate_limited", "retrying", "pending", "privacy_rejected"] {
            let sut = presentation(signedIn, status(cloudReady: false, uploadStatus: uploadStatus, failed: 3))
            XCTAssertEqual(sut.state, .offline, uploadStatus)
        }
    }

    /// `cloudReady` is the readiness probe, not the account: signed out, the
    /// header talks about the account even when the server answers.
    func testNeverSignedInOrSignedOut() {
        for cloudReady in [true, false] {
            let sut = presentation(.loggedOut, status(cloudReady: cloudReady))
            XCTAssertEqual(sut.state, .signedOut)
            XCTAssertEqual(sut.headline, "Local only · sign in to sync")
            XCTAssertEqual(sut.detail, "No activity is uploaded while you're signed out.")
        }
        // Before the first status arrives, too.
        XCTAssertEqual(presentation(.loggedOut, nil).headline, "Local only · sign in to sync")
    }

    /// Nothing new queues while signed out, but batches queued before it stay
    /// and keep being retried.
    func testSignedOutWithBatchesLeftFromBefore() {
        let sut = presentation(.loggedOut, status(cloudReady: false, pending: 2, queued: 120))

        XCTAssertEqual(sut.state, .signedOut)
        XCTAssertEqual(
            sut.detail,
            "No activity is uploaded while you're signed out. 120 events are waiting. " + Self.dropRule
        )
    }

    func testSessionEndedByTheServer() {
        let sut = presentation(.loggedOut, reauth: true, status(cloudReady: true, pending: 1, queued: 40))

        XCTAssertEqual(sut.state, .signInAgain)
        XCTAssertEqual(sut.headline, "Signed out · everything local still works")
        XCTAssertEqual(sut.detail, "Sign in again to resume uploads. 40 events are waiting. " + Self.dropRule)
    }

    /// Signing in again after the server ended the session reads as signing
    /// in: the ended-session flag clears only once that sign-in succeeds.
    func testSigningInAgainOutranksTheEndedSession() {
        let sut = presentation(.loggingIn, reauth: true, status(cloudReady: true, pending: 1, queued: 40))

        XCTAssertEqual(sut.state, .signingIn)
        XCTAssertEqual(sut.headline, "Signing in…")
        XCTAssertNil(sut.detail)
    }

    func testAccountTransitions() {
        XCTAssertEqual(presentation(.loggingIn, nil).headline, "Signing in…")
        XCTAssertEqual(presentation(.loggingOut, nil).headline, "Signing out…")
        XCTAssertEqual(presentation(.pendingErasure, nil).headline, "Account deletion in progress")
        for state in [AccountState.loggingIn, .loggingOut, .pendingErasure] {
            XCTAssertNil(presentation(state, nil).detail)
        }
    }

    func testSignedInBeforeTheFirstStatus() {
        let sut = presentation(signedIn, nil)

        XCTAssertEqual(sut.state, .checking)
        XCTAssertEqual(sut.headline, "Checking sync…")
        XCTAssertNil(sut.detail)
    }

    func testNoAccountModelAtAll() {
        let sut = presentation(nil, status())

        XCTAssertEqual(sut.state, .unavailable)
        XCTAssertEqual(sut.headline, "Sync status unavailable")
        XCTAssertNil(sut.detail)
    }

    func testUploadingNormally() {
        let pending = presentation(signedIn, status(uploadStatus: "pending", pending: 2, queued: 60))
        XCTAssertEqual(pending.state, .syncing)
        XCTAssertEqual(pending.headline, "Syncing…")
        XCTAssertNil(pending.detail)

        let ready = presentation(signedIn, status(uploadStatus: "ready", queued: 4))
        XCTAssertEqual(ready.state, .synced)
        XCTAssertEqual(ready.headline, "Synced")
        XCTAssertNil(ready.detail)
    }

    func testBackingOffWithAKnownRetryTime() {
        let retryAt = now.addingTimeInterval(9 * 60)
        for uploadStatus in ["retrying", "rate_limited"] {
            let sut = presentation(
                signedIn, status(uploadStatus: uploadStatus, failed: 3, queued: 90, nextAttempt: retryAt))
            let time = retryAt.formatted(
                Date.FormatStyle(date: .omitted, time: .shortened, locale: locale, timeZone: utc))

            XCTAssertEqual(sut.state, .backingOff, uploadStatus)
            XCTAssertEqual(sut.headline, "Uploads paused briefly · everything local still works")
            XCTAssertEqual(
                sut.detail, "Velvt tries again at \(time). 90 events are waiting. " + Self.dropRule)
        }
    }

    func testBackingOffWhoseRetryIsAlreadyDue() {
        let sut = presentation(
            signedIn,
            status(uploadStatus: "retrying", failed: 1, queued: 3, nextAttempt: now.addingTimeInterval(-5)))

        XCTAssertEqual(sut.detail, "Velvt tries again shortly. 3 events are waiting. " + Self.dropRule)
    }

    func testServerRefusedThisMacsCredentials() {
        let sut = presentation(signedIn, status(uploadStatus: "auth_required", failed: 2, queued: 30))

        XCTAssertEqual(sut.state, .uploadsPaused)
        XCTAssertEqual(sut.headline, "Uploads paused · everything local still works")
        XCTAssertEqual(
            sut.detail,
            "Velvt tries these uploads again every 15 minutes. "
                + "30 events are waiting. " + Self.dropRule
        )
    }

    /// `last_upload_error_code` can come from a batch that was already
    /// abandoned; with nothing left waiting, nothing is paused.
    func testAStaleErrorCodeWithAnEmptyQueueIsSynced() {
        for uploadStatus in ["auth_required", "retrying", "rate_limited"] {
            XCTAssertEqual(presentation(signedIn, status(uploadStatus: uploadStatus)).state, .synced, uploadStatus)
        }
    }

    /// `privacy_rejected` names one refused batch; the counts say whether the
    /// rest of the queue is moving.
    func testPrivacyRejectedFallsBackToTheQueue() {
        XCTAssertEqual(presentation(signedIn, status(uploadStatus: "privacy_rejected")).state, .synced)
        XCTAssertEqual(
            presentation(signedIn, status(uploadStatus: "privacy_rejected", pending: 1)).state, .syncing)
        XCTAssertEqual(
            presentation(signedIn, status(uploadStatus: "privacy_rejected", failed: 1)).state, .backingOff)
    }

    /// The founder's rule: no raw count in the header, and no alarm words.
    func testNoHeadlineCarriesACountOrAnAlarmWord() {
        let alarming = ["error", "fail", "unreachable", "queued", "cloud"]
        for state in CloudSyncStatusPresentation.State.allCases {
            let headline = CloudSyncStatusPresentation.headline(for: state)
            XCTAssertNil(headline.rangeOfCharacter(from: .decimalDigits), headline)
            for word in alarming {
                XCTAssertFalse(headline.lowercased().contains(word), "\(headline) contains \(word)")
            }
        }
    }

    func testTheDropRuleIsTheOneThePresentationShips() {
        XCTAssertEqual(CloudSyncStatusPresentation.dropRule, Self.dropRule)
    }
}

// MARK: - The header redraws when the service answers

@MainActor
final class CloudSyncStatusHeaderRedrawTests: XCTestCase {
    /// `MenuBarPopoverView` holds the status model as a plain optional `let`.
    /// A line computed in its body waited for some unrelated observed object to
    /// publish before it showed a new `menu_status`. Here nothing else
    /// publishes: only the status arrives, and the header must follow it.
    func testTheHeaderFollowsAMenuStatusWithNothingElsePublishing() async throws {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let account = try signedInAccount()
        let menuStatus = MenuStatusViewModel(ipcClient: client, messages: messages)
        let headlines = HeadlineRecorder()
        let host = hosted(popover(account: account, menuStatus: menuStatus, client: client), recordingInto: headlines)

        try await waitUntil("the header draws before the first status", laying: host) {
            headlines.latest == "Checking sync…"
        }

        messages.send(.menuStatus(offlineStatus(queued: 14_632)))
        try await waitUntil("the header follows the status with nothing else publishing", laying: host) {
            headlines.latest == "Offline · everything local still works"
        }

        messages.send(.menuStatus(onlineStatus()))
        try await waitUntil("and follows it back when the server answers again", laying: host) {
            headlines.latest == "Synced"
        }
    }

    /// The account model is a plain optional `let` on the popover too, so a
    /// session the server ended has to reach the header by itself.
    func testTheHeaderFollowsTheAccountWithNothingElsePublishing() async throws {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let account = try signedInAccount()
        account.startListening(to: client)
        let menuStatus = MenuStatusViewModel(ipcClient: client, messages: messages)
        let headlines = HeadlineRecorder()
        let host = hosted(popover(account: account, menuStatus: menuStatus, client: client), recordingInto: headlines)
        messages.send(.menuStatus(onlineStatus()))
        try await waitUntil("the header shows the synced state", laying: host) { headlines.latest == "Synced" }

        client.inject(.needsReauth(NeedsReauth(reason: "token_expired")))

        try await waitUntil("the header follows the ended session", laying: host) {
            headlines.latest == "Signed out · everything local still works"
        }
    }

    /// The Keychain snapshot `AccountStateManager.init` restores a session
    /// from, in the shape it stores one.
    private struct StoredSession: Encodable {
        let userId: String
        let email: String?
        let pendingDeletion: Bool
        let session: AuthSession
    }

    private func signedInAccount() throws -> AccountStateManager {
        let keychain = FakeKeychain()
        let snapshot = StoredSession(
            userId: "u1",
            email: nil,
            pendingDeletion: false,
            session: AuthSession(
                deviceId: "device-1",
                accessToken: "access-token",
                refreshToken: "refresh-token",
                expiresAt: Date(timeIntervalSinceNow: 3600)
            )
        )
        let encoded = String(decoding: try JSONEncoder().encode(snapshot), as: UTF8.self)
        try keychain.store(token: encoded, for: .authSnapshot)
        let account = AccountStateManager(keychain: keychain)
        XCTAssertEqual(account.accountState, .loggedIn(userId: "u1"))
        return account
    }

    private final class HeadlineRecorder {
        var latest: String?
    }

    private func hosted(_ view: MenuBarPopoverView, recordingInto recorder: HeadlineRecorder) -> NSView {
        let host = NSHostingView(
            rootView: view.onPreferenceChange(CloudSyncHeadlinePreferenceKey.self) { headline in
                recorder.latest = headline
            }
        )
        host.frame = NSRect(x: 0, y: 0, width: 600, height: 480)
        host.layoutSubtreeIfNeeded()
        return host
    }

    private func popover(
        account: AccountStateManager,
        menuStatus: MenuStatusViewModel,
        client: FakeIPCClient
    ) -> MenuBarPopoverView {
        MenuBarPopoverView(
            presentation: PermissionPresentationModel(
                permissionManager: FakePermissionManager(),
                onboardingStateStore: InMemoryOnboardingStateStore()
            ),
            coordinator: ConcreteDisplayDataCoordinator(),
            serviceConnectionStatus: ServiceConnectionStatusModel(
                connectionStatus: Just(.connected).eraseToAnyPublisher()
            ),
            collectionActivityStatus: CollectionActivityStatusModel(
                collectionStatus: Just(.running).eraseToAnyPublisher()
            ),
            currentActivity: CurrentActivityModel(),
            serviceAlertModel: ServiceAlertModel(messages: Empty<ServerMessage, Never>()),
            accountStateManager: account,
            ipcClient: client,
            menuStatusViewModel: menuStatus,
            updateController: .disabled(),
            onEscape: {}
        )
    }

    /// A hosting view with no window has no display cycle to commit a
    /// pending update, so each turn lays the view out the way the window
    /// would before it drew.
    private func waitUntil(
        _ description: String,
        laying host: NSView,
        timeout: TimeInterval = 3,
        _ condition: @escaping () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            host.layoutSubtreeIfNeeded()
            if condition() { return }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTFail("Timed out waiting until \(description)")
    }

    private func offlineStatus(queued: Int) -> MenuStatus {
        MenuStatus(
            deviceID: "device",
            cloudReady: false,
            uploadStatus: "network_unavailable",
            lastUploadErrorCode: "transport",
            nextUploadAttemptAt: nil,
            pendingUploadBatchCount: 580,
            failedUploadBatchCount: 0,
            rejectedUploadBatchCount: 0,
            queuedEventCount: queued,
            queuedEvents: []
        )
    }

    private func onlineStatus() -> MenuStatus {
        MenuStatus(
            deviceID: "device",
            cloudReady: true,
            uploadStatus: "ready",
            lastUploadErrorCode: nil,
            nextUploadAttemptAt: nil,
            pendingUploadBatchCount: 0,
            failedUploadBatchCount: 0,
            rejectedUploadBatchCount: 0,
            queuedEventCount: 0,
            queuedEvents: []
        )
    }
}
