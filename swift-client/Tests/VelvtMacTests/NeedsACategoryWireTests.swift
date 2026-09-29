import Foundation
import XCTest

@testable import VelvtMac

/// Protocol 33 on the Swift side of the socket: sites on the needs-a-category
/// list, `set_site_category`, the site rule scope, and the card-and-reminder
/// messages.
///
/// A value this client cannot decode inside a message it knows drops the
/// connection (`UnixSocketIPCClient`), so every message here is checked against
/// the schema file the Rust service is checked against: the keys it writes,
/// and the values it must accept.
final class NeedsACategoryWireTests: XCTestCase {
    private let appKey = String(repeating: "a", count: 64)
    private let siteKey = String(repeating: "b", count: 64)
    private let promptID = String(repeating: "c", count: 64)

    // MARK: - unclassified_triage

    /// Byte for byte the entry pair the Rust service's own test emits.
    func testTheServicesTriageShapeDecodesWithASiteAndAnUnnamedApplication() throws {
        let json = """
            {"type":"unclassified_triage","payload":{"entries":[\
            {"kind":"site","stable_id":"\(siteKey)","display_name":"PRIVATE_HOST.example",\
            "seconds_observed":900,"event_count":4},\
            {"kind":"application","stable_id":"\(appKey)","display_name":null,\
            "seconds_observed":600,"event_count":2}],"window_days":7}}
            """
        let message = try IPCMessageCodec.makeDecoder().decode(ServerMessage.self, from: Data(json.utf8))

        guard case .unclassifiedTriage(let triage) = message else {
            return XCTFail("expected unclassified_triage, got \(message.safeLogDescription)")
        }
        XCTAssertEqual(triage.windowDays, 7)
        XCTAssertEqual(
            triage.entries,
            [
                UnclassifiedTriageEntry(
                    kind: .site, stableID: siteKey, displayName: "PRIVATE_HOST.example",
                    secondsObserved: 900, eventCount: 4),
                UnclassifiedTriageEntry(
                    kind: .application, stableID: appKey, displayName: nil,
                    secondsObserved: 600, eventCount: 2),
            ])
        XCTAssertNotEqual(triage.entries[0].id, triage.entries[1].id)
    }

    func testTheEntryKindAcceptsExactlyTheSchemasValues() throws {
        let values = try schemaEnum(
            "unclassified_triage",
            ["properties", "payload", "properties", "entries", "items", "properties", "kind"])
        XCTAssertEqual(Set(values), ["application", "site"])
        for value in values {
            XCTAssertNotNil(UnclassifiedTriageEntryKind(rawValue: value), value)
        }
    }

    /// The old shape is refused rather than half-read: a v32 entry has no
    /// kind, and guessing one would send a site's answer as an app's.
    func testAProtocol32EntryDoesNotDecode() {
        let json = """
            {"app_stable_id":"\(appKey)","display_name":"Code","seconds_observed":600,"event_count":1}
            """
        XCTAssertThrowsError(
            try IPCMessageCodec.makeDecoder().decode(UnclassifiedTriageEntry.self, from: Data(json.utf8)))
    }

    // MARK: - set_site_category

    func testSetSiteCategoryRoundTripsAndWritesTheSchemasKeys() throws {
        let properties = try schemaPayloadProperties("set_site_category")
        let named = ClientMessage.setSiteCategory(
            SetSiteCategory(siteStableID: siteKey, category: "REFERENCE", activityName: "Team wiki"))
        let bare = ClientMessage.setSiteCategory(SetSiteCategory(siteStableID: siteKey, category: "REFERENCE"))

        XCTAssertEqual(try payloadKeys(of: named), Set(properties.keys))
        // No name is no key: the hostname is never filled in for one.
        XCTAssertEqual(try payloadKeys(of: bare), ["site_stable_id", "category"])
        XCTAssertEqual(try envelopeType(of: bare), "set_site_category")
        for message in [named, bare] {
            XCTAssertEqual(try roundTrip(message), message)
        }
    }

