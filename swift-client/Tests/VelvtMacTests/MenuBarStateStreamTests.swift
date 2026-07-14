import Combine
import XCTest
@testable import VelvtMac

/// Covers the "multiple publishers change simultaneously" race the pure
/// `MenuBarStateResolver` cannot, by itself, protect against: `CombineLatest`
/// re-emits on *every* upstream change using the latest cached value from
/// the others, so two changes that are logically one event but arrive as
/// separate emissions (e.g. `AccountStateManager` setting `accountState` and
/// `isDeviceRevoked` in two statements) can momentarily combine a stale value
/// with a fresh one. `MenuBarStateStream` guards against this by debouncing
/// the *resolved* state before it reaches the icon.
@MainActor
final class MenuBarStateStreamTests: XCTestCase {

    func testRapidSuccessiveInputChangesCoalesceToOnlyTheFinalCorrectState() async throws {
        let collectionSubject = PassthroughSubject<CollectionStatus, Never>()
        let connectionSubject = PassthroughSubject<ConnectionStatus, Never>()
        let accountManager = AccountStateManager(keychain: FakeKeychain())
        var recorded: [MenuBarState] = []
        let settled = expectation(description: "final normal state emitted")

        let cancellable = MenuBarStateStream.make(
            resolver: MenuBarStateResolver(),
            collectionStatus: collectionSubject,
            connectionStatus: connectionSubject,
            accountStateManager: accountManager,
            debounceInterval: .milliseconds(20)
        ).sink { state in
            recorded.append(state)
            if state == .normal {
                settled.fulfill()
            }
        }

        collectionSubject.send(.running)
        connectionSubject.send(.connected)
        // A rapid burst within the debounce window: flips to permissionRevoked
        // and immediately back to running, as if two independent status
        // sources both changed for the same underlying event.
        collectionSubject.send(.permissionRevoked)
        connectionSubject.send(.connected)
        collectionSubject.send(.running)

        await fulfillment(of: [settled], timeout: 2)

        XCTAssertEqual(recorded, [.normal], "Only the settled final state should reach the icon — no transient flicker")
        cancellable.cancel()
    }

    func testDeviceRevokedPushSettlesWithoutAnIncorrectIntermediateStateReachingTheSink() async throws {
        // AccountStateManager's deviceRevoked handler sets `accountState` and
        // `isDeviceRevoked` in two separate statements. This reproduces that
        // exact compound update through the real listener pipeline and
        // verifies the debounced stream still settles cleanly on
        // `.deviceRevoked` without ever delivering a different state after
        // the push begins.
        let client = FakeIPCClient()
        let accountManager = AccountStateManager(keychain: FakeKeychain())
        accountManager.startListening(to: client)

        let collectionSubject = CurrentValueSubject<CollectionStatus, Never>(.running)
        let connectionSubject = CurrentValueSubject<ConnectionStatus, Never>(.connected)
        var recorded: [MenuBarState] = []
        let baselineSettled = expectation(description: "baseline normal state emitted")
        let revocationSettled = expectation(description: "device revoked state emitted")

        let cancellable = MenuBarStateStream.make(
            resolver: MenuBarStateResolver(),
            collectionStatus: collectionSubject,
            connectionStatus: connectionSubject,
            accountStateManager: accountManager,
            debounceInterval: .milliseconds(20)
        ).sink { state in
            recorded.append(state)
            if state == .normal {
                baselineSettled.fulfill()
            } else if state == .deviceRevoked {
                revocationSettled.fulfill()
            }
        }

        // Baseline settles to .normal before the revoke push.
        await fulfillment(of: [baselineSettled], timeout: 2)
        XCTAssertEqual(recorded, [.normal])

        client.inject(.deviceRevoked(DeviceRevoked(message: "revoked")))
        await fulfillment(of: [revocationSettled], timeout: 2)

        XCTAssertEqual(recorded, [.normal, .deviceRevoked], "No intermediate non-revoked state must be delivered once the revoke push starts")
        cancellable.cancel()
    }

    func testSettlesToCollectionPausedWhenConnectionRecoversWhilePermissionIsRevoked() async throws {
        // Demonstrates the resolver's own precedence (ipcDisconnected >
        // collectionPaused) still applies after debounced settling, even
        // when the two inputs change in the same burst.
        let collectionSubject = PassthroughSubject<CollectionStatus, Never>()
        let connectionSubject = PassthroughSubject<ConnectionStatus, Never>()
        let accountManager = AccountStateManager(keychain: FakeKeychain())
        var recorded: [MenuBarState] = []
        let settled = expectation(description: "collection paused state emitted")

        let cancellable = MenuBarStateStream.make(
            resolver: MenuBarStateResolver(),
            collectionStatus: collectionSubject,
            connectionStatus: connectionSubject,
            accountStateManager: accountManager,
            debounceInterval: .milliseconds(20)
        ).sink { state in
            recorded.append(state)
            if state == .collectionPaused {
                settled.fulfill()
            }
        }

        connectionSubject.send(.disconnected)
        collectionSubject.send(.permissionRevoked)
        // Burst: connection recovers, but collection is still revoked.
        connectionSubject.send(.connected)

        await fulfillment(of: [settled], timeout: 2)

        XCTAssertEqual(recorded, [.collectionPaused])
        cancellable.cancel()
    }
}
