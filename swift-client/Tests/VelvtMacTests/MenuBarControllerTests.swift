import Combine
import SwiftUI
import XCTest
@testable import VelvtMac

@MainActor
private final class TestPopover: PopoverPresenting {
    var behavior: NSPopover.Behavior = .transient
    var animates = false
    var contentViewController: NSViewController?
    var contentSize = NSSize.zero
    private(set) var isShown = false

    func show(relativeTo _: NSRect, of _: NSView, preferredEdge _: NSRectEdge) {
        isShown = true
    }

    func close() {
        isShown = false
    }
}

@MainActor
private final class TestStatusItemManager: StatusItemManaging {
    let button: NSButton? = NSButton()

    func install(target: AnyObject, action: Selector) {
        button?.target = target
        button?.action = action
    }

    func remove() {}
}

@MainActor
final class MenuBarControllerTests: XCTestCase {

    private func makePresentation() -> PermissionPresentationModel {
        PermissionPresentationModel(
            permissionManager: FakePermissionManager(),
            onboardingStateStore: InMemoryOnboardingStateStore()
        )
    }

    func testPopoverUsesPreferredCompactSizeWhenScreenAllows() {
        let visibleFrame = CGRect(x: 0, y: 0, width: 1_440, height: 900)

        XCTAssertEqual(MenuBarPopoverLayout.preferredContentSize, CGSize(width: 660, height: 450))
        XCTAssertEqual(
            MenuBarPopoverLayout.contentSize(for: visibleFrame),
            MenuBarPopoverLayout.preferredContentSize
        )
    }

    func testPopoverSizeStaysWithinVisibleScreen() {
        let visibleFrame = CGRect(x: 0, y: 0, width: 600, height: 300)
        let size = MenuBarPopoverLayout.contentSize(for: visibleFrame)

        XCTAssertLessThanOrEqual(size.width, visibleFrame.width)
        XCTAssertLessThanOrEqual(size.height, visibleFrame.height)
        XCTAssertEqual(size.width, visibleFrame.width - MenuBarPopoverLayout.screenInset)
        XCTAssertEqual(size.height, visibleFrame.height - MenuBarPopoverLayout.screenInset)
    }

    func testWalkthroughAddsHeightWithoutExceedingVisibleScreen() {
        let roomyFrame = CGRect(x: 0, y: 0, width: 1_440, height: 900)
        let compactFrame = CGRect(x: 0, y: 0, width: 600, height: 500)

        XCTAssertEqual(
            MenuBarPopoverLayout.contentSize(
                for: roomyFrame,
                includesWalkthrough: true
            ),
            MenuBarPopoverLayout.walkthroughContentSize
        )
        XCTAssertEqual(
            MenuBarPopoverLayout.contentSize(
                for: compactFrame,
                includesWalkthrough: true
            ).height,
            compactFrame.height - MenuBarPopoverLayout.screenInset
        )
    }

    func testHostingControllerCannotOverrideExplicitPopoverSize() throws {
        let popover = TestPopover()

        _ = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: ConcreteDisplayDataCoordinator(),
            popover: popover,
            statusItemManager: TestStatusItemManager(),
            activateApp: {}
        )

        let hostingController = try XCTUnwrap(
            popover.contentViewController as? NSHostingController<MenuBarPopoverView>
        )
        XCTAssertTrue(hostingController.sizingOptions.isEmpty)
        XCTAssertEqual(popover.contentSize, MenuBarPopoverLayout.preferredContentSize)
    }

    func testGuidedTourExpandsPopoverInsteadOfCompressingMainContent() {
        let popover = TestPopover()
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: ConcreteDisplayDataCoordinator(),
            popover: popover,
            statusItemManager: TestStatusItemManager(),
            activateApp: {}
        )
        sut.install()

        sut.beginGuidedTour()

        XCTAssertGreaterThan(
            popover.contentSize.height,
            MenuBarPopoverLayout.preferredContentSize.height
        )
    }

    // MARK: - Popover stays open across data pushes

    func testPopoverStaysOpenWhenANewInsightArrivesWhileShown() {
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            popover: TestPopover(),
            statusItemManager: TestStatusItemManager(),
            activateApp: {}
        )
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
            popover: TestPopover(),
            statusItemManager: TestStatusItemManager(),
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
            popover: TestPopover(),
            statusItemManager: TestStatusItemManager(),
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
            popover: TestPopover(),
            statusItemManager: TestStatusItemManager(),
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
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            popover: TestPopover(),
            statusItemManager: TestStatusItemManager(),
            activateApp: {}
        )
        sut.install()

        sut.togglePopover()
        XCTAssertTrue(sut.isPopoverShown)

        sut.togglePopover()
        XCTAssertFalse(sut.isPopoverShown)

        sut.remove()
    }

    func testClosePopoverClosesAnOpenPopover() {
        let coordinator = ConcreteDisplayDataCoordinator()
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            popover: TestPopover(),
            statusItemManager: TestStatusItemManager(),
            activateApp: {}
        )
        sut.install()

        sut.showPopover()
        XCTAssertTrue(sut.isPopoverShown)

        sut.closePopover()
        XCTAssertFalse(sut.isPopoverShown, "Escape (wired to closePopover()) must close an open popover")

        sut.remove()
    }

    func testCollectionActivityStatusModelPublishesRunningState() {
        let subject = PassthroughSubject<CollectionStatus, Never>()
        let sut = CollectionActivityStatusModel(collectionStatus: subject.eraseToAnyPublisher())

        XCTAssertEqual(sut.status, .idle)

        subject.send(.running)

        let update = expectation(description: "Collection activity status updates")
        DispatchQueue.main.async {
            XCTAssertEqual(sut.status, .running)
            update.fulfill()
        }
        wait(for: [update], timeout: 1)
    }

    func testCurrentActivityModelPublishesTheLatestCollectedEvent() {
        let sut = CurrentActivityModel()
        let event = RawEvent(
            appName: "Browser",
            windowTitle: "Velvt Dashboard",
            occurredAt: Date(timeIntervalSince1970: 1)
        )

        sut.receive(event)

        let update = expectation(description: "Current activity updates")
        DispatchQueue.main.async {
            XCTAssertEqual(sut.activity, CurrentActivity(appName: "Browser", windowTitle: "Velvt Dashboard"))
            update.fulfill()
        }
        wait(for: [update], timeout: 1)
    }

    func testCurrentActivityModelCountsCollectedEvents() {
        let sut = CurrentActivityModel()

        sut.receive(RawEvent(appName: "Browser", windowTitle: "Velvt", occurredAt: Date(timeIntervalSince1970: 1)))
        sut.receive(RawEvent(appName: "Editor", windowTitle: "Code", occurredAt: Date(timeIntervalSince1970: 2)))

        let update = expectation(description: "Collected count updates")
        DispatchQueue.main.async {
            XCTAssertEqual(sut.collectedEventCount, 2)
            update.fulfill()
        }
        wait(for: [update], timeout: 1)
    }
}
