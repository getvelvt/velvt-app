import Foundation

/// Registered Swift-authored labels for the 0.1.6 Scope 4 surfaces: the
/// weekly receipts digest, the demotion disclosure, and the explain
/// affordance (D5, D6, D7).
///
/// Rules, enforced by `RecoveryFramingTests` against this registry:
/// - Recoveries and completions lead; the wrong-intervention count appears
///   once, plainly; nothing here can render a streak, a broken chain, or a
///   failure tally (D8; roadmap invariant 6).
/// - Analyst voice (roadmap invariant 7): labels name evidence, never the
///   user's history, and never use absence framing.
/// - The explain affordance is one tap and one sentence — no label here may
///   suggest a reply, question flow, or conversation (D7).
///
/// Both card bodies (the digest headline and the demotion disclosure) are
/// Rust-authored and rendered verbatim; only these short structural labels
/// are Swift's.
public enum DigestFraming {
  // Weekly receipts digest (D6). Row order is the display order:
  // recoveries first, completions second.
  public static let digestTitle = "Weekly receipts"
  public static let returnedLabel = "Returned to your work"
  public static let completedLabel = "Blocks completed"
  public static let declaredLabel = "Blocks declared"
  public static let invitationsLabel = "Invitations accepted"
  public static let wrongLabel = "Wrong interventions"
  public static let withheldLabel = "Nudges Velvt chose not to send"
  public static let acknowledgeLabel = "Got it"

  // Demotion disclosure (D5). The body copy is Rust-authored.
  public static let demotionTitle = "Velvt has gone quiet"
  public static let resumeLabel = "Resume nudges"

  /// Inspectable demotion detail: the exact counts, versioned threshold,
  /// and window the deterministic rule evaluated. Numbers come from the
  /// service payload verbatim.
  public static func demotionDetail(_ state: DemotionState) -> String {
    "\(state.wrongCount) of \(state.deliveredCount) recent nudges were wrong · "
      + "threshold \(state.thresholdPercent)% over \(state.windowDays) days "
      + "(rule v\(state.thresholdPolicyVersion), resume rule v\(state.repromotionPolicyVersion))"
  }

  // The explain affordance (D7): one tap, one sentence, no reply.
  public static let explainLabel = "Explain this nudge"
}
