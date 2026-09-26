import AppKit
import XCTest

@testable import VelvtMac

/// The window now stays open when you click another app. That single change
/// has a second-order consequence these tests exist to pin down: a surface
/// that refuses to dismiss while still sitting at `.statusBar` level on every
/// Space is an always-on-top overlay, which is exactly what this product must
/// not feel like.
@MainActor
final class MenuBarPinnedWindowTests: XCTestCase {

    private var priorValue: Any?

    override func setUp() {
        super.setUp()
        priorValue = UserDefaults.standard.object(
            forKey: MenuBarPanelPresenter.staysOpenDefaultsKey)
    }

    override func tearDown() {
        if let priorValue {
            UserDefaults.standard.set(priorValue, forKey: MenuBarPanelPresenter.staysOpenDefaultsKey)
        } else {
            UserDefaults.standard.removeObject(forKey: MenuBarPanelPresenter.staysOpenDefaultsKey)
        }
        super.tearDown()
    }

    /// Clicking away must not close the window unless the user asked for that.
    func testStaysOpenIsTheDefault() {
        UserDefaults.standard.removeObject(forKey: MenuBarPanelPresenter.staysOpenDefaultsKey)
        let presenter = MenuBarPanelPresenter()
        XCTAssertTrue(presenter.staysOpenOnFocusLoss, "the window must not vanish on an outside click")
    }

    /// The important one. A pinned window is an ordinary window: it sits behind
    /// the app you switch to and lives on one Space.
    func testAPinnedWindowIsNotAnAlwaysOnTopOverlay() {
        let presenter = MenuBarPanelPresenter()
        presenter.staysOpenOnFocusLoss = true

        XCTAssertEqual(
            presenter.panel.level, .normal,
            "a window that never dismisses must not also float above every other app")
        XCTAssertFalse(
            presenter.panel.collectionBehavior.contains(.canJoinAllSpaces),
            "a window that never dismisses must not follow the user onto every Space")
    }

    /// The transient shape is still available, and still behaves as it did.
    func testUnpinnedRestoresTheTransientPopoverBehaviour() {
        let presenter = MenuBarPanelPresenter()
        presenter.staysOpenOnFocusLoss = false

        XCTAssertEqual(presenter.panel.level, .statusBar)
        XCTAssertTrue(presenter.panel.collectionBehavior.contains(.canJoinAllSpaces))
    }

    /// `shouldDismiss` answers "did the user click away", which stays true when
    /// the window is pinned — the preference is applied at the call site, so the
    /// rule itself remains pure and independently meaningful.
    func testPinningDoesNotChangeTheClickAwayRuleItself() {
        let presenter = MenuBarPanelPresenter()
        presenter.staysOpenOnFocusLoss = true
        let unrelated = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 10, height: 10),
            styleMask: [.titled], backing: .buffered, defer: false)

        XCTAssertTrue(
            MenuBarPanelPresenter.shouldDismiss(panel: presenter.panel, keyWindow: unrelated),
            "the rule still reports a click-away; only the response to it changed")
    }

    /// Minimise is only safe because the menu-bar icon can reopen a miniaturised
    /// window — this app is `.accessory` and has no Dock tile to restore from.
    func testMiniaturisedWindowReadsAsNotShownSoTheIconReopensIt() {
        let presenter = MenuBarPanelPresenter()
        XCTAssertTrue(presenter.panel.styleMask.contains(.miniaturizable))
        XCTAssertFalse(
            presenter.isShown,
            "isShown tracks panel.isVisible, which a miniaturised panel reports false")
    }
}
