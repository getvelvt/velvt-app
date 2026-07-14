import XCTest
@testable import VelvtMac

@MainActor
final class NotificationResponseRouterTests: XCTestCase {

    func testHandleOpensPopoverAndScrollsToTheInsightDate() {
        var openedPopover = false
        var scrolledDate: String?
        let action = ScrollToDateAction { date in scrolledDate = date }
        let sut = NotificationResponseRouter(
            openPopover: { openedPopover = true },
            scrollToDate: action
        )

        sut.handle(userInfo: ["insight_date": "2026-06-10"])

        XCTAssertTrue(openedPopover)
        XCTAssertEqual(scrolledDate, "2026-06-10")
    }

    func testHandleIgnoresUserInfoMissingInsightDate() {
        var openedPopover = false
        let sut = NotificationResponseRouter(
            openPopover: { openedPopover = true },
            scrollToDate: ScrollToDateAction { _ in XCTFail("should not scroll") }
        )

        sut.handle(userInfo: [:])

        XCTAssertFalse(openedPopover)
    }

    func testHandleIgnoresNonStringInsightDate() {
        var openedPopover = false
        let sut = NotificationResponseRouter(
            openPopover: { openedPopover = true },
            scrollToDate: ScrollToDateAction { _ in XCTFail("should not scroll") }
        )

        sut.handle(userInfo: ["insight_date": 42])

        XCTAssertFalse(openedPopover)
    }
}