    func testTheSiteCategoryAcceptsTheSameCategoriesAsTheApplicationOne() throws {
        let site = try schemaEnum("set_site_category", ["properties", "payload", "properties", "category"])
        let application = try schemaEnum(
            "set_application_category", ["properties", "payload", "properties", "category"])
        XCTAssertEqual(site, application)
        XCTAssertTrue(Set(QueuedEventPresentation.teachableCategories).isSubset(of: Set(site)))
    }

    // MARK: - request_category_prompt

    func testRequestCategoryPromptWritesTheSchemasKeysAndClampsTheOffset() throws {
        let properties = try schemaPayloadProperties("request_category_prompt")
        let message = ClientMessage.requestCategoryPrompt(RequestCategoryPrompt(utcOffsetSeconds: -25_200))

        XCTAssertEqual(try payloadKeys(of: message), Set(properties.keys))
        XCTAssertEqual(try envelopeType(of: message), "request_category_prompt")
        XCTAssertEqual(try roundTrip(message), message)

        let offset = try XCTUnwrap(properties["utc_offset_seconds"] as? [String: Any])
        XCTAssertEqual(offset["minimum"] as? Int, RequestCategoryPrompt.utcOffsetRange.lowerBound)
        XCTAssertEqual(offset["maximum"] as? Int, RequestCategoryPrompt.utcOffsetRange.upperBound)
        XCTAssertEqual(RequestCategoryPrompt(utcOffsetSeconds: 90_000).utcOffsetSeconds, 64_800)
        XCTAssertEqual(RequestCategoryPrompt(utcOffsetSeconds: -90_000).utcOffsetSeconds, -64_800)
        XCTAssertEqual(RequestCategoryPrompt(utcOffsetSeconds: 19_800).utcOffsetSeconds, 19_800)
    }

    // MARK: - category_prompt

    func testCategoryPromptRoundTripsAndEveryObjectCarriesTheSchemasKeys() throws {
        let payload = try schemaPayloadProperties("category_prompt")
        let card = try XCTUnwrap((payload["card"] as? [String: Any])?["properties"] as? [String: Any])
        let notification = try XCTUnwrap(
            (payload["notification"] as? [String: Any])?["properties"] as? [String: Any])
        let prompt = fullPrompt()
        let message = ServerMessage.categoryPrompt(prompt)

        XCTAssertEqual(try payloadKeys(of: message), Set(payload.keys))
        let encoded = try payloadObject(of: message)
        XCTAssertEqual(Set(try XCTUnwrap(encoded["card"] as? [String: Any]).keys), Set(card.keys))
        XCTAssertEqual(
            Set(try XCTUnwrap(encoded["notification"] as? [String: Any]).keys), Set(notification.keys))
        XCTAssertEqual(try envelopeType(of: message), "category_prompt")
        XCTAssertEqual(try roundTrip(message), message)
        XCTAssertEqual(message.safeLogDescription, "category_prompt")
    }

