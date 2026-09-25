import Foundation
import XCTest

@testable import VelvtMac

/// Covers the facts the client reads off an application's own `Info.plist` and
/// the bounds it applies before any of them reach the socket. Nothing here
/// asserts a category, a purpose, or a confidence: Swift decides none of those.
final class DeclaredAppMetadataTests: XCTestCase {
    private var temporaryDirectory: URL!

    override func setUpWithError() throws {
        temporaryDirectory = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("velvt-declared-metadata-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: temporaryDirectory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: temporaryDirectory)
        temporaryDirectory = nil
    }

    // MARK: - Caching

    func testPlistIsReadOncePerBundleIdentifierForTheProcessLifetime() {
        let loader = SpyPropertyListLoader(
            plist: [
                "LSApplicationCategoryType": "public.app-category.developer-tools"
            ])
        let provider = BundleInfoPlistMetadataProvider(loadPropertyList: loader.load)
        let application = Self.application(bundleIdentifier: "com.microsoft.VSCode")

        let first = provider.metadata(for: application)
        let second = provider.metadata(for: application)

        XCTAssertEqual(first, second)
        XCTAssertEqual(
            loader.loadCount, 1,
            "A second lookup for the same application must come from the cache.")
    }

    func testCacheIsConsultedOnASecondEventForTheSameApplication() throws {
        let loader = SpyPropertyListLoader(
            plist: [
                "LSApplicationCategoryType": "public.app-category.developer-tools",
                "CFBundleDocumentTypes": [
                    ["LSItemContentTypes": ["public.source-code"]]
                ],
            ])
        let provider = CountingMetadataProvider(
            wrapped: BundleInfoPlistMetadataProvider(loadPropertyList: loader.load))
        let sink = RecordingSink()
        let workspace = StubWorkspaceObserver()
        let accessibility = StubAccessibilityObserver(initialTitle: "first")
        let agent = Self.agent(
            sink: sink, workspace: workspace, accessibility: accessibility, provider: provider)

        try agent.start()
        workspace.activate(Self.application(bundleIdentifier: "com.microsoft.VSCode"))
        accessibility.emit(title: "second")

        XCTAssertEqual(provider.callCount, 2, "Every event asks for the metadata.")
        XCTAssertEqual(loader.loadCount, 1, "Only the first event reads the plist.")
        XCTAssertEqual(sink.events.count, 1)
        XCTAssertEqual(
            sink.events[0].declaredAppCategory, "public.app-category.developer-tools")
        XCTAssertEqual(sink.events[0].documentTypeIDs, ["public.source-code"])
        // The agent's own `deinit` flushes the dwell that is still open, which
        // would add a second event: keep it alive until the assertions are done.
        withExtendedLifetime(agent) {}
    }

    func testUnreadablePlistIsCachedSoItIsNotRetriedOnEveryEvent() {
        let loader = SpyPropertyListLoader(plist: nil)
        let provider = BundleInfoPlistMetadataProvider(loadPropertyList: loader.load)
        let application = Self.application(bundleIdentifier: "com.example.Sandboxed")

        for _ in 0..<5 {
            XCTAssertEqual(provider.metadata(for: application), .absent)
        }

        XCTAssertEqual(loader.loadCount, 1)
    }

    func testApplicationWithoutABundleIdentifierIsNeverRead() {
        let loader = SpyPropertyListLoader(plist: ["LSApplicationCategoryType": "public.app-category.music"])
        let provider = BundleInfoPlistMetadataProvider(loadPropertyList: loader.load)

        let metadata = provider.metadata(
            for: RunningApplication(
                processIdentifier: 42,
                appName: "Unbundled",
                bundleIdentifier: nil,
                bundleURL: temporaryDirectory.appendingPathComponent("Unbundled.app")))

        XCTAssertEqual(metadata, .absent)
        XCTAssertEqual(
            loader.loadCount, 0,
            "With no cache key there is nothing to key a later lookup on, so nothing is read.")
    }

    // MARK: - Degrading silently

    func testMissingBundleURLProducesExactlyTodaysEvent() throws {
        let sink = RecordingSink()
        let workspace = StubWorkspaceObserver()
        let accessibility = StubAccessibilityObserver(initialTitle: "Draft")
        let dates = DateSequence([
            Date(timeIntervalSince1970: 0),
            Date(timeIntervalSince1970: 30),
            Date(timeIntervalSince1970: 60),
        ])
        let agent = Self.agent(
            sink: sink,
            workspace: workspace,
            accessibility: accessibility,
            provider: BundleInfoPlistMetadataProvider(loadPropertyList: { _ in
                XCTFail("No bundle URL means no file is touched.")
                return nil
            }),
            now: dates.next)

        try agent.start()
        workspace.activate(
            RunningApplication(
                processIdentifier: 10,
                appName: "Editor",
                bundleIdentifier: "com.example.Editor",
                bundleURL: nil))
        accessibility.emit(title: "Draft 2")

        XCTAssertEqual(
            sink.events,
            [
                RawEvent(
                    appName: "Editor",
                    bundleIdentifier: "com.example.Editor",
                    windowTitle: "Draft",
                    occurredAt: Date(timeIntervalSince1970: 0),
                    durationSeconds: 30
                )
            ],
            "Absent metadata must produce the event this agent produced before the field existed.")
        withExtendedLifetime(agent) {}
    }

    func testMalformedPropertyListFileDegradesToAbsent() throws {
        let bundleURL = try writeBundle(named: "Garbage.app", infoPlistBytes: Data("not a plist at all".utf8))

        XCTAssertNil(
            BundleInfoPlistMetadataProvider.readPropertyList(
                at: bundleURL.appendingPathComponent("Contents/Info.plist")))

        let provider = BundleInfoPlistMetadataProvider()
        XCTAssertEqual(
            provider.metadata(
                for: Self.application(bundleIdentifier: "com.example.Garbage", bundleURL: bundleURL)),
            .absent)
    }

    func testMissingPropertyListFileDegradesToAbsent() throws {
        let bundleURL = temporaryDirectory.appendingPathComponent("Absent.app")
        try FileManager.default.createDirectory(at: bundleURL, withIntermediateDirectories: true)
        let provider = BundleInfoPlistMetadataProvider()

        XCTAssertEqual(
            provider.metadata(
                for: Self.application(bundleIdentifier: "com.example.Absent", bundleURL: bundleURL)),
            .absent)
    }

    func testPlistWithoutTheDeclaredKeysDegradesToAbsent() throws {
        let bundleURL = try writeBundle(
            named: "Bare.app", plist: ["CFBundleIdentifier": "com.example.Bare"])
        let provider = BundleInfoPlistMetadataProvider()

        XCTAssertEqual(
            provider.metadata(
                for: Self.application(bundleIdentifier: "com.example.Bare", bundleURL: bundleURL)),
            .absent)
    }

    func testDeclaredKeysOfTheWrongShapeAreIgnoredRatherThanTrusted() {
        let provider = BundleInfoPlistMetadataProvider(loadPropertyList: { _ in
            [
                // A number where a string belongs, a dictionary where an array
                // belongs, and a nested value that is not a string: a third
                // party's file that does not match its own documented shape is a
                // file we know nothing from.
                "LSApplicationCategoryType": 17,
                "CFBundleDocumentTypes": ["LSItemContentTypes": ["public.source-code"]],
            ]
        })

        XCTAssertEqual(
            provider.metadata(for: Self.application(bundleIdentifier: "com.example.Wrong")),
            .absent)
    }

    func testNonStringContentTypeEntriesAreSkippedWithoutLosingTheRest() {
        let provider = BundleInfoPlistMetadataProvider(loadPropertyList: { _ in
            [
                "CFBundleDocumentTypes": [
                    ["LSItemContentTypes": ["public.source-code", 12, ["nested"]]],
                    ["CFBundleTypeName": "no content types here"],
                ]
            ]
        })

        XCTAssertEqual(
            provider.metadata(for: Self.application(bundleIdentifier: "com.example.Mixed"))
                .documentTypeIDs,
            ["public.source-code"])
    }

    func testWhitespaceOnlyDeclaredCategoryIsReportedAsAbsent() {
        let provider = BundleInfoPlistMetadataProvider(loadPropertyList: { _ in
            ["LSApplicationCategoryType": "   "]
        })

        XCTAssertNil(
            provider.metadata(for: Self.application(bundleIdentifier: "com.example.Blank"))
                .declaredAppCategory)
    }

    func testDeclaredCategoryIsReportedVerbatimWithoutJudgement() throws {
        let bundleURL = try writeBundle(
            named: "Terminal.app",
            plist: ["LSApplicationCategoryType": "public.app-category.utilities"])
        let provider = BundleInfoPlistMetadataProvider()

        // `utilities` is a value the service refuses to draw any conclusion
        // from. The client still reports it: deciding is Rust's job.
        XCTAssertEqual(
            provider.metadata(
                for: Self.application(bundleIdentifier: "com.apple.Terminal", bundleURL: bundleURL)
            ).declaredAppCategory,
            "public.app-category.utilities")
    }

    // MARK: - Bounds

    func testDocumentTypesAreFlattenedDeduplicatedAndSorted() throws {
        let bundleURL = try writeBundle(
            named: "Editor.app",
            plist: [
                "CFBundleDocumentTypes": [
                    ["LSItemContentTypes": ["public.source-code", "public.plain-text"]],
                    ["LSItemContentTypes": ["public.plain-text", "com.adobe.pdf"]],
                ]
            ])
        let provider = BundleInfoPlistMetadataProvider()

        XCTAssertEqual(
            provider.metadata(
                for: Self.application(bundleIdentifier: "com.example.Editor", bundleURL: bundleURL)
            ).documentTypeIDs,
            ["com.adobe.pdf", "public.plain-text", "public.source-code"])
    }

    func testExactlyTheMaximumNumberOfDeclaredTypesIsStillReported() {
        let declared = (0..<DeclaredDocumentTypeBounds.maximumCount).map { "public.type-\($0)" }

        XCTAssertEqual(
            DeclaredDocumentTypeBounds.representable(declared).count,
            DeclaredDocumentTypeBounds.maximumCount)
    }

    func testMoreDeclaredTypesThanTheProtocolCarriesReportsNothingRatherThanAPrefix() {
        let declared = (0...DeclaredDocumentTypeBounds.maximumCount).map { "public.type-\($0)" }

        XCTAssertEqual(declared.count, DeclaredDocumentTypeBounds.maximumCount + 1)
        XCTAssertEqual(
            DeclaredDocumentTypeBounds.representable(declared), [],
            "A prefix is a set the application never declared; abstaining is honest.")
    }

    func testDuplicatesDoNotCountTowardTheMaximum() {
        let declared =
            (0..<DeclaredDocumentTypeBounds.maximumCount).map { "public.type-\($0)" }
            + (0..<DeclaredDocumentTypeBounds.maximumCount).map { "public.type-\($0)" }

        XCTAssertEqual(
            DeclaredDocumentTypeBounds.representable(declared).count,
            DeclaredDocumentTypeBounds.maximumCount)
    }

    func testIdentifierAtTheLengthLimitIsKeptAndOneByteOverAbstainsFromTheWholeSet() {
        let atLimit = String(repeating: "a", count: DeclaredDocumentTypeBounds.maximumIdentifierLength)
        let overLimit = atLimit + "a"

        XCTAssertEqual(DeclaredDocumentTypeBounds.representable([atLimit]), [atLimit])
        XCTAssertEqual(
            DeclaredDocumentTypeBounds.representable(["public.source-code", overLimit]), [],
            "The service refuses an over-long identifier and the refusal costs the whole event.")
    }

    func testLengthLimitCountsUTF8BytesTheWayTheServiceDoes() {
        // 33 two-byte scalars is 33 characters and 66 bytes: over the bound the
        // service checks, which counts bytes.
        let multibyte = String(repeating: "é", count: 33)

        XCTAssertLessThan(
            multibyte.count, DeclaredDocumentTypeBounds.maximumIdentifierLength)
        XCTAssertEqual(DeclaredDocumentTypeBounds.representable([multibyte]), [])
    }

    func testEmptyAndWhitespaceOnlyIdentifiersAreDroppedWithoutSilencingTheSet() {
        XCTAssertEqual(
            DeclaredDocumentTypeBounds.representable(["", "  ", " public.source-code "]),
            ["public.source-code"])
    }

    // MARK: - The IPC boundary

    func testRawEventMessageAppliesTheSameBoundAsCollection() {
        let tooMany = (0...DeclaredDocumentTypeBounds.maximumCount).map { "public.type-\($0)" }

        let message = RawEventMessage(
            eventID: UUID(),
            occurredAt: Date(timeIntervalSince1970: 0),
            appName: "Xcode",
            windowTitle: "velvt",
            bundleID: "com.apple.dt.Xcode",
            declaredAppCategory: "public.app-category.developer-tools",
            documentTypeIDs: tooMany)

        XCTAssertEqual(message.documentTypeIDs, [])
        XCTAssertEqual(message.declaredAppCategory, "public.app-category.developer-tools")
    }

    func testRawEventMessageRoundTripsDeclaredMetadataOverTheWire() throws {
        let message = RawEventMessage(
            eventID: UUID(),
            occurredAt: Date(timeIntervalSince1970: 1_700_000_000),
            durationSeconds: 12,
            appName: "Code",
            windowTitle: "IPCTypes.swift",
            bundleID: "com.microsoft.VSCode",
            declaredAppCategory: "public.app-category.developer-tools",
            documentTypeIDs: ["public.source-code", "public.folder"])

        let data = try IPCMessageCodec.makeEncoder().encode(ClientMessage.rawEvent(message))
        let json = try XCTUnwrap(String(data: data, encoding: .utf8))
        XCTAssertTrue(json.contains("\"declared_app_category\""))
        XCTAssertTrue(json.contains("\"document_type_ids\""))

        let decoded = try IPCMessageCodec.makeDecoder().decode(ClientMessage.self, from: data)
        guard case let .rawEvent(roundTripped) = decoded else {
            return XCTFail("Expected a raw event.")
        }
        XCTAssertEqual(roundTripped, message)
        XCTAssertEqual(roundTripped.documentTypeIDs, ["public.folder", "public.source-code"])
    }

    func testRawEventWithNoDeclaredMetadataEmitsTheSameFrameAsBeforeTheFieldsExisted() throws {
        let eventID = UUID()
        let message = RawEventMessage(
            eventID: eventID,
            occurredAt: Date(timeIntervalSince1970: 1_700_000_000),
            appName: "Editor",
            windowTitle: "Draft",
            bundleID: nil)

        let data = try IPCMessageCodec.makeEncoder().encode(ClientMessage.rawEvent(message))
        let json = try XCTUnwrap(String(data: data, encoding: .utf8))

        XCTAssertFalse(json.contains("declared_app_category"))
        XCTAssertFalse(json.contains("document_type_ids"))
    }

    func testCollectedMetadataSurvivesTheRelayIntoTheRawEventFrame() async throws {
        let client = FakeIPCClient()
        let relay = EventRelay(ipcClient: client, capacity: 10)
        await relay.start()
        try? await Task.sleep(for: .milliseconds(100))
        await relay.connectionDidChange(to: .connected)

        relay.receive(
            RawEvent(
                appName: "Code",
                bundleIdentifier: "com.microsoft.VSCode",
                declaredAppCategory: "public.app-category.developer-tools",
                documentTypeIDs: ["public.source-code"],
                windowTitle: "IPCTypes.swift",
                occurredAt: Date(timeIntervalSince1970: 1)))
        try? await Task.sleep(for: .milliseconds(100))

        let sent = client.sentMessages.compactMap { message -> RawEventMessage? in
            if case let .rawEvent(raw) = message { return raw }
            return nil
        }
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(sent.first?.declaredAppCategory, "public.app-category.developer-tools")
        XCTAssertEqual(sent.first?.documentTypeIDs, ["public.source-code"])
        await relay.stop()
    }

    // MARK: - Triage messages

    func testTriageMessagesRoundTrip() throws {
        let encoder = IPCMessageCodec.makeEncoder()
        let decoder = IPCMessageCodec.makeDecoder()

        let request = ClientMessage.requestUnclassifiedTriage(
            RequestUnclassifiedTriage(lookbackDays: 7))
        let requestData = try encoder.encode(request)
        XCTAssertTrue(
            try XCTUnwrap(String(data: requestData, encoding: .utf8))
                .contains("\"request_unclassified_triage\""))
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: requestData), request)

