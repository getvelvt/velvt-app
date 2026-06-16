import XCTest
@testable import VelvtMac

final class MenuBarStateResolverTests: XCTestCase {

    private let sut = MenuBarStateResolver()

    private static let collectionStatuses: [CollectionStatus] = [
        .idle, .running, .permissionRevoked, .error("boom")
    ]

    private static let connectionStatuses: [ConnectionStatus] = [
        .disconnected, .connecting, .handshaking, .connected,
        .reconnecting(attempt: 1, nextRetryIn: 2)
    ]

    private static let accountStates: [AccountState] = [
        .loggedOut, .loggingIn, .loggedIn(userId: "u1"), .loggingOut, .pendingErasure
    ]

    /// `isDeviceRevoked` is always the most severe signal, regardless of the
    /// other three inputs.
    func testDeviceRevokedTakesPrecedenceOverEverything() {
        for collection in Self.collectionStatuses {
            for connection in Self.connectionStatuses {
                for account in Self.accountStates {
                    let state = sut.resolve(
                        collectionStatus: collection,
                        connectionStatus: connection,
                        accountState: account,
                        isDeviceRevoked: true
                    )
                    XCTAssertEqual(
                        state, .deviceRevoked,
                        "collection=\(collection) connection=\(connection) account=\(account)"
                    )
                }
            }
        }
    }

    /// Exhaustive table over `CollectionStatus` x `ConnectionStatus` x
    /// `AccountState` with `isDeviceRevoked` false: connection takes
    /// precedence over collection, which takes precedence over normal.
    func testResolvesExpectedStateForEveryCombinationWhenNotDeviceRevoked() {
        for collection in Self.collectionStatuses {
            for connection in Self.connectionStatuses {
                for account in Self.accountStates {
                    let state = sut.resolve(
                        collectionStatus: collection,
                        connectionStatus: connection,
                        accountState: account,
                        isDeviceRevoked: false
                    )
                    let expected = Self.expectedState(collection: collection, connection: connection)
                    XCTAssertEqual(
                        state, expected,
                        "collection=\(collection) connection=\(connection) account=\(account)"
                    )
                }
            }
        }
    }

    func testNormalWhenConnectedAndCollectionRunning() {
        let state = sut.resolve(
            collectionStatus: .running,
            connectionStatus: .connected,
            accountState: .loggedIn(userId: "u1"),
            isDeviceRevoked: false
        )
        XCTAssertEqual(state, .normal)
    }

    func testCollectionPausedWhenConnectedButPermissionRevoked() {
        let state = sut.resolve(
            collectionStatus: .permissionRevoked,
            connectionStatus: .connected,
            accountState: .loggedIn(userId: "u1"),
            isDeviceRevoked: false
        )
        XCTAssertEqual(state, .collectionPaused)
    }

    func testIpcDisconnectedWhenNotConnectedEvenIfCollectionRunning() {
        let state = sut.resolve(
            collectionStatus: .running,
            connectionStatus: .disconnected,
            accountState: .loggedIn(userId: "u1"),
            isDeviceRevoked: false
        )
        XCTAssertEqual(state, .ipcDisconnected)
    }

    func testIpcDisconnectedTakesPrecedenceOverCollectionPaused() {
        let state = sut.resolve(
            collectionStatus: .permissionRevoked,
            connectionStatus: .reconnecting(attempt: 2, nextRetryIn: 4),
            accountState: .loggedOut,
            isDeviceRevoked: false
        )
        XCTAssertEqual(state, .ipcDisconnected)
    }

    // MARK: - Oracle

    private static func expectedState(
        collection: CollectionStatus,
        connection: ConnectionStatus
    ) -> MenuBarState {
        if connection != .connected {
            return .ipcDisconnected
        }
        if collection == .permissionRevoked {
            return .collectionPaused
        }
        return .normal
    }
}
