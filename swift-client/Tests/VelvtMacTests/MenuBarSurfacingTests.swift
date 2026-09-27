import AppKit
import Combine
import XCTest

@testable import VelvtMac

/// The window stays open when you click another app, which means "open" and "in
/// front of the person" stopped being the same thing. Everything here is about
/// that gap: a click on the menu-bar icon, and a tap on a notification, both
/// have to end with the window in front — never with it vanishing from behind
/// the app the person was working in.
@MainActor
final class MenuBarSurfacingTests: XCTestCase {

    private func makePresentation() -> PermissionPresentationModel {
        PermissionPresentationModel(
            permissionManager: FakePermissionManager(),
            onboardingStateStore: InMemoryOnboardingStateStore()
        )
    }

    private func makeDefaults(_ name: String = #function) -> UserDefaults {
        let suite = "MenuBarSurfacingTests.\(name)"
        UserDefaults().removePersistentDomain(forName: suite)
        return UserDefaults(suiteName: suite) ?? .standard
    }

    private func makeController(
        surface: OccludableSurface,
        statusItems: SurfacingStatusItemManager? = nil,
        activate: @escaping @MainActor () -> Void = {}
    ) -> MenuBarController {
        MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: ConcreteDisplayDataCoordinator(),
            popover: surface,
            statusItemManager: statusItems ?? SurfacingStatusItemManager(),
            windowSizeStore: MenuBarWindowSizeStore(defaults: makeDefaults()),
            activateApp: activate
        )
    }

    // MARK: Clicking the icon

    /// The high one. The window is pinned open but sitting behind another app,
    /// so the person cannot see it; they click the icon to come back to Velvt.
    /// Asking only "is it visible" closed it instead, and a second click was
    /// needed to see anything at all.
    func testAnOccludedWindowComesForwardOnOneClickInsteadOfClosing() {
        var activations = 0
        let surface = OccludableSurface()
        let sut = makeController(surface: surface, activate: { activations += 1 })
        sut.install()

        sut.showPopover()
        XCTAssertTrue(sut.isPopoverShown)

        // The person clicks another app. The window stays open, behind it.
        surface.occlude()

        sut.togglePopover()

        XCTAssertTrue(sut.isPopoverShown, "the window the person asked for must not disappear")
        XCTAssertEqual(surface.bringToFrontCount, 1, "one click has to be enough to get back to Velvt")
        XCTAssertEqual(surface.closeCount, 0)
        XCTAssertEqual(activations, 2, "the app comes forward with its window")

        sut.remove()
    }

    /// And the other half of the same rule: when the window *is* the thing in
    /// front of the person, the icon still puts it away.
    func testAWindowInFrontStillClosesOnClick() {
        let surface = OccludableSurface()
        let sut = makeController(surface: surface)
        sut.install()

        sut.showPopover()
        sut.togglePopover()

        XCTAssertFalse(sut.isPopoverShown)
        XCTAssertEqual(surface.closeCount, 1)
        XCTAssertEqual(surface.bringToFrontCount, 0)

        sut.remove()
    }

    /// Occluded, brought forward, then clicked again: the second click now sees
    /// a window in front and closes it. The two clicks are not the same gesture
    /// and must not have the same effect.
    func testBringingForwardThenClickingAgainCloses() {
        let surface = OccludableSurface()
        let sut = makeController(surface: surface)
        sut.install()

        sut.showPopover()
        surface.occlude()
        sut.togglePopover()
        XCTAssertTrue(sut.isPopoverShown)

        sut.togglePopover()
        XCTAssertFalse(sut.isPopoverShown)

        sut.remove()
    }

    // MARK: Notification taps

    /// The other high one. A notification is the product's single
    /// in-the-moment intervention; tapping it did nothing whenever the window
    /// had been left open behind another app, because `showPopover()` returned
    /// early on "already shown".
    func testANotificationTapSurfacesAWindowLeftOpenBehindAnotherApp() {
        var activations = 0
        let surface = OccludableSurface()
        let coordinator = ConcreteDisplayDataCoordinator()
        coordinator.updateHistory(HistoryPayload(days: 1, summaries: []))
        let menuBar = MenuBarController(
            presentation: makePresentation(),
            displayCoordinator: coordinator,
            popover: surface,
            statusItemManager: SurfacingStatusItemManager(),
            windowSizeStore: MenuBarWindowSizeStore(defaults: makeDefaults()),
            activateApp: { activations += 1 }
        )
        menuBar.install()
        menuBar.showPopover()
        surface.occlude()

        var scrolledDate: String?
        let router = NotificationResponseRouter(
            openPopover: { [weak menuBar] in menuBar?.showPopover() },
            scrollToDate: ScrollToDateAction { date in scrolledDate = date }
        )

        router.handle(userInfo: ["insight_date": "2026-09-24"])

        XCTAssertTrue(menuBar.isPopoverShown)
        XCTAssertEqual(surface.bringToFrontCount, 1, "a tapped notification must always surface the window")
        XCTAssertEqual(activations, 2)
        XCTAssertEqual(scrolledDate, "2026-09-24")

        menuBar.remove()
    }

    /// Surfacing is only for a window the person cannot see. One already in
    /// front is left exactly as it is — no re-activation, no reordering, and
    /// nothing that would reset the content they are reading.
    func testAFrontmostWindowIsLeftAloneRatherThanReSurfaced() {
        var activations = 0
        let surface = OccludableSurface()
        let sut = makeController(surface: surface, activate: { activations += 1 })
        sut.install()

        sut.showPopover()
        sut.showPopover()

        XCTAssertEqual(activations, 1)
        XCTAssertEqual(surface.bringToFrontCount, 0)

        sut.remove()
    }

    /// A surface that cannot be occluded at all — an `NSPopover`, or any simple
    /// fake — keeps the old meaning through the protocol default, so the
    /// distinction costs nothing where it does not apply.
    func testASurfaceThatCannotBeOccludedReportsShownAsFrontmost() {
        let popover = NSPopover()
        XCTAssertFalse(popover.isShown)
        XCTAssertFalse(popover.isFrontmostSurface)
    }

    // MARK: The panel's own answer

    /// `isShown` is `panel.isVisible`, so an ordered-out or miniaturised panel
    /// is never frontmost and the icon opens it rather than raising it.
    func testAHiddenPanelIsNeverFrontmost() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }

        XCTAssertFalse(presenter.panel.isVisible)
        XCTAssertFalse(presenter.isFrontmostSurface)
    }

    /// Raising the window must not take the keyboard. A `.nonactivatingPanel`
    /// can become key while the app is inactive, so `makeKeyAndOrderFront(_:)`
    /// here would send the person's next keystrokes to Velvt instead of to
    /// whatever they were typing into.
    func testBringingThePanelForwardRaisesItWithoutTakingKeyboardFocus() {
        let presenter = MenuBarPanelPresenter()
        defer { presenter.close() }

        presenter.bringToFront()

        XCTAssertTrue(presenter.panel.isVisible, "the window has to actually come forward")
        XCTAssertFalse(presenter.panel.isKeyWindow, "raising must not grab key from the user's work")
    }

    // MARK: The icon carries the state

    /// The resolved `MenuBarState` used to reach the tooltip and nothing else,
    /// so a revoked device or a dead service looked identical to a healthy one.
    func testTheIconCarriesTheResolvedStateAndNotOnlyTheTooltip() throws {
        let statusItems = SurfacingStatusItemManager()
        let sut = makeController(surface: OccludableSurface(), statusItems: statusItems)
        sut.install()
        let button = try XCTUnwrap(statusItems.button)

        var renderings: Set<Data> = []
        for state in MenuBarState.allCases {
            sut.applyIcon(for: state)
            let image = try XCTUnwrap(button.image, "every state needs a menu bar icon")
            XCTAssertEqual(
                image.accessibilityDescription,
                MenuBarIconProvider.accessibilityDescription(for: state),
                "VoiceOver has to read the same state the glyph shows"
            )
            XCTAssertEqual(button.toolTip, MenuBarIconProvider.accessibilityDescription(for: state))
            XCTAssertTrue(image.isTemplate, "the menu bar tints the icon; the app must not")
            renderings.insert(try XCTUnwrap(image.tiffRepresentation))
        }

        XCTAssertEqual(
            renderings.count,
            MenuBarState.allCases.count,
            "each state has to look different, not just describe itself differently"
        )

        sut.remove()
    }

}