        let answer = ClientMessage.setApplicationCategory(
            SetApplicationCategory(
                appStableID: "app_key_hash", category: "FOCUS_WORK", activityName: "Editing"))
        let answerData = try encoder.encode(answer)
        XCTAssertTrue(
            try XCTUnwrap(String(data: answerData, encoding: .utf8))
                .contains("\"set_application_category\""))
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: answerData), answer)

        let triage = ServerMessage.unclassifiedTriage(
            UnclassifiedTriage(
                entries: [
                    UnclassifiedTriageEntry(
                        appStableID: "app_key_hash",
                        displayName: "Code",
                        secondsObserved: 4 * 3600,
                        eventCount: 12)
                ],
                windowDays: 7))
        let triageData = try encoder.encode(triage)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: triageData), triage)
    }

    /// The entry carries exactly the keys `proto/schema/unclassified_triage.json`
    /// declares. Until 2026-09-25 the schema and this type both had an optional
    /// `bundle_id` that the Rust type omits and no Rust build ever sent.
    func testTriageEntryEncodesExactlyTheSchemaKeys() throws {
        let schemaURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("proto/schema/unclassified_triage.json")
        var node: Any? = try JSONSerialization.jsonObject(with: Data(contentsOf: schemaURL))
        for key in ["properties", "payload", "properties", "entries", "items", "properties"] {
            node = (node as? [String: Any])?[key]
        }
        let properties = try XCTUnwrap(node as? [String: Any])
        let entry = UnclassifiedTriageEntry(
            appStableID: "app_key_hash", displayName: "Code", secondsObserved: 600, eventCount: 3)
        let encoded = try XCTUnwrap(
            JSONSerialization.jsonObject(with: IPCMessageCodec.makeEncoder().encode(entry))
                as? [String: Any])
        XCTAssertEqual(Set(encoded.keys), Set(properties.keys))
    }

    func testTriageLookbackIsClampedToTheRetentionWindow() {
        XCTAssertEqual(RequestUnclassifiedTriage(lookbackDays: 90).lookbackDays, 14)
        XCTAssertEqual(RequestUnclassifiedTriage(lookbackDays: 0).lookbackDays, 1)
        XCTAssertEqual(RequestUnclassifiedTriage(lookbackDays: 7).lookbackDays, 7)
    }

    func testCorrectionScopeDefaultsToWindowWhenTheServiceOmitsIt() throws {
        let json = """
            {"stable_id":"abs","label":"reference:inferred","category":"REFERENCE",
             "updated_at":"2026-09-23T10:00:00Z"}
            """
        let summary = try IPCMessageCodec.makeDecoder().decode(
            ClassificationCorrectionSummary.self, from: Data(json.utf8))

        XCTAssertEqual(summary.scope, .window)
    }

    func testAppScopedRuleDecodesItsScope() throws {
        let json = """
            {"stable_id":"app_key_hash","label":"focus_work:app","category":"FOCUS_WORK",
             "updated_at":"2026-09-23T10:00:00Z","scope":"app"}
            """
        let summary = try IPCMessageCodec.makeDecoder().decode(
            ClassificationCorrectionSummary.self, from: Data(json.utf8))

        XCTAssertEqual(summary.scope, .app)
    }

    func testDeclaredClassificationSourcesDecode() throws {
        let decoder = IPCMessageCodec.makeDecoder()

        // Decoded inside an array: a bare string is a top-level JSON fragment,
        // which is not what the service ever sends and not what Foundation
        // reliably accepts.
        XCTAssertEqual(
            try decoder.decode(
                [ClassificationSource].self,
                from: Data("[\"declared_document_types\",\"declared_app_category\"]".utf8)),
            [.declaredDocumentTypes, .declaredAppCategory])
    }

    // MARK: - Helpers

    private static func application(
        bundleIdentifier: String?,
        bundleURL: URL? = URL(fileURLWithPath: "/Applications/Stub.app")
    ) -> RunningApplication {
        RunningApplication(
            processIdentifier: 10,
            appName: "Stub",
            bundleIdentifier: bundleIdentifier,
            bundleURL: bundleURL)
    }

    private static func agent(
        sink: RecordingSink,
        workspace: StubWorkspaceObserver,
        accessibility: StubAccessibilityObserver,
        provider: any DeclaredAppMetadataReading,
        now: @escaping () -> Date = Date.init
    ) -> AXCollectionAgent {
        AXCollectionAgent(
            eventSink: sink,
            permissionChecker: AlwaysTrustedPermissionChecker(),
            workspaceObserver: workspace,
            accessibilityObserver: accessibility,
            metadataProvider: provider,
            now: now)
    }

    private func writeBundle(named name: String, plist: [String: Any]) throws -> URL {
        let data = try PropertyListSerialization.data(
            fromPropertyList: plist, format: .xml, options: 0)
        return try writeBundle(named: name, infoPlistBytes: data)
    }

    private func writeBundle(named name: String, infoPlistBytes: Data) throws -> URL {
        let bundleURL = temporaryDirectory.appendingPathComponent(name)
        let contentsURL = bundleURL.appendingPathComponent("Contents")
        try FileManager.default.createDirectory(at: contentsURL, withIntermediateDirectories: true)
        try infoPlistBytes.write(to: contentsURL.appendingPathComponent("Info.plist"))
        return bundleURL
    }
}

