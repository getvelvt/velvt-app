import SwiftUI

/// Shows the one thing Velvt does, before the user has to earn it.
///
/// The drift offer is the product, and it is gated behind evidence (drift
/// policy version 3): a block running past a three-minute warm-up, three
/// confident switches inside ten minutes, and two minutes still remaining.
/// Plenty of people will use Velvt for a week without ever meeting those
/// conditions, and conclude it does nothing — the app is inert until it is
/// not, and there is no way to tell the difference from the outside.
///
/// So this shows the offer once, up front, clearly labelled as an example.
/// Nobody can judge an intervention they have never seen, and "here is the one
/// thing this does" argues better than any paragraph about it.
///
/// The card is deliberately built from the same pieces as the live one in
/// `WorkBlockView.interventionCard`: same title weight, same body treatment,
/// same reply vocabulary in the same order. A preview that flatters the real
/// thing would be a lie the first real nudge exposes.
public struct NudgePreviewView: View {
    private let onContinue: () -> Void

    public init(onContinue: @escaping () -> Void) {
        self.onContinue = onContinue
    }

    /// Copy mirrors what Rust authors for a real offer: an observed count, a
    /// window, and one bounded action. No praise, no diagnosis, no history.
    private static let exampleTitle = "Your work block is running"
    private static let exampleBody =
        "Velvt observed 4 switches away from focus work in the last 10 minutes. "
        + "Protect the next 10 minutes for the work you chose."

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("This is the whole product")
                .velvtDisplay(24)

            Text(
                "When Velvt is confident you have drifted from a block you started, "
                    + "it says so once. This is what that looks like."
            )
            .velvtBody(13)
            .fixedSize(horizontal: false, vertical: true)

            exampleCard
                .overlay(alignment: .topTrailing) {
                    Text("Example")
                        .font(VelvtType.label(9.5))
                        .tracking(VelvtType.labelTracking)
                        .textCase(.uppercase)
                        .foregroundStyle(VelvtInk.labelOnPaper)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(VelvtSurface.blushInset, in: Capsule())
                        .padding(VelvtMetrics.spaceSM)
                }

            Text(
                "At most one per block, and never outside one. Whichever reply is honest "
                    + "is the right one — telling Velvt it was wrong is how it stops being wrong."
            )
            .velvtBody(12)
            .fixedSize(horizontal: false, vertical: true)

            HStack {
                Spacer()
                Button("Got it") { onContinue() }
                    .buttonStyle(VelvtPrimaryButtonStyle(uppercase: true))
                    .keyboardShortcut(.defaultAction)
            }
        }
        .frame(maxWidth: 460, alignment: .leading)
        .padding(28)
        // The ground is ink and never adapts to system appearance, so the
        // window carries it rather than borrowing whatever the OS proposes.
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(VelvtSurface.ground)
    }

    /// Inert by construction: no coordinator, no bindings, nothing to press.
    /// The buttons are shown disabled so the shape and the reply vocabulary
    /// read true without implying a live offer is waiting.
    ///
    /// Paper stock, because a drift offer is a sentence addressed to the person.
    private var exampleCard: some View {
        VelvtPaperCard(padding: VelvtMetrics.spaceMD) {
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
                Text(Self.exampleTitle)
                    .velvtHeading(14, onPaper: true)

                Text(Self.exampleBody)
                    .velvtBody(12, onPaper: true)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: VelvtMetrics.spaceSM) {
                    Button("Back to work") {}
                        .buttonStyle(VelvtPrimaryButtonStyle(uppercase: true))
                    Spacer(minLength: 0)
                    Image(systemName: "xmark")
                        .font(VelvtType.caption())
                        .foregroundStyle(VelvtInk.tertiaryOnPaper)
                }

                HStack(spacing: VelvtMetrics.spaceMD) {
                    Text("I was focused")
                    Text("Wrong category")
                    Text("Not helpful")
                }
                .font(VelvtType.caption())
                .foregroundStyle(VelvtInk.secondaryOnPaper)
            }
        }
        .disabled(true)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "Example drift offer. \(Self.exampleTitle). \(Self.exampleBody) "
                + "Replies: Back to work, I was focused, Wrong category, Not helpful."
        )
    }
}
