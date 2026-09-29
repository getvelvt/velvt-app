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

    /// The needs-a-category reminder opens the list it is about, which is not
    /// where a plain opening lands, and has no insight date to scroll to.
    func testHandleOpensTheNeedsACategoryListForTheReminder() {
        var openedPopover = false
        var openedList = 0
        let sut = NotificationResponseRouter(
            openPopover: { openedPopover = true },
            scrollToDate: ScrollToDateAction { _ in XCTFail("should not scroll") },
            openNeedsACategory: { openedList += 1 }
        )

        sut.handle(userInfo: [categoryPromptNotificationUserInfoKey: true])

        XCTAssertEqual(openedList, 1)
        XCTAssertFalse(openedPopover)
    }

    func testTheOtherTwoKindsNeverOpenTheNeedsACategoryList() {
        let sut = NotificationResponseRouter(
            openPopover: {},
            scrollToDate: ScrollToDateAction { _ in },
            openNeedsACategory: { XCTFail("only the reminder opens the list") }
        )

        sut.handle(userInfo: [interventionNotificationUserInfoKey: true])
        sut.handle(userInfo: ["insight_date": "2026-06-10"])
        sut.handle(userInfo: [:])
    }

    func testEachKindIsToldApartByItsMarker() {
        XCTAssertEqual(
            NotificationResponseRouter.surface(of: [interventionNotificationUserInfoKey: true]), .driftOffer)
        XCTAssertEqual(
            NotificationResponseRouter.surface(of: [categoryPromptNotificationUserInfoKey: true]), .categoryPrompt)
        XCTAssertEqual(NotificationResponseRouter.surface(of: ["insight_date": "2026-06-10"]), .dailyInsight)
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

    // MARK: Presentation while Velvt is the active app

    /// Active is not the same as showing the offer: a focused settings or
    /// onboarding window hides it, so the banner is the only thing that says
    /// it exists.
    func testWhileActiveADriftOfferIsABannerWhenTheCardIsNotInFront() {
        let reporter = RecordingNotificationDeliveryReporter()
        let sut = NotificationResponseRouter(
            openPopover: {},
            scrollToDate: ScrollToDateAction { _ in },
            isDriftCardInFront: { false },
            reporter: reporter
        )

        XCTAssertEqual(sut.presentationWhileActive(for: .driftOffer), [.banner, .list, .sound])
        XCTAssertEqual(reporter.entries, [.init(outcome: .bannerWhileActive, surface: .driftOffer)])
    }

    /// The menu-bar window draws the offer's card above every tab. With that
    /// window in front, a banner and a sound would announce what the person is
    /// already looking at, so the offer is only listed.
    func testWhileActiveADriftOfferIsListedOnlyBehindTheCardInFront() {
        let reporter = RecordingNotificationDeliveryReporter()
        let sut = NotificationResponseRouter(
            openPopover: {},
            scrollToDate: ScrollToDateAction { _ in },
            isDriftCardInFront: { true },
            reporter: reporter
        )

        XCTAssertEqual(sut.presentationWhileActive(for: .driftOffer), [.list])
        XCTAssertEqual(
            reporter.entries, [.init(outcome: .listedBehindVisibleCard, surface: .driftOffer)])
    }

    /// The card rule is about the drift card. A daily insight is presented as
    /// it always was.
    func testWhileActiveAnInsightIsAlwaysABanner() {
        let reporter = RecordingNotificationDeliveryReporter()
        let sut = NotificationResponseRouter(
            openPopover: {},
            scrollToDate: ScrollToDateAction { _ in },
            isDriftCardInFront: { true },
            reporter: reporter
        )

        XCTAssertEqual(sut.presentationWhileActive(for: .dailyInsight), [.banner, .list, .sound])
        XCTAssertEqual(reporter.entries, [.init(outcome: .bannerWhileActive, surface: .dailyInsight)])
    }

    /// The reminder is presented and reported as itself, not as an insight.
    func testWhileActiveTheReminderIsABannerReportedAsItsOwnSurface() {
        let reporter = RecordingNotificationDeliveryReporter()
        let sut = NotificationResponseRouter(
            openPopover: {},
            scrollToDate: ScrollToDateAction { _ in },
            isDriftCardInFront: { true },
            reporter: reporter
        )

        XCTAssertEqual(sut.presentationWhileActive(for: .categoryPrompt), [.banner, .list, .sound])
        XCTAssertEqual(reporter.entries, [.init(outcome: .bannerWhileActive, surface: .categoryPrompt)])
    }
}
