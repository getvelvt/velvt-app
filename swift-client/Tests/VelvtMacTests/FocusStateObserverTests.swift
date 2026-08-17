import Combine
import XCTest

@testable import VelvtMac

/// The Swift side of Focus citizenship observes only: it samples one coarse
/// authorized boolean at event boundaries and reports edges. Every decision
/// derived from Focus state lives in the Rust service.
@MainActor
final class FocusStateObserverTests: XCTestCase {

    private func makeObserver(
        provider: FakeFocusStatusProvider,
        client: FakeIPCClient
    ) -> FocusStateObserver {
        FocusStateObserver(
            ipcClient: client,
            provider: provider,
            now: { Date(timeIntervalSince1970: 1_800_000_000) },
            utcOffsetSeconds: { -28_800 }
        )
    }

    func testReportsOnlyEdgesNotRepeatedSamples() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: false)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: false)
        observer.sample(forceReport: false)
        provider.set(true)
        observer.sample(forceReport: false)
        observer.sample(forceReport: false)
        provider.set(false)
        observer.sample(forceReport: false)
        await observer.inFlightTask?.value

        let actives = client.sentMessages.compactMap { message -> Bool? in
            guard case .focusStateChanged(let transition) = message else { return nil }
            return transition.active
        }
        XCTAssertEqual(actives, [false, true, false])
    }

    func testUnauthorizedProviderReportsNothing() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: nil)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: true)
        observer.sample(forceReport: false)
        await observer.inFlightTask?.value

        XCTAssertTrue(client.sentMessages.isEmpty, "no authorization means no evidence")
    }

    func testReconnectForcesACurrentStateReport() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: true)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: false)
        // Unchanged state, but a reconnect must resynchronize the service.
        observer.sample(forceReport: true)
        await observer.inFlightTask?.value

        XCTAssertEqual(client.sentMessages.count, 2)
    }

    func testTransitionCarriesOnlyCoarseFields() async throws {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: true)
        let observer = makeObserver(provider: provider, client: client)
        observer.sample(forceReport: false)
        await observer.inFlightTask?.value

        let message = try XCTUnwrap(client.sentMessages.first)
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        let data = try encoder.encode(message)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let payload = try XCTUnwrap(object["payload"] as? [String: Any])

        XCTAssertEqual(object["type"] as? String, "focus_state_changed")
        XCTAssertEqual(
            Set(payload.keys), ["active", "occurred_at", "utc_offset_seconds"],
            "the transition must never grow a Focus mode name, schedule, or configuration"
        )
        XCTAssertEqual(payload["active"] as? Bool, true)
        XCTAssertEqual(payload["utc_offset_seconds"] as? Int, -28_800)
    }
}
