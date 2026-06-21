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
}