// MARK: - Test doubles

/// A surface that can be visible without being in front of the person, which is
/// exactly the state a window that stays open lands in as soon as another app is
/// clicked or the person moves to another Space.
@MainActor
private final class OccludableSurface: PopoverPresenting {
    var behavior: NSPopover.Behavior = .transient
    var animates = false
    var contentViewController: NSViewController?
    var contentSize = NSSize.zero
    private(set) var isShown = false
    private(set) var isFrontmostSurface = false
    private(set) var bringToFrontCount = 0
    private(set) var closeCount = 0

    func show(relativeTo _: NSRect, of _: NSView, preferredEdge _: NSRectEdge) {
        isShown = true
        isFrontmostSurface = true
    }

    func close() {
        isShown = false
        isFrontmostSurface = false
        closeCount += 1
    }

    func bringToFront() {
        bringToFrontCount += 1
        isFrontmostSurface = true
    }

    /// The person clicks another app, or switches Space: still open, no longer
    /// something they can see.
    func occlude() {
        isFrontmostSurface = false
    }
}

@MainActor
private final class SurfacingStatusItemManager: StatusItemManaging {
    let button: NSButton? = NSButton()

    func install(target: AnyObject, action: Selector) {
        button?.target = target
        button?.action = action
    }

    func remove() {}
}
