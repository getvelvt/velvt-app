import XCTest

@testable import VelvtMac

@MainActor
final class FocusAllowancePromptTests: XCTestCase {
    private func makeDefaults() -> UserDefaults {
        let suite = UserDefaults(suiteName: "FocusAllowancePromptTests.\(UUID().uuidString)")!
        suite.removePersistentDomain(forName: suite.description)
        return suite
    }

    func test_the_ask_starts_unasked() {
        let model = FocusAllowancePromptModel(defaults: makeDefaults(), openFocusSettings: {})
        XCTAssertFalse(model.hasBeenAsked)
    }

    func test_allowing_opens_the_user_s_own_focus_settings() {
        var opened = 0
        let model = FocusAllowancePromptModel(
            defaults: makeDefaults(), openFocusSettings: { opened += 1 })

        model.allowAndOpenFocusSettings()

        XCTAssertEqual(opened, 1, "Velvt never edits Focus itself; it opens the user's settings")
        XCTAssertTrue(model.hasRequested)
    }

    /// Asked once, remembered forever — a permission ask that returns on every
    /// launch reads as nagging, which is the opposite of the restraint the
    /// copy promises.
    func test_the_ask_is_remembered_across_launches() {
        let defaults = makeDefaults()
        let first = FocusAllowancePromptModel(defaults: defaults, openFocusSettings: {})
        first.allowAndOpenFocusSettings()

        let relaunched = FocusAllowancePromptModel(defaults: defaults, openFocusSettings: {})
        XCTAssertTrue(relaunched.hasBeenAsked)
    }

    func test_declining_is_also_remembered() {
        let defaults = makeDefaults()
        FocusAllowancePromptModel(defaults: defaults, openFocusSettings: {}).skip()

        let relaunched = FocusAllowancePromptModel(defaults: defaults, openFocusSettings: {})
        XCTAssertTrue(relaunched.hasBeenAsked)
    }

    /// Marked asked before the panel opens, so a settings panel that fails to
    /// open cannot leave the prompt returning forever.
    func test_a_settings_panel_that_never_opens_still_counts_as_asked() {
        let defaults = makeDefaults()
        let model = FocusAllowancePromptModel(
            defaults: defaults,
            openFocusSettings: { /* simulates the panel failing to open */ }
        )

        model.allowAndOpenFocusSettings()

        XCTAssertTrue(model.hasBeenAsked)
    }
}
