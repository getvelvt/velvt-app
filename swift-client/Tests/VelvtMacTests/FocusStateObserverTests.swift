import Combine
import Intents
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

        XCTAssertEqual(edges(client), [false, true, false])
    }

    func testUnauthorizedProviderReportsNoEvidence() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: nil)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: true)
        observer.sample(forceReport: false)
        await observer.inFlightTask?.value

        XCTAssertEqual(edges(client), [], "no authorization means no evidence")
        XCTAssertEqual(availabilities(client), [.notDetermined], "only that it cannot tell")
    }

    func testReconnectForcesACurrentStateReport() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: true)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: false)
        // Unchanged state, but a reconnect must resynchronize the service.
        observer.sample(forceReport: true)
        await observer.inFlightTask?.value

        XCTAssertEqual(edges(client), [true, true])
        XCTAssertEqual(availabilities(client), [.available, .available])
    }

    /// Proto v36, B5. The installed 1.0.13 build has no Communication
    /// Notifications entitlement: the person can allow Focus status and the
    /// system still answers `isFocused == false`, a value it never read. That
    /// build reported "off" on every reconnect. Now the availability goes
    /// ahead of any edge, and an unreadable status sends no edge at all.
    func testAnUnreadableStatusReportsItsAvailabilityAndNoEdge() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: false, availability: .unavailable)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: true)
        observer.sample(forceReport: false)
        provider.set(true)
        observer.sample(forceReport: false)
        observer.sample(forceReport: true)
        await observer.inFlightTask?.value

        XCTAssertEqual(edges(client), [], "a value the app cannot vouch for is never evidence")
        XCTAssertEqual(availabilities(client), [.unavailable, .unavailable])
    }

    /// The availability is reported when it changes (the onboarding grant is
    /// read at the next activation), always ahead of the edge it qualifies,
    /// and an edge read after a stretch of unreadability is sent afresh.
    func testAvailabilityChangesAreReportedAheadOfTheirEdges() async {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: nil)
        let observer = makeObserver(provider: provider, client: client)

        observer.sample(forceReport: true)
        provider.set(true)
        observer.sample(forceReport: false)
        provider.setAvailability(.denied)
        observer.sample(forceReport: false)
        provider.setAvailability(nil)
        observer.sample(forceReport: false)
        await observer.inFlightTask?.value

        let kinds = client.sentMessages.map { message -> String in
            switch message {
            case .focusStatusAvailability(let report): return report.availability.rawValue
            case .focusStateChanged(let transition): return "edge:\(transition.active)"
            default: return "other"
            }
        }
        XCTAssertEqual(
            kinds,
            ["not_determined", "available", "edge:true", "denied", "available", "edge:true"]
        )
    }

    func testTheAvailabilityFactCarriesOneValueAndNothingAboutTheFocus() async throws {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: true, availability: .unavailable)
        let observer = makeObserver(provider: provider, client: client)
        observer.sample(forceReport: true)
        await observer.inFlightTask?.value

        let message = try XCTUnwrap(client.sentMessages.first)
        let data = try JSONEncoder().encode(message)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let payload = try XCTUnwrap(object["payload"] as? [String: Any])
        XCTAssertEqual(object["type"] as? String, "focus_status_availability")
        XCTAssertEqual(Set(payload.keys), ["availability"])
        XCTAssertEqual(payload["availability"] as? String, "unavailable")
    }

    /// The entitlement decides first: an authorized but unentitled app cannot
    /// read Focus status (Apple: "Focus Status will only be available to apps
    /// that have the Communication Notifications capability").
    func testAvailabilityResolvesTheEntitlementBeforeThePersonsAnswer() {
        let unentitled: [INFocusStatusAuthorizationStatus] = [
            .authorized, .denied, .notDetermined, .restricted,
        ]
        for status in unentitled {
            XCTAssertEqual(
                FocusStatusAvailability.resolve(entitled: false, authorization: status),
                .unavailable,
                "status \(status.rawValue)"
            )
        }
        XCTAssertEqual(FocusStatusAvailability.resolve(entitled: true, authorization: .authorized), .available)
        XCTAssertEqual(FocusStatusAvailability.resolve(entitled: true, authorization: .denied), .denied)
        XCTAssertEqual(
            FocusStatusAvailability.resolve(entitled: true, authorization: .notDetermined), .notDetermined)
        XCTAssertEqual(FocusStatusAvailability.resolve(entitled: true, authorization: .restricted), .unavailable)
    }

    func testAReadingNeverCarriesAValueItCannotVouchFor() {
        XCTAssertEqual(
            FocusStatusReading(availability: .unavailable, isFocused: false),
            FocusStatusReading(availability: .unavailable, isFocused: nil)
        )
        XCTAssertNil(FocusStatusReading(availability: .denied, isFocused: true).isFocused)
        let empty = FocusStatusReading(availability: .available, isFocused: nil)
        XCTAssertEqual(empty.availability, .unavailable, "readable but no value is unreadable")
        XCTAssertEqual(FocusStatusReading(availability: .available, isFocused: true).isFocused, true)
    }

    /// The test runner is not signed with the entitlement, and neither is
    /// any build without a provisioning profile: the production provider
    /// must say so rather than pass on the system's unread answer.
    func testTheUnsignedTestRunnerHasNoCommunicationNotificationsEntitlement() {
        XCTAssertFalse(FocusStatusEntitlement.isPresentInThisProcess())
        XCTAssertEqual(
            FocusStatusEntitlement.communicationNotifications,
            "com.apple.developer.usernotifications.communication"
        )
        let reading = INFocusStatusProvider().currentReading()
        XCTAssertEqual(reading.availability, .unavailable)
        XCTAssertNil(reading.isFocused)
    }

    /// The launch self-check is one line of fixed tokens: values only.
    func testTheLaunchSelfCheckLogsValuesOnly() {
        let unentitled = FocusStatusSelfCheck(
            authorization: .authorized,
            isFocused: false,
            entitled: false,
            availability: .unavailable
        )
        XCTAssertEqual(
            unentitled.line,
            "focus_status_self_check authorization_status=authorized is_focused=false "
                + "communication_notifications_entitlement=absent availability=unavailable"
        )
        let unanswered = FocusStatusSelfCheck(
            authorization: .notDetermined,
            isFocused: nil,
            entitled: true,
            availability: .notDetermined
        )
        XCTAssertEqual(
            unanswered.line,
            "focus_status_self_check authorization_status=not_determined is_focused=nil "
                + "communication_notifications_entitlement=present availability=not_determined"
        )
        XCTAssertEqual(FocusStatusSelfCheck.token(for: .denied), "denied")
        XCTAssertEqual(FocusStatusSelfCheck.token(for: .restricted), "restricted")
    }

    private func edges(_ client: FakeIPCClient) -> [Bool] {
        client.sentMessages.compactMap { message -> Bool? in
            guard case .focusStateChanged(let transition) = message else { return nil }
            return transition.active
        }
    }

    private func availabilities(_ client: FakeIPCClient) -> [FocusStatusAvailability] {
        client.sentMessages.compactMap { message -> FocusStatusAvailability? in
            guard case .focusStatusAvailability(let report) = message else { return nil }
            return report.availability
        }
    }

    func testTransitionCarriesOnlyCoarseFields() async throws {
        let client = FakeIPCClient()
        let provider = FakeFocusStatusProvider(initial: true)
        let observer = makeObserver(provider: provider, client: client)
        observer.sample(forceReport: false)
        await observer.inFlightTask?.value

        let message = try XCTUnwrap(
            client.sentMessages.first { message in
                if case .focusStateChanged = message { return true }
                return false
            })
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
