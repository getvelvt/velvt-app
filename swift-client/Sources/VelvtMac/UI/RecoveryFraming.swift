import Foundation

/// Copy source for the recovery stat (`plan/05-unified-roadmap.md` invariant 6).
///
/// Recoveries are the headline personal stat: an accumulating count of returns
/// to the declared block that can only grow. Framing rules, enforced by
/// `RecoveryFramingTests`:
///
/// - always positive ("returned to the block N times"), never a failure count;
/// - never a streak and never a broken-chain metaphor;
/// - analyst voice (roadmap invariant 7): evidence only, no "still", no
///   moralizing, no reference to the user's failure history.
///
/// Both surfaces that show recoveries (the work-block session result and the
/// Focus Fragmentation card) must draw their copy from here so the framing
/// cannot drift apart.
public enum RecoveryFraming {
  /// Headline shown wherever the recovery count is surfaced.
  public static func headline(count: Int) -> String {
    switch max(0, count) {
    case 0: return "Returns to the block are counted here; the count only grows."
    case 1: return "Returned to the block 1 time."
    case let count: return "Returned to the block \(count) times."
    }
  }

  /// Plain-language explanation attached as `.help` and read by VoiceOver.
  public static let explanation =
    "A return means you came back to the block's work category after an observed switch, "
    + "using the documented session rule. This count accumulates and can only grow."

  /// VoiceOver label: the headline plus its plain-language explanation.
  public static func accessibilityLabel(count: Int) -> String {
    "\(headline(count: count)) \(explanation)"
  }
}