    /// An empty payload is the service saying there is no card.
    func testAnEmptyCategoryPromptDecodesAsNoCard() throws {
        let message = try IPCMessageCodec.makeDecoder().decode(
            ServerMessage.self, from: Data(#"{"type":"category_prompt","payload":{}}"#.utf8))
        XCTAssertEqual(message, .categoryPrompt(CategoryPrompt()))
        XCTAssertEqual(try payloadKeys(of: message), [])
    }

    // MARK: - acknowledge_category_prompt

    func testAcknowledgeCategoryPromptWritesTheSchemasKeysAndResponses() throws {
        let properties = try schemaPayloadProperties("acknowledge_category_prompt")
        for response in [CategoryPromptResponse.opened, .notNow] {
            let message = ClientMessage.acknowledgeCategoryPrompt(
                AcknowledgeCategoryPrompt(promptID: promptID, response: response))
            XCTAssertEqual(try payloadKeys(of: message), Set(properties.keys))
            XCTAssertEqual(try envelopeType(of: message), "acknowledge_category_prompt")
            XCTAssertEqual(try roundTrip(message), message)
        }
        let responses = try schemaEnum(
            "acknowledge_category_prompt", ["properties", "payload", "properties", "response"])
        XCTAssertEqual(Set(responses), Set([CategoryPromptResponse.opened, .notNow].map(\.rawValue)))
    }

    // MARK: - Rule scope

    /// Both places a rule's scope crosses the socket accept `site` now, and
    /// the client decodes every value either lists.
    func testEveryRuleScopeTheSchemasListDecodes() throws {
        // This schema describes the payload alone, with no envelope.
        let history = try schemaEnum(
            "correction_history_page", ["properties", "items", "items", "properties", "scope"])
        let status = try schemaEnum(
            "menu_status",
            ["properties", "payload", "properties", "correction_history", "items", "properties", "scope"])
        XCTAssertEqual(Set(history), ["window", "app", "site"])
        XCTAssertEqual(Set(status), Set(history))
        for value in history {
            XCTAssertNotNil(CorrectionScope(rawValue: value), value)
        }

        let json = """
            {"stable_id":"\(siteKey)","label":"site","category":"REFERENCE",
             "updated_at":"2026-09-27T10:00:00Z","scope":"site"}
            """
        let rule = try IPCMessageCodec.makeDecoder().decode(
            ClassificationCorrectionSummary.self, from: Data(json.utf8))
        XCTAssertEqual(rule.scope, .site)
        XCTAssertNil(rule.localLabel)
    }

    // MARK: - Fixtures

    private func fullPrompt() -> CategoryPrompt {
        CategoryPrompt(
            promptID: promptID,
            card: CategoryPromptCard(
                title: "Needs a category",
                body:
                    "2 sites and 1 app you used this week don't have a category yet. Choose once and "
                    + "it covers every page of a site and every window of an app.",
                primaryAction: "Choose categories",
                secondaryAction: "Not now",
                entryCount: 3),
            notification: CategoryPromptNotification(
                title: "A few things need a category",
                body: "2 sites and 1 app you used this week don't have a category yet. Choose once in Velvt."))
    }

    private func roundTrip<Message: Codable & Equatable>(_ message: Message) throws -> Message {
        try IPCMessageCodec.makeDecoder().decode(
            Message.self, from: IPCMessageCodec.makeEncoder().encode(message))
    }

    private func envelope(of message: some Encodable) throws -> [String: Any] {
        try XCTUnwrap(
            JSONSerialization.jsonObject(with: IPCMessageCodec.makeEncoder().encode(message))
                as? [String: Any])
    }

    private func envelopeType(of message: some Encodable) throws -> String? {
        try envelope(of: message)["type"] as? String
    }

    private func payloadObject(of message: some Encodable) throws -> [String: Any] {
        try XCTUnwrap(try envelope(of: message)["payload"] as? [String: Any])
    }

    private func payloadKeys(of message: some Encodable) throws -> Set<String> {
        Set(try payloadObject(of: message).keys)
    }

    private func schema(_ name: String) throws -> Any {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("proto/schema/\(name).json")
        return try JSONSerialization.jsonObject(with: Data(contentsOf: url))
    }

    private func schemaNode(_ name: String, _ path: [String]) throws -> Any? {
        var node: Any? = try schema(name)
        for key in path {
            node = (node as? [String: Any])?[key]
        }
        return node
    }

    private func schemaPayloadProperties(_ name: String) throws -> [String: Any] {
        try XCTUnwrap(
            try schemaNode(name, ["properties", "payload", "properties"]) as? [String: Any],
            "\(name).json has no payload properties")
    }

    private func schemaEnum(_ name: String, _ path: [String]) throws -> [String] {
        let node = try XCTUnwrap(try schemaNode(name, path) as? [String: Any], "\(name).json: \(path)")
        return try XCTUnwrap(node["enum"] as? [String], "\(name).json: \(path) has no enum")
    }
}
