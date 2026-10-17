import SwiftUI

/// What the question after a block (proto v38) puts on a result surface.
///
/// Every case carries Rust's words, and Swift adds none. Rust decides when
/// the question is asked, when it expires and what each tap records
/// (`rust-service/src/self_rating/`); the coordinator only remembers the taps
/// it sent until the service's reply settles them.
public enum QuestionAfterABlock: Equatable {
    /// The question, above the block's result. Without `viaLink` it is up for
    /// the first time, and the block's observation and numbers stay hidden
    /// until an answer or Not now. With `viaLink` it was reopened from the
    /// link, after the result was already on screen.
    case question(RatingPrompt, viaLink: Bool)
    /// After Not now: the result shows, with this link under it.
    case link(RatingPrompt)
    /// After an answer: the acknowledgement, above the result.
    case acknowledged(String)
    /// The one-time invitation to turn the question on.
    case invitation(RatingOptIn)
}

/// An answer's acknowledgement, kept for the block it was given about, with
/// the deadline of the question it answered. The service refuses an answer
/// that reaches it at or after that deadline, and its reply then omits the
/// question just as it does after a recorded answer; only a reply that
/// arrives before the deadline tells the two apart.
struct RatingAcknowledgement: Equatable {
    let blockID: UUID
    let text: String
    let expiresAt: Date
}

extension WorkBlockPhase {
    /// Completed, abandoned or expired: the phases whose snapshot can carry
    /// the question after a block. Never active or paused, because the
    /// question is never drawn during a block.
    var hasEnded: Bool {
        switch self {
        case .completed, .abandoned, .expired: true
        case .idle, .active, .paused: false
        }
    }
}

/// The question after a block, drawn beside a block's result.
///
/// Two surfaces show a block's result, so two call sites draw this view: the
/// result view in the focus-session popover (`WorkBlockView`), and the Now
/// tab of the panel, whose fragmentation card summarises the same block. The
/// popover draws the parts that ask, and the acknowledgement, directly above
/// the result; the Now tab draws them first on the tab, above Today so far
/// and the insight. Both draw the link directly under the result. Only one of
/// the two is on screen with the question at a time: the panel leaves it to
/// the popover while the popover is open.
///
/// It is never a notification, and nothing here is drawn during a block. The
/// sighting it reports is a fact, sent only while the window drawing the
/// question is on screen, for the reason the drift card's is
/// (`WorkBlockProactiveCards`): the panel's view tree outlives the panel, and
/// an open popover can sit behind a locked screen.
public struct QuestionAfterABlockView: View {
    /// The parts of the question a call site draws in one place.
    public enum Part: Hashable {
        /// The question and the one-time invitation: what asks something.
        case ask
        /// "Noted.", after an answer.
        case acknowledgement
        /// The link under the result after Not now.
        case link
    }

    @ObservedObject private var coordinator: WorkBlockCoordinator
    private let parts: Set<Part>
    private let surfaceIsOnScreen: Bool

    public init(coordinator: WorkBlockCoordinator, parts: Set<Part>, surfaceIsOnScreen: Bool) {
        self.coordinator = coordinator
        self.parts = parts
        self.surfaceIsOnScreen = surfaceIsOnScreen
    }

    /// What this view draws right now. The only inputs are the coordinator's
    /// state and the parts this call site draws, so it can be asserted
    /// without a window.
    var presented: QuestionAfterABlock? {
        Self.presented(coordinator.questionAfterABlock, parts: parts)
    }

    static func presented(_ state: QuestionAfterABlock?, parts: Set<Part>) -> QuestionAfterABlock? {
        guard let state else { return nil }
        let part: Part =
            switch state {
            case .question, .invitation: .ask
            case .acknowledged: .acknowledgement
            case .link: .link
            }
        return parts.contains(part) ? state : nil
    }

