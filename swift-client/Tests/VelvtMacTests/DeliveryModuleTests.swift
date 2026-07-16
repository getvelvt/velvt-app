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

        sut.correct(event, category: "COMMUNICATION")
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(
            client.sentMessages,
            [.correctEventClassification(.init(
                eventID: eventID,
                stableID: "abs_safe",
                category: "COMMUNICATION"
            ))]
        )
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
                .resetClassificationOverrides,
            ]
        )
    }
}
