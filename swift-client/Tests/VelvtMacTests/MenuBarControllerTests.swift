import Combine
import XCTest
@testable import VelvtMac

@MainActor
final class MenuBarControllerTests: XCTestCase {

    private func makePresentation() -> PermissionPresentationModel {
        PermissionPresentationModel(
            permissionManager: FakePermissionManager(),
            onboardingStateStore: InMemoryOnboardingStateStore()
        )
    }

    // MARK: - Popover stays open across data pushes

    func testPopoverStaysOpenWhenANewInsightArrivesWhileShown() {
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(presentation: makePresentation(), displayCoordinator: coordinator)
        sut.install()

        sut.showPopover()
        XCTAssertTrue(sut.isPopoverShown)

        coordinator.updateInsight(
            InsightPayload(date: "2026-06-15", text: "first", confidenceLevel: .high, lowConfidence: false, generatedAt: Date())
        )
        XCTAssertTrue(sut.isPopoverShown, "Pushing new display data must not close the popover")

        coordinator.updateInsight(
            InsightPayload(date: "2026-06-16", text: "second", confidenceLevel: .high, lowConfidence: false, generatedAt: Date())
        )
        XCTAssertTrue(sut.isPopoverShown, "A second push while still open must not toggle the popover")

        sut.remove()
    }

    func testInsightUpdatesInPlaceRatherThanResettingToLoading() {
        // The same InsightViewModel instance is reused across pushes once
        // populated, so SwiftUI updates the existing card in place instead
        // of the popover content tearing down and rebuilding (which would
        // visually look like a close/reopen).
        let coordinator = ConcreteDisplayDataCoordinator()
        coordinator.updateInsight(
            InsightPayload(date: "2026-06-15", text: "first", confidenceLevel: .high, lowConfidence: false, generatedAt: Date())
        )
        guard case .populated(let insightVM, _) = coordinator.state else {
            XCTFail("Expected populated state")
            return
        }

        coordinator.updateInsight(
            InsightPayload(date: "2026-06-16", text: "second", confidenceLevel: .high, lowConfidence: false, generatedAt: Date())
        )

        guard case .populated(let insightVM2, _) = coordinator.state else {
            XCTFail("Expected populated state")
            return
        }
        XCTAssertTrue(insightVM === insightVM2, "Coordinator must reuse the same view model instance across pushes")
        XCTAssertEqual(insightVM.text, "second")
    }

    // MARK: - App hidden when notification tap fires

    func testShowPopoverActivatesTheAppBeforeShowing() {
        var activateCallCount = 0
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            activateApp: { activateCallCount += 1 }
        )
        sut.install()

        XCTAssertFalse(sut.isPopoverShown)
        sut.showPopover()

        XCTAssertEqual(activateCallCount, 1, "showPopover() must activate/unhide the app so a hidden app becomes visible")
        XCTAssertTrue(sut.isPopoverShown)

        sut.remove()
    }

    func testShowPopoverDoesNotReactivateWhenAlreadyShown() {
        var activateCallCount = 0
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            activateApp: { activateCallCount += 1 }
        )
        sut.install()

        sut.showPopover()
        sut.showPopover()

        XCTAssertEqual(activateCallCount, 1)
        sut.remove()
    }

    func testNotificationTapOpensPopoverViaActivation() {
        // Mirrors the real AppDelegate wiring: NotificationResponseRouter's
        // openPopover closure calls MenuBarController.showPopover().
        var activateCallCount = 0
        let coordinator = ConcreteDisplayDataCoordinator()
        coordinator.updateHistory(HistoryPayload(days: 1, summaries: []))
        let menuBar = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            activateApp: { activateCallCount += 1 }
        )
        menuBar.install()

        var scrolledDate: String?
        let router = NotificationResponseRouter(
            openPopover: { [weak menuBar] in menuBar?.showPopover() },
            scrollToDate: ScrollToDateAction { date in scrolledDate = date }
        )

        router.handle(userInfo: ["insight_date": "2026-06-12"])

        XCTAssertEqual(activateCallCount, 1, "Tapping a notification while the app is hidden must bring it to the foreground")
        XCTAssertTrue(menuBar.isPopoverShown)
        XCTAssertEqual(scrolledDate, "2026-06-12")

        menuBar.remove()
    }

    // MARK: - Toggle / Escape

    func testToggleOpensThenClosesThePopover() {
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(presentation: makePresentation(), displayCoordinator: coordinator)
        sut.install()

        sut.togglePopover()
        XCTAssertTrue(sut.isPopoverShown)

        sut.togglePopover()
        XCTAssertFalse(sut.isPopoverShown)

        sut.remove()
    }

    func testClosePopoverClosesAnOpenPopover() {
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(presentation: makePresentation(), displayCoordinator: coordinator)
        sut.install()

        sut.showPopover()
        XCTAssertTrue(sut.isPopoverShown)

        sut.closePopover()
        XCTAssertFalse(sut.isPopoverShown, "Escape (wired to closePopover()) must close an open popover")

        sut.remove()
    }
}
