import Combine
import ObjectiveC
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

        XCTAssertEqual(MenuBarPopoverLayout.preferredContentSize, CGSize(width: 600, height: 480))
        XCTAssertEqual(MenuBarPopoverLayout.walkthroughContentSize, CGSize(width: 600, height: 567))
        XCTAssertEqual(
            MenuBarPopoverLayout.contentSize(for: visibleFrame),
            MenuBarPopoverLayout.preferredContentSize
        )
    }

    /// 600pt is 47% of the narrowest Mac laptop screen. It used to be 660,
    /// which is 52% — more than half the display for a menu bar popover.
    func testPopoverIsUnderHalfOfTheNarrowestLaptopScreen() {
        XCTAssertLessThan(MenuBarPopoverLayout.preferredContentSize.width, 1_280 / 2)
    }

    func testPopoverSizeStaysWithinVisibleScreen() {
        let visibleFrame = CGRect(x: 0, y: 0, width: 600, height: 300)
        let size = MenuBarPopoverLayout.contentSize(for: visibleFrame)

        XCTAssertLessThanOrEqual(size.width, visibleFrame.width)
        XCTAssertLessThanOrEqual(size.height, visibleFrame.height)
        XCTAssertEqual(size.width, visibleFrame.width - MenuBarPopoverLayout.screenInset)
        // The height floor wins here: 300 - 24 is 276pt, below the 320pt
        // minimum, so the popover takes the whole visible height rather than
        // shrinking under the size at which it stops being readable.
        XCTAssertEqual(size.height, visibleFrame.height)
    }

    /// The old clamp was `max(1, visibleFrame - inset)`, which hands
    /// `NSPopover` a 1pt dimension on a small enough frame.
    func testTinyVisibleFrameNeverProducesAOnePointPopover() {
        for edge in [CGFloat(1), 4, 10, 25, 26, 60, 200, 340] {
            let size = MenuBarPopoverLayout.contentSize(
                for: CGRect(x: 0, y: 0, width: edge, height: edge)
            )
            XCTAssertEqual(size.width, min(MenuBarPopoverLayout.minimumContentSize.width, edge))
            XCTAssertEqual(size.height, min(MenuBarPopoverLayout.minimumContentSize.height, edge))
        }
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

    // MARK: - The surface is a window, because a popover cannot be resized

    /// The request was "let me resize this with the mouse", and the first
    /// question is whether the surface it was asked of can do that at all.
    ///
    /// It cannot, and this pins why: `NSPopover` declares no `styleMask`, no
    /// `isResizable`, no `minSize`/`maxSize` and no `frameAutosaveName`. There
    /// is no property to set and no edge to grab; `contentSize` is settable
    /// only in code. If a future macOS adds one of these, this test fails and
    /// the conversion below can be reconsidered.
    func testNSPopoverHasNoUserResizeAffordanceAtAll() {
        for name in ["styleMask", "isResizable", "resizable", "minSize", "maxSize", "frameAutosaveName"] {
            XCTAssertNil(
                class_getProperty(NSPopover.self, name),
                "NSPopover unexpectedly declares \(name); the panel conversion may no longer be needed"
            )
        }
    }

    func testTheShippingSurfaceIsAResizablePanel() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }

        XCTAssertTrue(presenter.panel.isResizable)
        XCTAssertTrue(presenter.panel.styleMask.contains(.resizable))
        XCTAssertTrue(presenter.panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertTrue(presenter.panel.styleMask.contains(.titled))
    }

    /// `.fullSizeContentView` lays the content out *under* the title bar —
    /// measured, `contentView.safeAreaInsets.top` becomes 28 — which is
    /// exactly the reported "wordmark cut off at the top edge". Without it the
    /// insets are zero on every edge, so no chrome can eat the header.
    func testNothingIsDrawnUnderTheTitleBar() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        presenter.contentViewController = NSViewController()
        presenter.contentViewController?.view = NSView(
            frame: NSRect(origin: .zero, size: MenuBarPopoverLayout.preferredContentSize)
        )
        presenter.contentSize = MenuBarPopoverLayout.preferredContentSize

        XCTAssertFalse(presenter.panel.styleMask.contains(.fullSizeContentView))
        let insets = presenter.panel.contentView?.safeAreaInsets ?? NSEdgeInsetsZero
        XCTAssertEqual(insets.top, 0)
        XCTAssertEqual(insets.left, 0)
        XCTAssertEqual(insets.right, 0)
        XCTAssertEqual(insets.bottom, 0)
        XCTAssertEqual(
            presenter.panel.contentLayoutRect.size,
            presenter.panel.contentView?.bounds.size
        )
    }

    /// It must still feel like a menu bar app: focusable enough for the
    /// Settings text fields and the Escape handler, never a document window,
    /// present on every Space, and out of the window cycler. Staying out of
    /// Cmd-Tab and the Dock is the application's `.accessory` activation
    /// policy, which the executable entry point owns and this type leaves
    /// alone.
    func testThePanelStillBehavesLikeAMenuBarSurface() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        let panel = presenter.panel

        XCTAssertTrue(panel.canBecomeKey, "Settings text fields and Escape need key")
        XCTAssertFalse(panel.canBecomeMain, "A menu bar surface is never the main window")
        XCTAssertEqual(panel.level, .statusBar)
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertTrue(panel.collectionBehavior.contains(.fullScreenAuxiliary))
        XCTAssertTrue(panel.collectionBehavior.contains(.ignoresCycle))
        XCTAssertEqual(panel.titleVisibility, .hidden)
        XCTAssertTrue(panel.titlebarAppearsTransparent)
        XCTAssertEqual(panel.standardWindowButton(.closeButton)?.isHidden, true)
        XCTAssertEqual(panel.standardWindowButton(.miniaturizeButton)?.isHidden, true)
        XCTAssertEqual(panel.standardWindowButton(.zoomButton)?.isHidden, true)
        XCTAssertFalse(panel.isRestorable)
        XCTAssertEqual(panel.animationBehavior, .none)
    }

    /// Click-away dismissal, minus the activation cycle. Key moving to a
    /// window the panel owns is not the user leaving.
    ///
    /// The Settings submenus used to be the main thing that took key inside
    /// this panel, and they are gone — settings detail is rendered in the
    /// panel now. The guard still has work: the focus-session popover is an
    /// `NSPopover` anchored in this panel, the sign-in flow is a sheet on it,
    /// and both "Clear Local Work Blocks" and "Delete Account" put a
    /// confirmation sheet on it. Losing any of those to a dismissal would
    /// close the surface the moment the user opened it.
    ///
    /// The last case is the one that keeps this honest after the submenus
    /// left: a window that is somebody *else's* child is still a stranger, so
    /// the guard may not be loosened to "any window with a parent".
    func testClickAwayDismissesButOpeningSomethingInsideDoesNot() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        let panel = presenter.panel
        let child = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 100, height: 100),
            styleMask: [.titled], backing: .buffered, defer: false
        )
        let stranger = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 100, height: 100),
            styleMask: [.titled], backing: .buffered, defer: false
        )
        let otherWindow = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 100, height: 100),
            styleMask: [.titled], backing: .buffered, defer: false
        )
        let othersChild = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 100, height: 100),
            styleMask: [.titled], backing: .buffered, defer: false
        )
        panel.addChildWindow(child, ordered: .above)
        otherWindow.addChildWindow(othersChild, ordered: .above)
        defer {
            panel.removeChildWindow(child)
            otherWindow.removeChildWindow(othersChild)
        }

        XCTAssertFalse(MenuBarPanelPresenter.shouldDismiss(panel: panel, keyWindow: panel))
        XCTAssertFalse(MenuBarPanelPresenter.shouldDismiss(panel: panel, keyWindow: child))
        XCTAssertTrue(MenuBarPanelPresenter.shouldDismiss(panel: panel, keyWindow: stranger))
        XCTAssertTrue(MenuBarPanelPresenter.shouldDismiss(panel: panel, keyWindow: othersChild))
        XCTAssertTrue(MenuBarPanelPresenter.shouldDismiss(panel: panel, keyWindow: nil))
    }

    /// Keyboard navigation, asserted where it actually lives.
    ///
    /// The destination list is a SwiftUI `List`, and a `List` on macOS is an
    /// `NSTableView` underneath — which is the entire reason it is a `List`
    /// and not the column of `Button`s it replaced. Arrow keys between rows,
    /// Tab into the list, and a selection that survives focus leaving are the
    /// table's behaviour, not something reimplemented above it. A column of
    /// buttons would photograph identically and answer no key press, so the
    /// check is that the table is really there with a row per destination.
    func testTheSettingsDestinationListIsARealKeyboardNavigableTable() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        let hosting = NSHostingController(
            rootView: makeSettingsPopoverView().openedOnSettings(nil)
        )
        hosting.sizingOptions = []
        presenter.contentViewController = hosting
        presenter.contentSize = CGSize(width: 600, height: 520)
        presenter.panel.orderFront(nil)
        presenter.panel.layoutIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))

        guard let table = Self.firstTableView(in: presenter.panel.contentView) else {
            return XCTFail("The settings destination list did not materialize as a table")
        }
        XCTAssertGreaterThanOrEqual(
            table.numberOfRows,
            SettingsSubmenu.allCases.count - 1,
            "Every settings destination outside DEBUG needs a row a key press can reach"
        )
        XCTAssertTrue(
            presenter.panel.canBecomeKey,
            "A list nobody can focus is not keyboard navigation"
        )
    }

    private static func firstTableView(in view: NSView?) -> NSTableView? {
        guard let view else { return nil }
        if let table = view as? NSTableView { return table }
        for subview in view.subviews {
            if let table = firstTableView(in: subview) { return table }
        }
        return nil
    }

    private func makeSettingsPopoverView() -> MenuBarPopoverView {
        let client = FakeIPCClient()
        return MenuBarPopoverView(
            presentation: PermissionPresentationModel(
                permissionManager: FakePermissionManager(),
                onboardingStateStore: InMemoryOnboardingStateStore()
            ),
            coordinator: ConcreteDisplayDataCoordinator(),
            serviceConnectionStatus: ServiceConnectionStatusModel(
                connectionStatus: Just(.connected).eraseToAnyPublisher()
            ),
            collectionActivityStatus: CollectionActivityStatusModel(
                collectionStatus: Just(.idle).eraseToAnyPublisher()
            ),
            currentActivity: CurrentActivityModel(),
            serviceAlertModel: ServiceAlertModel(messages: Empty<ServerMessage, Never>()),
            ipcClient: client,
            updateController: .disabled(),
            onEscape: {}
        )
    }

    /// The panel is still the thing that has to survive a settings click.
    ///
    /// Selecting a destination no longer creates any window at all, so no
    /// resign-key event is posted and there is nothing for the guard to catch.
    ///
    /// The assertion has to be made with a destination actually open, through
    /// the real panel: an empty panel with no content installed has no child
    /// windows either, so checking one proves nothing about settings. Every
    /// destination is mounted in turn and the panel is required to still own
    /// no window afterwards — that is the user's complaint, stated as a test.
    func testSelectingASettingsDestinationCreatesNoWindowForTheGuardToCatch() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        let panel = presenter.panel
        presenter.contentSize = CGSize(width: 600, height: 520)
        panel.orderFront(nil)

        for destination in SettingsSubmenu.allCases {
            let hosting = NSHostingController(
                rootView: makeSettingsPopoverView().openedOnSettings(destination)
            )
            hosting.sizingOptions = []
            presenter.contentViewController = hosting
            panel.layoutIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))

            XCTAssertEqual(
                panel.childWindows?.count ?? 0, 0,
                "Opening \(destination.title) put a detached window on the panel"
            )
            XCTAssertTrue(
                panel.sheets.isEmpty,
                "Opening \(destination.title) put a sheet on the panel"
            )
            XCTAssertFalse(MenuBarPanelPresenter.shouldDismiss(panel: panel, keyWindow: panel))
        }
    }

    /// `NSWindow.contentMinSize` is enforced for a user drag but not for
    /// `setContentSize(_:)`: measured, a panel with a 420x320 minimum accepts a
    /// programmatic 100x100. Every programmatic path therefore clamps here.
    func testProgrammaticSizeIsClampedBecauseContentMinSizeIsNot() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        presenter.minimumContentSize = CGSize(width: 500, height: 320)
        presenter.maximumContentSize = CGSize(width: 900, height: 700)

        presenter.contentSize = CGSize(width: 100, height: 100)
        XCTAssertEqual(presenter.contentSize, CGSize(width: 500, height: 320))

        presenter.contentSize = CGSize(width: 4_000, height: 4_000)
        XCTAssertEqual(presenter.contentSize, CGSize(width: 900, height: 700))

        presenter.contentSize = CGSize(width: 640, height: 500)
        XCTAssertEqual(presenter.contentSize, CGSize(width: 640, height: 500))
    }

    /// A 1pt window is not graceful degradation, and neither is a maximum that
    /// sits under the minimum: on a screen too small for the floor the window
    /// takes the whole screen rather than inverting the bounds.
    func testTheResizeBoundsNeverInvert() {
        for edge in [CGFloat(1), 40, 200, 320, 480, 600, 1_440, 3_008] {
            let frame = CGRect(x: 0, y: 0, width: edge, height: edge)
            let maximum = MenuBarPopoverLayout.maximumContentSize(for: frame)
            XCTAssertGreaterThanOrEqual(maximum.width, MenuBarPopoverLayout.minimumContentSize.width)
            XCTAssertGreaterThanOrEqual(
                maximum.height, MenuBarPopoverLayout.minimumContentSize.height)
        }
    }

    /// Clicking the status item while the window is open is one gesture that
    /// arrives as two events — focus leaves the window, and the toggle fires —
    /// in an order AppKit picks. Both orders have to end with the window
    /// closed. Without the grace window the dismissal-first order reopens the
    /// window and the click looks like it did nothing.
    func testClickingTheIconWhileOpenAlwaysEndsClosedInEitherEventOrder() {
        var clock = Date(timeIntervalSince1970: 1_000)
        let popover = TestPopover()
        let sut = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: ConcreteDisplayDataCoordinator(),
            popover: popover,
            statusItemManager: TestStatusItemManager(),
            windowSizeStore: MenuBarWindowSizeStore(defaults: makeDefaults()),
            focusLossToggleGrace: 0.3,
            now: { clock },
            activateApp: {}
        )
        sut.install()

        // Order A — the toggle wins: it sees an open window and closes it.
        sut.showPopover()
        XCTAssertTrue(sut.isPopoverShown)
        sut.togglePopover()
        XCTAssertFalse(sut.isPopoverShown)

        // Order B — the dismissal wins: the window is already closed by the
        // time the toggle lands, and the toggle must not reopen it.
        sut.showPopover()
        XCTAssertTrue(sut.isPopoverShown)
        sut.simulateFocusLossDismissalForTesting()
        XCTAssertFalse(sut.isPopoverShown)
        sut.togglePopover()
        XCTAssertFalse(sut.isPopoverShown, "The click that closed it must not reopen it")

        // A deliberate click a moment later still opens.
        clock = clock.addingTimeInterval(1)
        sut.togglePopover()
        XCTAssertTrue(sut.isPopoverShown)

        sut.remove()
    }

    // MARK: - The chosen size survives a relaunch

    private func makeDefaults(_ name: String = #function) -> UserDefaults {
        let suite = "MenuBarControllerTests.\(name)"
        UserDefaults().removePersistentDomain(forName: suite)
        return UserDefaults(suiteName: suite) ?? .standard
    }

    func testAResizeTheUserMadeIsStillThereNextLaunch() {
        let defaults = makeDefaults()
        let store = MenuBarWindowSizeStore(defaults: defaults)
        XCTAssertNil(store.contentSize, "First launch has nothing stored")

        store.store(CGSize(width: 760, height: 620))

        let nextLaunch = MenuBarWindowSizeStore(defaults: defaults)
        XCTAssertEqual(nextLaunch.contentSize, CGSize(width: 760, height: 620))
        XCTAssertEqual(
            MenuBarPopoverLayout.resolvedContentSize(
                stored: nextLaunch.contentSize,
                visibleFrame: CGRect(x: 0, y: 0, width: 1_920, height: 1_080)
            ),
            CGSize(width: 760, height: 620)
        )
    }

    /// The whole round trip, through the real panel: a drag changes the
    /// window, the change is persisted, and the next launch opens at it.
    /// `panel.setContentSize` stands in for the drag — it is the same code
    /// path AppKit runs at the end of one, and it fires the same
    /// `windowDidResize`.
    func testDraggingTheWindowPersistsTheSizeForTheNextLaunch() {
        let defaults = makeDefaults()
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        presenter.maximumContentSize = CGSize(width: 1_600, height: 1_000)
        let store = MenuBarWindowSizeStore(defaults: defaults)
        presenter.onUserResize = { store.store($0) }

        presenter.contentSize = MenuBarPopoverLayout.preferredContentSize
        XCTAssertNil(store.contentSize, "Opening at the default size is not a user preference")

        presenter.panel.setContentSize(NSSize(width: 820, height: 610))

        XCTAssertEqual(store.contentSize, CGSize(width: 820, height: 610))
        XCTAssertEqual(
            MenuBarPopoverLayout.resolvedContentSize(
                stored: MenuBarWindowSizeStore(defaults: defaults).contentSize,
                visibleFrame: CGRect(x: 0, y: 0, width: 1_920, height: 1_080)
            ),
            CGSize(width: 820, height: 610)
        )
    }

    /// Re-anchoring under the status item moves the window without that
    /// counting as a resize the user asked for.
    func testRepositioningTheWindowIsNotMistakenForAResize() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }
        var reported: [CGSize] = []
        presenter.onUserResize = { reported.append($0) }
        presenter.maximumContentSize = CGSize(width: 1_600, height: 1_000)
        presenter.contentSize = MenuBarPopoverLayout.preferredContentSize
        reported.removeAll()

        let host = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
            styleMask: [.titled], backing: .buffered, defer: false
        )
        let button = NSView(frame: NSRect(x: 100, y: 260, width: 24, height: 22))
        host.contentView?.addSubview(button)
        presenter.position(under: button)
        presenter.position(under: button)

        XCTAssertEqual(reported, [], "Moving the window is not the user resizing it")
    }

    func testACorruptStoredSizeFallsBackInsteadOfOpeningInvisible() {
        let defaults = makeDefaults()
        defaults.set(0.0, forKey: "velvt.menubar.window_content_width")
        defaults.set(0.0, forKey: "velvt.menubar.window_content_height")
        let store = MenuBarWindowSizeStore(defaults: defaults)

        XCTAssertNil(store.contentSize)
        XCTAssertEqual(
            MenuBarPopoverLayout.resolvedContentSize(stored: store.contentSize, visibleFrame: nil),
            MenuBarPopoverLayout.preferredContentSize
        )
    }

    /// A window dragged wide on a 6K display and then opened on a laptop must
    /// come back inside the laptop, not open wider than the screen.
    func testASizeFromABiggerDisplayIsClampedToTheOneItOpensOn() {
        let laptop = CGRect(x: 0, y: 0, width: 1_280, height: 800)
        let resolved = MenuBarPopoverLayout.resolvedContentSize(
            stored: CGSize(width: 2_400, height: 1_600),
            visibleFrame: laptop
        )

        XCTAssertLessThanOrEqual(resolved.width, laptop.width)
        XCTAssertLessThanOrEqual(resolved.height + MenuBarPopoverLayout.titleBarHeight, laptop.height)
        XCTAssertEqual(resolved, MenuBarPopoverLayout.maximumContentSize(for: laptop))
    }

    /// The tour bar belongs to the window, not to the user. Storing an
    /// inflated height would make the tour's 87pt permanent and the window
    /// would grow by a bar on every launch.
    func testTheGuidedTourBarIsNotFoldedIntoTheStoredSize() {
        let dragged = CGSize(width: 700, height: 600)
        let withTour = CGSize(
            width: 700,
            height: 600 + MenuBarPopoverLayout.guidedTourBarHeight
        )

        XCTAssertEqual(
            MenuBarPopoverLayout.storableContentSize(withTour, includesWalkthrough: true),
            dragged
        )
        XCTAssertEqual(
            MenuBarPopoverLayout.storableContentSize(dragged, includesWalkthrough: false),
            dragged
        )
        XCTAssertEqual(
            MenuBarPopoverLayout.resolvedContentSize(
                stored: dragged,
                visibleFrame: CGRect(x: 0, y: 0, width: 1_920, height: 1_080),
                includesWalkthrough: true
            ),
            withTour
        )
    }

    // MARK: - Placement

    /// A status item in the middle of a 1440pt menu bar — far enough from the
    /// right edge that centring the window under it does not also have to be
    /// clamped, so the two behaviours can be asserted separately.
    private var statusItemFrame: CGRect {
        CGRect(x: 700, y: 875, width: 24, height: 22)
    }

    func testTheWindowHangsUnderTheStatusItemAndInsideTheScreen() {
        let visibleFrame = CGRect(x: 0, y: 0, width: 1_440, height: 875)
        let content = MenuBarPopoverLayout.preferredContentSize
        let frame = MenuBarPopoverLayout.windowFrame(
            forContentSize: content,
            statusItemFrame: statusItemFrame,
            visibleFrame: visibleFrame
        )

        XCTAssertEqual(frame.width, content.width)
        XCTAssertEqual(frame.height, content.height + MenuBarPopoverLayout.titleBarHeight)
        XCTAssertEqual(frame.midX, statusItemFrame.midX, accuracy: 0.5)
        XCTAssertEqual(
            frame.maxY,
            statusItemFrame.minY - MenuBarPopoverLayout.statusItemGap,
            accuracy: 0.5
        )
        XCTAssertTrue(visibleFrame.contains(frame), "\(frame) escaped \(visibleFrame)")
    }

    /// A status item in the far right corner would centre the window off the
    /// edge of the screen.
    func testAStatusItemInTheCornerDoesNotPushTheWindowOffScreen() {
        let visibleFrame = CGRect(x: 0, y: 0, width: 1_440, height: 875)
        for x in [CGFloat(0), 4, 700, 1_400, 1_416] {
            let item = CGRect(x: x, y: 875, width: 24, height: 22)
            let frame = MenuBarPopoverLayout.windowFrame(
                forContentSize: MenuBarPopoverLayout.preferredContentSize,
                statusItemFrame: item,
                visibleFrame: visibleFrame
            )
            XCTAssertTrue(visibleFrame.contains(frame), "item at \(x) produced \(frame)")
        }
    }

    /// The second display in this arrangement is to the *left* of the primary,
    /// so its visible frame has a negative origin. A window placed there and
    /// then reopened after the display is unplugged is repositioned from the
    /// status item, which is now on the remaining screen — there is no stored
    /// origin left over to strand it.
    func testAWindowOnASecondDisplayComesBackWhenThatDisplayIsUnplugged() {
        let secondary = CGRect(x: -1_920, y: 0, width: 1_920, height: 1_055)
        let secondaryItem = CGRect(x: -800, y: 1_055, width: 24, height: 22)
        let onSecondary = MenuBarPopoverLayout.windowFrame(
            forContentSize: MenuBarPopoverLayout.preferredContentSize,
            statusItemFrame: secondaryItem,
            visibleFrame: secondary
        )
        XCTAssertTrue(secondary.contains(onSecondary))

        let builtIn = CGRect(x: 0, y: 0, width: 1_440, height: 875)
        let afterUnplug = MenuBarPopoverLayout.windowFrame(
            forContentSize: MenuBarPopoverLayout.preferredContentSize,
            statusItemFrame: statusItemFrame,
            visibleFrame: builtIn
        )
        XCTAssertTrue(builtIn.contains(afterUnplug), "\(afterUnplug) escaped \(builtIn)")
    }

    /// A window bigger than the screen keeps its top-left corner on screen, so
    /// the header and the navigation rail are the parts that stay reachable.
    func testAWindowLargerThanTheScreenKeepsItsTopLeftCornerVisible() {
        let tiny = CGRect(x: 0, y: 0, width: 400, height: 300)
        let frame = MenuBarPopoverLayout.windowFrame(
            forContentSize: CGSize(width: 900, height: 700),
            statusItemFrame: CGRect(x: 300, y: 300, width: 24, height: 22),
            visibleFrame: tiny
        )

        XCTAssertEqual(frame.minX, tiny.minX)
        XCTAssertEqual(frame.minY, tiny.minY)
    }

    /// The origin is derived on every open, never restored, so there is no
    /// path by which a saved frame lands on a display that is gone.
    func testNoOriginIsEverPersisted() {
        let defaults = makeDefaults()
        let store = MenuBarWindowSizeStore(defaults: defaults)
        store.store(CGSize(width: 700, height: 560))

        let persisted = defaults.dictionaryRepresentation().keys
            .filter { $0.hasPrefix("velvt.menubar.window") }
            .sorted()
        XCTAssertEqual(
            persisted,
            ["velvt.menubar.window_content_height", "velvt.menubar.window_content_width"]
        )
    }

    // MARK: - The measured floor

    /// 470pt is where the bottom bar stops truncating on the rendered width
    /// ladder ("Start a focus sess…" at 460, "Start a focus sessi…" at 465,
    /// the whole label at 470). The floor keeps slack above that; it is not
    /// allowed to drift back under the measurement.
    func testTheMinimumWidthIsAboveTheMeasuredTruncationPoint() {
        XCTAssertGreaterThanOrEqual(MenuBarPopoverLayout.minimumContentSize.width, 470)
        XCTAssertLessThan(
            MenuBarPopoverLayout.minimumContentSize.width,
            MenuBarPopoverLayout.preferredContentSize.width,
            "A minimum equal to the preferred size is not a resizable window"
        )
        XCTAssertLessThan(
            MenuBarPopoverLayout.minimumContentSize.height,
            MenuBarPopoverLayout.preferredContentSize.height
        )
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
