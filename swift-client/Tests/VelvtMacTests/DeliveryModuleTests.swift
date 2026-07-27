import Combine
import XCTest
@testable import VelvtMac

final class DeliveryModuleTests: XCTestCase {
    func testScaffoldTargetIsWired() {
        XCTAssertTrue(true)
    }

    @MainActor
    func testSendAllNowRequestsFlushAndWaitsForFlushStatus() async {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)

        sut.sendAllNow()
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(client.sentMessages, [.flushUploadQueue])
    }

    @MainActor
    func testUploadFlushFailureIsPublishedForTheMenu() async {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)

        messages.send(.errorResponse(ErrorResponse(
            code: "upload_flush_failed",
            message: "Unable to send queued events.",
            relatedEventID: nil
        )))
        await Task.yield()

        XCTAssertEqual(sut.sendError, "Unable to send queued events.")
    }

    @MainActor
    func testCorrectionPickerSendsEventAndStableIdentifiers() async {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)
        let eventID = UUID()
        let event = QueuedEventSummary(
            eventID: eventID,
            stableID: "abs_safe",
            label: "unlogged",
            localLabel: "Browser",
            category: "UNLOGGED",
            classificationTier: "fallback",
            classificationStatus: .unclassified,
            classificationConfidence: .none,
            classificationSource: .fallback,
            occurredAt: Date()
        )

        sut.correct(event, category: "COMMUNICATION", localActivityName: "Client messages")
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(
            client.sentMessages,
            [.correctEventClassification(.init(
                eventID: eventID,
                stableID: "abs_safe",
                category: "COMMUNICATION",
                localActivityName: "Client messages"
            )),
            .requestCorrectionHistory(.init(query: nil, offset: 0))]
        )
    }

    func testQueuedEventPresentationNamesBothSpecificActivityAndCategory() {
        let event = QueuedEventSummary(
            eventID: UUID(),
            stableID: "abs_safe",
            label: "video:youtube",
            localLabel: "YouTube",
            category: "PASSIVE_CONSUMPTION",
            classificationTier: "local_purpose_heuristic",
            classificationStatus: .classified,
            classificationConfidence: .high,
            classificationSource: .heuristic,
            occurredAt: Date()
        )

        XCTAssertEqual(QueuedEventPresentation.activity(event), "YouTube")
        XCTAssertEqual(QueuedEventPresentation.category(event), "Passive Consumption")
    }

    func testQueuedEventPresentationFallsBackToSafeActivityType() {
        let event = QueuedEventSummary(
            eventID: UUID(),
            stableID: "abs_safe",
            label: "reference:stack_overflow",
            localLabel: nil,
            category: "REFERENCE",
            classificationTier: "local_purpose_heuristic",
            classificationStatus: .classified,
            classificationConfidence: .medium,
            classificationSource: .heuristic,
            occurredAt: Date()
        )

        XCTAssertEqual(QueuedEventPresentation.activity(event), "Stack Overflow")
        XCTAssertEqual(QueuedEventPresentation.category(event), "Reference")
    }

    func testCorrectionHistoryPresentationNamesBothSpecificActivityAndCategory() {
        let correction = ClassificationCorrectionSummary(
            stableID: "abs_safe",
            label: "reference:inferred",
            localLabel: "Research reading",
            category: "REFERENCE",
            updatedAt: Date()
        )

        XCTAssertEqual(QueuedEventPresentation.activity(correction), "Research reading")
        XCTAssertEqual(QueuedEventPresentation.category(correction), "Reference")
    }

    @MainActor
    func testCorrectionCanBeUndoneAndAllLearningReset() async {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)
        let event = QueuedEventSummary(
            eventID: UUID(),
            stableID: "abs_safe",
            label: "communication:inferred",
            localLabel: "Slack",
            category: "COMMUNICATION",
            classificationTier: "exact_match",
            classificationStatus: .classified,
            classificationConfidence: .high,
            classificationSource: .userRule,
            occurredAt: Date()
        )

        sut.undoCorrection(event)
        sut.resetClassificationLearning()
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(
            client.sentMessages,
            [
                .removeClassificationOverride(.init(stableID: "abs_safe")),
                .requestCorrectionHistory(.init(query: nil, offset: 0)),
                .resetClassificationOverrides,
                .requestCorrectionHistory(.init(query: nil, offset: 0)),
            ]
        )
    }

    @MainActor
    func testCorrectionHistorySearchAndPaginationStayBounded() async {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)
        let items = (0..<20).map { index in
            ClassificationCorrectionSummary(
                stableID: "abs_\(index)",
                label: "reference:inferred",
                localLabel: "Local \(index)",
                category: "REFERENCE",
                updatedAt: Date()
            )
        }
        messages.send(.correctionHistoryPage(.init(
            items: items,
            offset: 0,
            pageSize: 20,
            totalCount: 41,
            hasMore: true
        )))
        await Task.yield()

        sut.refreshCorrectionHistory(query: "Research", offset: 0)
        sut.nextCorrectionHistoryPage()
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(
            client.sentMessages,
            [
                .requestCorrectionHistory(.init(query: "Research", offset: 0)),
                .requestCorrectionHistory(.init(query: "Research", offset: 20)),
            ]
        )
    }
}
