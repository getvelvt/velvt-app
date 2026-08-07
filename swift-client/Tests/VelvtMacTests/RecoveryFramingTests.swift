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

  /// The app-side banned-token registry (roadmap invariants 2, 6, and 7;
  /// D8), extended in 0.1.6 Scope 4 with the core-side gate vocabulary so
  /// the app scan is no narrower than the cloud copy gate: absence framing,
  /// failure tallies, and streak language are banned in every registered
  /// Swift-authored string, not just the recovery copy.
  static let bannedTokens = [
    "fail", "streak", "broke", "broken", "chain", "lost", "lose",
    "still ", "distract", "wasted", "guilt", "shame", "drift",
    "missed", "skipped", "declined", "you didn't", "you haven't", "you never",
    "last invitation", "last offer", "dismissed", "ignored",
  ]

  func testFramingNeverUsesFailureStreakOrBrokenChainLanguage() {
    var corpus = [RecoveryFraming.explanation]
    for count in [-1, 0, 1, 2, 7, 40] {
      corpus.append(RecoveryFraming.headline(count: count))
      corpus.append(RecoveryFraming.accessibilityLabel(count: count))
    }
    let text = corpus.joined(separator: " ").lowercased()
    for term in Self.bannedTokens {
      XCTAssertFalse(text.contains(term), "Recovery copy must never contain \"\(term)\"")
    }
  }

  /// The Scope 4 registry: digest labels, demotion disclosure labels, and
  /// the explain affordance. Same banned vocabulary, plus D6/D8 structure
  /// checks — recoveries lead, and no label suggests a reply or thread.
  func testScope4RegisteredLabelsAreAnalystVoiceWithNoFailureFraming() {
    let sampleState = DemotionState(
      state: .demoted,
      wrongCount: 4,
      deliveredCount: 16,
      thresholdPercent: 15,
      minimumSample: 10,
      windowDays: 14,
      thresholdPolicyVersion: 1,
      repromotionPolicyVersion: 1
    )
    let corpus = [
      DigestFraming.digestTitle,
      DigestFraming.returnedLabel,
      DigestFraming.completedLabel,
      DigestFraming.declaredLabel,
      DigestFraming.invitationsLabel,
      DigestFraming.wrongLabel,
      DigestFraming.withheldLabel,
      DigestFraming.acknowledgeLabel,
      DigestFraming.demotionTitle,
      DigestFraming.resumeLabel,
      DigestFraming.demotionDetail(sampleState),
      DigestFraming.explainLabel,
    ]
    let text = corpus.joined(separator: " ").lowercased()
    for term in Self.bannedTokens {
      XCTAssertFalse(text.contains(term), "Scope 4 copy must never contain \"\(term)\"")
    }
    // The chat gate (D7): no registered label invites a conversation.
    for conversational in ["reply", "chat", "ask", "tell us", "talk"] {
      XCTAssertFalse(
        text.contains(conversational),
        "Scope 4 copy must never suggest a conversation (\"\(conversational)\")")
    }
    // The demotion detail is inspectable: exact counts and both versioned
    // rules are present.
    let detail = DigestFraming.demotionDetail(sampleState)
    XCTAssertTrue(detail.contains("4 of 16"))
    XCTAssertTrue(detail.contains("15%"))
    XCTAssertTrue(detail.contains("14 days"))
    XCTAssertTrue(detail.contains("rule v1"))
  }
}
