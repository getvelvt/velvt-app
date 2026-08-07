import XCTest

@testable import VelvtMac

/// Enforces `plan/05-unified-roadmap.md` invariant 6 (celebrate recovery, not
/// streaks) and invariant 7 (analyst voice) for the shared recovery copy used
/// by the session result and the Focus Fragmentation surface.
final class RecoveryFramingTests: XCTestCase {
  func testZeroRecoveriesIsForwardLookingNotAFailureCount() {
    XCTAssertEqual(
      RecoveryFraming.headline(count: 0),
      "Returns to the block are counted here; the count only grows."
    )
  }

  func testOneRecoveryUsesSingularPositiveHeadline() {
    XCTAssertEqual(RecoveryFraming.headline(count: 1), "Returned to the block 1 time.")
  }

  func testManyRecoveriesUsePluralPositiveHeadline() {
    XCTAssertEqual(RecoveryFraming.headline(count: 2), "Returned to the block 2 times.")
    XCTAssertEqual(RecoveryFraming.headline(count: 14), "Returned to the block 14 times.")
  }

  func testNegativeCountIsClampedToTheZeroFraming() {
    XCTAssertEqual(RecoveryFraming.headline(count: -3), RecoveryFraming.headline(count: 0))
  }

  func testVoiceOverLabelContainsHeadlineAndPlainLanguageExplanation() {
    let label = RecoveryFraming.accessibilityLabel(count: 3)
    XCTAssertTrue(label.hasPrefix("Returned to the block 3 times."))
    XCTAssertTrue(label.contains(RecoveryFraming.explanation))
    // The explanation must define a recovery in plain language, tied to the
    // documented session rule rather than an invented judgment.
    XCTAssertTrue(RecoveryFraming.explanation.contains("came back to the block's work category"))
    XCTAssertTrue(RecoveryFraming.explanation.contains("documented session rule"))
  }

  func testFramingNeverUsesFailureStreakOrBrokenChainLanguage() {
    var corpus = [RecoveryFraming.explanation]
    for count in [-1, 0, 1, 2, 7, 40] {
      corpus.append(RecoveryFraming.headline(count: count))
      corpus.append(RecoveryFraming.accessibilityLabel(count: count))
    }
    let text = corpus.joined(separator: " ").lowercased()
    let forbidden = [
      "fail", "streak", "broke", "broken", "chain", "lost", "lose",
      "still ", "distract", "wasted", "guilt", "shame", "drift",
    ]
    for term in forbidden {
      XCTAssertFalse(text.contains(term), "Recovery copy must never contain \"\(term)\"")
    }
  }
}
