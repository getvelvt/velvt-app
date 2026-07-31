import SwiftUI
import XCTest

@testable import VelvtMac

/// The bars are only interpretable if a colour means the same category on every
/// row. Assigning from each row's segment index silently repainted categories
/// from day to day.
final class ActivityPaletteTests: XCTestCase {

  private func day(
    _ date: String,
    _ proportions: [(String, Int, Double)]
  ) -> DaySummaryViewModel {
    DaySummaryViewModel(
      DailySummary(
        date: date, status: .ready, eventCount: 10,
        focusScore: 50, fragmentationScore: 20,
        confidenceLevel: .medium, activeSeconds: 3600,
        typeProportions: proportions.map {
          ActivityProportion(category: $0.0, seconds: $0.1, proportion: $0.2)
        }))
  }

  func testCategoryKeepsOneColourEvenWhenItsRankChangesBetweenDays() {
    // DEEP_WORK leads on the first day and trails on the second.
    let days = [
      day("2026-06-15", [("DEEP_WORK", 3000, 0.8), ("COMMUNICATION", 600, 0.2)]),
      day("2026-06-16", [("COMMUNICATION", 3000, 0.8), ("DEEP_WORK", 600, 0.2)]),
    ]

    let palette = ActivityPalette.assign(for: days)

    XCTAssertNotNil(palette["DEEP_WORK"])
    XCTAssertNotNil(palette["COMMUNICATION"])
    XCTAssertNotEqual(palette["DEEP_WORK"], palette["COMMUNICATION"])
  }

  func testAssignmentIsStableAcrossRepeatedRenders() {
    let days = [
      day("2026-06-15", [("DEEP_WORK", 3000, 0.6), ("COMMUNICATION", 1200, 0.24)]),
      day("2026-06-16", [("REFERENCE", 900, 0.3), ("DEEP_WORK", 2100, 0.7)]),
    ]

    XCTAssertEqual(ActivityPalette.assign(for: days), ActivityPalette.assign(for: days))
  }

  func testEqualTotalsBreakTiesOnNameSoOrderingIsDeterministic() {
    let days = [day("2026-06-15", [("BETA", 1000, 0.5), ("ALPHA", 1000, 0.5)])]

    let ordered = ActivityPalette.ordered(for: days).map(\.category)

    XCTAssertEqual(ordered, ["ALPHA", "BETA"])
  }

  func testLegendSkipsCategoriesWithNoObservedTime() {
    let days = [day("2026-06-15", [("DEEP_WORK", 3600, 1.0), ("IDLE", 0, 0.0)])]

    let ordered = ActivityPalette.ordered(for: days).map(\.category)

    XCTAssertEqual(ordered, ["DEEP_WORK"])
  }
}
