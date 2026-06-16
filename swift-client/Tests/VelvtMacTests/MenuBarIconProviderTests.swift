import XCTest
@testable import VelvtMac

final class MenuBarIconProviderTests: XCTestCase {
    func testEachStateHasADistinctSymbolName() {
        let names = MenuBarState.allCases.map(MenuBarIconProvider.symbolName(for:))
        XCTAssertEqual(Set(names).count, names.count)
    }

    func testEachStateHasANonEmptyAccessibilityDescription() {
        for state in MenuBarState.allCases {
            XCTAssertFalse(MenuBarIconProvider.accessibilityDescription(for: state).isEmpty)
        }
    }
}
