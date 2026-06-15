import XCTest
@testable import VelvtMac

final class IPCModuleTests: XCTestCase {
    private let encoder = IPCMessageCodec.makeEncoder()
    private let decoder = IPCMessageCodec.makeDecoder()

    func testEveryClientMessageRoundTrips() throws {
        let messages: [ClientMessage] = [
            .clientHello(ClientHello(protocolVersion: 2, clientVersion: "1.2.3")),
            .rawEvent(
                RawEventMessage(
                    eventID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
                    occurredAt: Date(timeIntervalSince1970: 1_700_000_000),
                    appName: "local-only",
                    windowTitle: "local-only",
                    bundleID: nil
                )
            ),
            .errorResponse(ErrorResponse(code: "safe_error", message: "safe", relatedEventID: nil))
        ]

        for message in messages {
            let data = try encoder.encode(message)
            XCTAssertEqual(try decoder.decode(ClientMessage.self, from: data), message)
        }
    }

    func testEveryServerMessageRoundTrips() throws {
        let eventID = UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!
        let messages: [ServerMessage] = [
            .serverHello(ServerHello(protocolVersion: 2)),
            .acknowledged(Acknowledged()),
            .versionMismatch(VersionMismatch(expected: 2, got: 1)),
            .rawEventAck(RawEventAcknowledgement(eventID: eventID, status: .accepted, dropReason: nil)),
            .insightPayload(
                InsightPayload(
                    date: "2026-06-13",
                    text: "ready-to-display",
                    confidenceLevel: .high,
                    lowConfidence: false,
                    generatedAt: Date(timeIntervalSince1970: 1_700_000_000)
                )
            ),
            .historyPayload(
                HistoryPayload(
                    days: 1,
                    summaries: [
                        DailySummary(
                            date: "2026-06-13",
                            status: .ready,
                            eventCount: 1,
                            focusScore: 0.8,
                            fragmentationScore: nil,
                            confidenceLevel: .medium,
                            activeSeconds: 60
                        )
                    ]
                )
            ),
            .serviceStatus(ServiceStatus(state: .ready, reason: nil)),
            .errorResponse(ErrorResponse(code: "safe_error", message: "safe", relatedEventID: nil)),
            .unknown(type: "future_message")
        ]

        for message in messages {
            let data = try encoder.encode(message)
            XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), message)
        }
    }

    func testTaggedEnumUsesTypeDiscriminator() throws {
        let data = try encoder.encode(ClientMessage.clientHello(ClientHello(protocolVersion: 2, clientVersion: "1.2.3")))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

        XCTAssertEqual(object["type"] as? String, "client_hello")
        XCTAssertEqual(object["protocol_version"] as? Int, 2)
    }

    func testDecoderRejectsUndeclaredFields() {
        let data = Data(#"{"type":"server_hello","protocol_version":2,"raw_title":"forbidden"}"#.utf8)

        XCTAssertThrowsError(try decoder.decode(ServerMessage.self, from: data))
    }

    func testUnknownServerMessageDecodesWithoutPayloadValues() throws {
        let data = Data(#"{"type":"future_message","raw_title":"must-not-be-retained"}"#.utf8)

        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), .unknown(type: "future_message"))
    }

    func testFakeClientDispatchesInjectedMessage() async throws {
        let client = FakeIPCClient()
        let message = ServerMessage.serviceStatus(ServiceStatus(state: .ready, reason: nil))
        let received = expectation(description: "received message")

        Task {
            for await incoming in client.incomingMessages {
                XCTAssertEqual(incoming, message)
                received.fulfill()
                break
            }
        }

        client.inject(message)
        await fulfillment(of: [received], timeout: 1)
    }

    func testBackoffDoublesAndCaps() {
        let backoff = ReconnectBackoff(maximumDelay: 4, jitter: { 1 })

        XCTAssertEqual(backoff.delay(forAttempt: 1), 1)
        XCTAssertEqual(backoff.delay(forAttempt: 2), 2)
        XCTAssertEqual(backoff.delay(forAttempt: 3), 4)
        XCTAssertEqual(backoff.delay(forAttempt: 4), 4)
    }

    func testBackoffJitterStaysWithinTwentyPercent() {
        XCTAssertEqual(ReconnectBackoff(jitter: { 0.8 }).delay(forAttempt: 3), 3.2, accuracy: 0.0001)
        XCTAssertEqual(ReconnectBackoff(jitter: { 1.2 }).delay(forAttempt: 3), 4.8, accuracy: 0.0001)
    }
}
