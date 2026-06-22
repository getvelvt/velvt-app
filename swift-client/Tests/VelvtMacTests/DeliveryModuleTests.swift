import Combine
import XCTest
@testable import VelvtMac

final class DeliveryModuleTests: XCTestCase {
    func testScaffoldTargetIsWired() {
        XCTAssertTrue(true)
    }

    @MainActor
    func testSendAllNowRequestsFlushThenRefreshesStatus() async {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)

        sut.sendAllNow()
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(client.sentMessages, [.flushUploadQueue, .requestMenuStatus])
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
}
