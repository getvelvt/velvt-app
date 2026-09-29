import SwiftUI

/// The needs-a-category card (protocol 33), drawn in the panel's proactive
/// stack above every tab, beside the drift offer and the soft-start
/// invitation.
///
/// The words are Rust's, verbatim: a count of the sites and apps on the list
/// and never a name. The primary action opens Settings → Apps & Sites and
/// answers `opened`; the secondary answers `not_now`. Either answer closes the
/// card until something new joins the list.
///
/// Never drawn while a work block is active or paused. The service withholds
/// the card then too, but a card it sent a moment before the block started is
/// still in hand, and a focus session is the one time nothing may interrupt.
public struct CategoryPromptCardView: View {
    @ObservedObject private var coordinator: CategoryPromptCoordinator
    @ObservedObject private var workBlockCoordinator: WorkBlockCoordinator
    private let onOpen: () -> Void

    /// - Parameter onOpen: takes the panel to the needs-a-category list.
    public init(
        coordinator: CategoryPromptCoordinator,
        workBlockCoordinator: WorkBlockCoordinator,
        onOpen: @escaping () -> Void
    ) {
        self.coordinator = coordinator
        self.workBlockCoordinator = workBlockCoordinator
        self.onOpen = onOpen
    }

    /// The card this view draws right now, or `nil`. The only inputs are the
    /// two coordinators' state, so it can be asserted without a window.
    var presentedPrompt: PresentedCategoryPrompt? {
        Self.presentedPrompt(coordinator.prompt, during: workBlockCoordinator.snapshot?.phase)
    }

    static func presentedPrompt(
        _ prompt: PresentedCategoryPrompt?,
        during phase: WorkBlockPhase?
    ) -> PresentedCategoryPrompt? {
        switch phase {
        case .active, .paused:
            return nil
        case .idle, .completed, .abandoned, .expired, nil:
            return prompt
        }
    }

    public var body: some View {
        if let prompt = presentedPrompt {
            card(prompt.card)
        }
    }

    private func card(_ card: CategoryPromptCard) -> some View {
        VelvtCard(padding: VelvtMetrics.spaceMD) {
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
                Label(card.title, systemImage: "tag")
                    .velvtHeading(14)

                Text(card.body)
                    .velvtBody(12)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: VelvtMetrics.spaceSM) {
                    Button(card.primaryAction) {
                        coordinator.open()
                        onOpen()
                    }
                    .buttonStyle(VelvtPrimaryButtonStyle())
                    .accessibilityHint("Opens the list of apps and sites that need a category in Settings")

                    Button(card.secondaryAction) {
                        coordinator.notNow()
                    }
                    .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                    .accessibilityHint("Closes this card until something new needs a category")

                    Spacer(minLength: 0)
                }
            }
        }
        .padding([.horizontal, .top], VelvtMetrics.cardPadding)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(card.title). \(card.body)")
    }
}