    public var body: some View {
        switch presented {
        case .question(let prompt, _)?:
            questionCard(prompt)
                .onAppear { reportSightingIfOnScreen() }
                .onChange(of: surfaceIsOnScreen) { _ in reportSightingIfOnScreen() }
        case .acknowledged(let text)?:
            Text(text)
                .velvtBody(12)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .invitation(let optIn)?:
            invitationCard(optIn)
        case .link(let prompt)?:
            Button(prompt.link) { coordinator.openRatingLink() }
                .buttonStyle(.plain)
                .font(VelvtType.caption(10.5))
                .foregroundStyle(VelvtInk.secondaryOnInk)
                .frame(maxWidth: .infinity, alignment: .leading)
        case nil:
            EmptyView()
        }
    }

    /// Reports the question as seen when, and only when, the surface drawing
    /// it is on screen. The coordinator adds its own guard: nothing is sent
    /// unless the question is up for the first time.
    func reportSightingIfOnScreen() {
        guard surfaceIsOnScreen else { return }
        coordinator.reportRatingShown()
    }

    /// The question, its five answers, Not now and the footnote.
    ///
    /// The five answers are one equal column, from most of the block to
    /// least, as Rust orders them. None is filled or coloured, so none reads
    /// as the expected answer or as a grade, and Not now is a quiet text
    /// button beside them, as the instrument specifies.
    private func questionCard(_ prompt: RatingPrompt) -> some View {
        VelvtCard(padding: VelvtMetrics.spaceMD) {
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
                Text(prompt.question)
                    .velvtHeading(14)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)

                VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
                    ForEach(prompt.options, id: \.answer) { option in
                        Button {
                            coordinator.answerRating(option.answer)
                        } label: {
                            Text(option.label)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                    }
                }
                .disabled(coordinator.isSendingRatingAnswer)

                HStack(alignment: .firstTextBaseline, spacing: VelvtMetrics.spaceSM) {
                    Text(prompt.footnote)
                        .font(VelvtType.caption(10.5))
                        .foregroundStyle(VelvtInk.tertiaryOnInk)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                    Button(prompt.notNow) { coordinator.declineRatingForNow() }
                        .buttonStyle(VelvtQuietButtonStyle())
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(prompt.question)
    }

    /// The one-time invitation to turn the question on. Turn on and Not now
    /// are drawn alike: the surface's one crimson action is the result view's
    /// own, and an instrument that is always optional should not look like the
    /// next step. The tap is reported as which button it was.
    private func invitationCard(_ optIn: RatingOptIn) -> some View {
        VelvtCard(padding: VelvtMetrics.spaceMD) {
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
                Text(optIn.body)
                    .velvtBody(12)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: VelvtMetrics.spaceSM) {
                    Button(optIn.turnOn) { coordinator.respondToRatingInvitation(turnOn: true) }
                        .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                    Button(optIn.notNow) { coordinator.respondToRatingInvitation(turnOn: false) }
                        .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                    Spacer(minLength: 0)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(optIn.body)
    }
}

/// The Settings toggle for the question after a block (proto v38).
///
/// Off by default: it shows what the service reports and assumes nothing,
/// and no row means off. Drawn only while Rust says the switch is available,
/// which is once this Mac's first 14 days of blocks are over, or while the
/// question is on; before then the service would refuse on anyway. The title
/// and subtitle are Rust's. Turning it on here never answers the one-time
/// invitation; the service keeps the two apart.
public struct RatingPromptSettingsToggle: View {
    @ObservedObject private var coordinator: WorkBlockCoordinator

    public init(coordinator: WorkBlockCoordinator) {
        self.coordinator = coordinator
    }

    /// The setting this view draws, or `nil` while there is no switch to
    /// offer.
    var presentedSettings: RatingPromptSettings? {
        guard let settings = coordinator.ratingPromptSettings, settings.available else { return nil }
        return settings
    }

    public var body: some View {
        if let settings = presentedSettings {
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
                Toggle(
                    settings.title,
                    isOn: Binding(
                        get: { coordinator.ratingPromptSettings?.enabled ?? false },
                        set: { coordinator.setRatingPromptEnabled($0) }
                    )
                )
                .toggleStyle(.switch)
                .font(VelvtType.body(12))

                Text(settings.subtitle)
                    .font(VelvtType.caption(10.5))
                    .foregroundStyle(VelvtInk.tertiaryOnInk)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 12)
            .accessibilityElement(children: .contain)
        }
    }
}