private final class SpyPropertyListLoader {
    private let plist: [String: Any]?
    private let lock = NSLock()
    private var count = 0

    var loadCount: Int { lock.withLock { count } }

    init(plist: [String: Any]?) {
        self.plist = plist
    }

    func load(_ url: URL) -> [String: Any]? {
        lock.withLock { count += 1 }
        return plist
    }
}

private final class CountingMetadataProvider: DeclaredAppMetadataReading {
    private let wrapped: any DeclaredAppMetadataReading
    private(set) var callCount = 0

    init(wrapped: any DeclaredAppMetadataReading) {
        self.wrapped = wrapped
    }

    func metadata(for application: RunningApplication) -> DeclaredAppMetadata {
        callCount += 1
        return wrapped.metadata(for: application)
    }
}

private final class RecordingSink: EventSink {
    private(set) var events: [RawEvent] = []

    func receive(_ event: RawEvent) {
        events.append(event)
    }
}

private final class AlwaysTrustedPermissionChecker: AccessibilityPermissionChecking {
    func hasPermission() -> Bool { true }
}

private final class StubWorkspaceObserver: WorkspaceActivationObserving {
    private var handler: ((RunningApplication) -> Void)?

    func start(activationHandler: @escaping (RunningApplication) -> Void) -> RunningApplication? {
        handler = activationHandler
        return nil
    }

    func stop() {
        handler = nil
    }

    func activate(_ application: RunningApplication) {
        handler?(application)
    }
}

private final class StubAccessibilityObserver: AccessibilityObserving {
    private let initialTitle: String
    private var activityHandler: ((FocusedActivity) -> Void)?

    init(initialTitle: String) {
        self.initialTitle = initialTitle
    }

    func start(
        observing application: RunningApplication,
        activityHandler: @escaping (FocusedActivity) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> FocusedActivity {
        self.activityHandler = activityHandler
        return FocusedActivity(windowTitle: initialTitle)
    }

    func stop() {
        activityHandler = nil
    }

    func emit(title: String?) {
        activityHandler?(FocusedActivity(windowTitle: title))
    }
}

private final class DateSequence {
    private var dates: [Date]

    init(_ dates: [Date]) {
        self.dates = dates
    }

    func next() -> Date {
        dates.isEmpty ? Date(timeIntervalSince1970: 0) : dates.removeFirst()
    }
}
