import AppKit
import SwiftUI

public enum OnboardingCopy {
    public static let privacySummary =
        "Raw app names, window titles, URLs, filenames, paths, contacts, and work-block intentions stay on this Mac. Approved broad categories, coarse durations, timestamps, and safe summaries may synchronize for beta insights. Depending on the service configuration, privacy-safe derived prompts may be processed by an approved external model provider."
}

public enum IntroStep: Int, CaseIterable, Equatable, Sendable {
    case welcome
    case privacy
    case capabilities
    case ready
    case quickStart
}

public enum OnboardingWindowLayout {
    public static let preferredContentSize = CGSize(width: 720, height: 520)
    public static let minimumContentSize = CGSize(width: 520, height: 400)
    public static let screenInset: CGFloat = 48

    public static func contentSize(for visibleFrame: CGRect?) -> CGSize {
        guard let visibleFrame else { return preferredContentSize }
        return CGSize(
            width: min(preferredContentSize.width, max(1, visibleFrame.width - screenInset)),
            height: min(preferredContentSize.height, max(1, visibleFrame.height - screenInset))
        )
    }
}

@MainActor
public final class IntroFlowModel: ObservableObject {
    @Published public private(set) var step: IntroStep = .welcome

    private let persistCompletion: () -> Void
    private let startUsing: () -> Void
    private let startTour: () -> Void

    public init(
        persistCompletion: @escaping () -> Void,
        startUsing: @escaping () -> Void,
        startTour: @escaping () -> Void
    ) {
        self.persistCompletion = persistCompletion
        self.startUsing = startUsing
        self.startTour = startTour
    }

    public var canGoBack: Bool {
        step != .welcome && step != .quickStart
    }

    public func continueForward() {
        guard step != .quickStart else { return }
        step = IntroStep(rawValue: step.rawValue + 1) ?? .ready
    }

    public func goBack() {
        guard canGoBack else { return }
        step = IntroStep(rawValue: step.rawValue - 1) ?? .welcome
    }

    public func skipIntro() {
        persistCompletion()
        step = .quickStart
    }

    public func showFullIntro() {
        step = .welcome
    }

    public func finishAndStartUsing() {
        persistCompletion()
        startUsing()
    }

    public func finishAndStartTour() {
        persistCompletion()
        startTour()
    }
}

@MainActor
public final class AccessibilityPromptModel: ObservableObject {
    @Published public private(set) var hasRequested = false
    @Published public private(set) var isRequesting = false

    private let presentation: PermissionPresentationModel
    private let permissionManager: any PermissionManagerProtocol
    private let onContinue: () -> Void

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: any PermissionManagerProtocol,
        onContinue: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.onContinue = onContinue
    }

    public var status: PermissionStatus {
        presentation.statuses[.accessibility] ?? .unknown
    }

    public var canContinue: Bool {
        status == .granted && !isRequesting
    }

    public func request() async {
        guard !isRequesting, status != .granted else { return }
        presentation.markPermissionRequested(.accessibility)
        hasRequested = true

        switch status {
        case .denied, .restricted:
            PermissionRecoveryView.openAccessibilitySettings()
        case .unknown:
            isRequesting = true
            _ = await permissionManager.requestPermission(for: .accessibility)
            isRequesting = false
        case .granted:
            break
        }
    }

    public func skip() {
        guard status == .granted else { return }
        onContinue()
    }

    public func continueToWalkthrough() {
        guard canContinue, !isRequesting else { return }
        onContinue()
    }
}

@MainActor
public final class NotificationPromptModel: ObservableObject {
    @Published public private(set) var isRequesting = false

    private let presentation: PermissionPresentationModel
    private let permissionManager: any PermissionManagerProtocol
    private let onContinue: () -> Void

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: any PermissionManagerProtocol,
        onContinue: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.onContinue = onContinue
    }

    public var status: PermissionStatus {
        presentation.statuses[.notifications] ?? .unknown
    }

    public func requestAndContinue() async {
        guard !isRequesting else { return }
        presentation.markPermissionRequested(.notifications)

        switch status {
        case .unknown:
            isRequesting = true
            _ = await permissionManager.requestPermission(for: .notifications)
            isRequesting = false
            onContinue()
        case .granted:
            onContinue()
        case .denied, .restricted:
            Self.openNotificationSettings()
        }
    }

    public func skip() {
        presentation.markPermissionRequested(.notifications)
        onContinue()
    }

    public static func openNotificationSettings() {
        guard let url = URL(
            string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
        ) else { return }
        NSWorkspace.shared.open(url)
    }
}

/// The single Focus-allowance ask (roadmap invariant 8; D2).
///
/// Velvt asks once whether the user wants to let it through their work
/// Focus mode — by adding Velvt in macOS Focus settings themselves — and
/// requests the coarse Focus-status read so suppressed deliveries can be
/// reconciled honestly. Velvt never reads or writes the Focus
/// configuration: the deep link opens the user's own settings and the
/// change is entirely theirs. Declining changes nothing else, and the ask
/// is remembered so it never repeats.
@MainActor
public final class FocusAllowancePromptModel: ObservableObject {
    public static let askedDefaultsKey = "focusAllowanceAsked"

    @Published public private(set) var hasRequested = false
    @Published public private(set) var isRequesting = false

    private let defaults: UserDefaults
    private let requestFocusAuthorization: (@escaping (Bool) -> Void) -> Void
    private let openFocusSettings: @MainActor () -> Void
    private let onContinue: () -> Void

    public init(
        defaults: UserDefaults = .standard,
        requestFocusAuthorization: @escaping (@escaping (Bool) -> Void) -> Void =
            INFocusStatusProvider.requestAuthorization,
        openFocusSettings: @escaping @MainActor () -> Void =
            { FocusAllowancePromptModel.openSystemFocusSettings() },
        onContinue: @escaping () -> Void
    ) {
        self.defaults = defaults
        self.requestFocusAuthorization = requestFocusAuthorization
        self.openFocusSettings = openFocusSettings
        self.onContinue = onContinue
    }

    /// True once the one ask has been made, on this or any earlier launch.
    public var hasBeenAsked: Bool {
        defaults.bool(forKey: Self.askedDefaultsKey)
    }

    /// The affirmative path: request the coarse Focus-status read once and
    /// open the user's Focus settings so they can add Velvt themselves.
    public func allowAndOpenFocusSettings() {
        guard !isRequesting else { return }
        defaults.set(true, forKey: Self.askedDefaultsKey)
        hasRequested = true
        isRequesting = true
        requestFocusAuthorization { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.isRequesting = false
                self.openFocusSettings()
            }
        }
    }

    /// Declining is one tap and changes nothing else about the product.
    public func skip() {
        defaults.set(true, forKey: Self.askedDefaultsKey)
        onContinue()
    }

    public func finish() {
        defaults.set(true, forKey: Self.askedDefaultsKey)
        onContinue()
    }

    public static func openSystemFocusSettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.Focus-Settings.extension")
        else { return }
        NSWorkspace.shared.open(url)
    }
}

public enum GuidedTourStep: Int, CaseIterable, Equatable, Sendable {
    case today
    case earlySignal
    case focusFragmentation
    case dailyActivity
    case statusAndRecovery
    case settings

    public var title: String {
        switch self {
        case .today: "Start a focus session"
        case .earlySignal: "Today's insight"
        case .focusFragmentation: "Focus fragmentation"
        case .dailyActivity: "Patterns"
        case .statusAndRecovery: "Collection status"
        case .settings: "Settings"
        }
    }

    public var detail: String {
        switch self {
        case .today:
            "Choose a work type and duration, then start a session. Your intention stays local."
        case .earlySignal:
            "A privacy-safe insight appears here as enough local evidence becomes available."
        case .focusFragmentation:
            "Review broad context changes within your current focus session."
        case .dailyActivity:
            // The seven-day activity chart this step used to point at has been
            // retired; the correction rows it carried now live in Settings.
            //
            // This line must describe what the tab RENDERS, which today is
            // WeekOverWeekCoachingView and nothing else. An earlier draft
            // promised "one thing Velvt has noticed about how you work" — that
            // is the antecedent card, which is shadow-only and reads nothing
            // into this tab. Onboarding is the first thing a user reads; a
            // promise it cannot keep on day one is the exact failure this
            // product's thesis cannot survive. Change this line when the
            // antecedent surface actually lands, not before.
            "Your week set against the one before it, once there is enough local evidence to compare."
        case .statusAndRecovery:
            "See whether local collection and cloud synchronization need attention."
        case .settings:
            "Manage collection, teach Velvt which apps are which, your account, and this tour."
        }
    }
}

public final class GuidedTourModel: ObservableObject {
    @Published public private(set) var isPresented = false
    @Published public private(set) var step: GuidedTourStep = .today

    public init() {}

    public var canGoBack: Bool { step != .today }
    public var isLastStep: Bool { step == .settings }
    public var progressLabel: String { "\(step.rawValue + 1) of \(GuidedTourStep.allCases.count)" }

    public func start() {
        step = .today
        isPresented = true
    }

    public func goBack() {
        guard canGoBack else { return }
        step = GuidedTourStep(rawValue: step.rawValue - 1) ?? .today
    }

    public func advance() {
        guard !isLastStep else {
            dismiss()
            return
        }
        step = GuidedTourStep(rawValue: step.rawValue + 1) ?? .settings
    }

    public func dismiss() {
        isPresented = false
    }
}

/// The hairline between header, page, and footer.
///
/// `Divider` takes its colour from the OS appearance, and the brand ground
/// deliberately does not follow the OS. This is the guide's "clear trace":
/// bright enough to bound a surface, never heavy enough to box the reader in.
private struct VelvtHairline: View {
    var body: some View {
        Rectangle()
            .fill(VelvtSurface.strokeOnInk)
            .frame(height: VelvtMetrics.hairline)
    }
}

public struct FirstRunExperienceView: View {
    @ObservedObject private var model: IntroFlowModel
    private let followsLaunchSequence: Bool
    private let continuesToTour: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        model: IntroFlowModel,
        followsLaunchSequence: Bool = false,
        continuesToTour: Bool = false
    ) {
        self.model = model
        self.followsLaunchSequence = followsLaunchSequence
        self.continuesToTour = continuesToTour
    }

    public var body: some View {
        VStack(spacing: 0) {
            header
            VelvtHairline()
            ScrollView {
                page
                    .frame(maxWidth: 620, alignment: .leading)
                    .padding(36)
            }
            VelvtHairline()
            footer
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(VelvtSurface.ground)
        .foregroundStyle(VelvtInk.primaryOnInk)
        .tint(VelvtPalette.crimson)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.18), value: model.step)
        .onExitCommand {
            if model.step == .quickStart {
                model.finishAndStartUsing()
            } else if followsLaunchSequence {
                model.finishAndStartUsing()
            } else {
                model.skipIntro()
            }
        }
    }

    private var header: some View {
        HStack(spacing: VelvtMetrics.spaceMD) {
            Image("VelvtWordmark")
                .resizable()
                .renderingMode(.template)
                .interpolation(.high)
                .scaledToFit()
                .frame(width: 92, height: 36, alignment: .leading)
                .foregroundStyle(VelvtInk.primaryOnInk)
                .accessibilityLabel("Velvt")
            Spacer()
            if model.step != .quickStart {
                Text("Step \(model.step.rawValue + 1) of \(introStepCount)")
                    .font(VelvtType.label())
                    .tracking(VelvtType.labelTracking)
                    .textCase(.uppercase)
                    .foregroundStyle(VelvtInk.labelOnInk)
                    .accessibilityLabel("Intro step \(model.step.rawValue + 1) of \(introStepCount)")
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 16)
    }

    @ViewBuilder private var page: some View {
        switch model.step {
        case .welcome:
            IntroPage(
                systemImage: "hand.raised.fill",
                // "Velvt learns how your attention breaks" was the first
                // sentence a new user read, and it claimed a capability the
                // shipped product does not have — banned outright by
                // GOAL.md. What Velvt actually does is notice one thing, and
                // that is a better promise than the one it was overstating.
                title:
                    "Velvt notices the moment you leave the work you sat down to do.",
                body:
                    "When it does, it offers one way back before the block is lost. Raw work context never leaves your Mac."
            )
        case .privacy:
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceLG) {
                IntroPage(
                    systemImage: "lock.shield",
                    title: "Private by design, clear about synchronization.",
                    body:
                        "Velvt can build broad, privacy-safe patterns from the active work context. Raw activity details stay on this Mac."
                )
                Text(OnboardingCopy.privacySummary)
                    .velvtBody(13)
                    .fixedSize(horizontal: false, vertical: true)
            }
        case .capabilities:
            VStack(alignment: .leading, spacing: VelvtMetrics.spaceLG) {
                IntroPage(
                    systemImage: "sparkles",
                    title: "One nudge, at the moment it helps.",
                    body:
                        "Velvt interrupts at most once per block, and only when the evidence is unambiguous. It counts the times you came back, never the times you did not."
                )
                capability(
                    "timer", "Start a meaningful work block with an optional local intention.")
                capability(
                    "waveform.path.ecg",
                    "See an early local signal without waiting for a seven-day baseline."
                )
                capability(
                    "arrow.triangle.2.circlepath",
                    "Understand broad context switching and uninterrupted stretches.")
                capability("leaf", "Take one modest recovery action.")
                // "Focus Fragmentation" is the name of a metric, and a
                // metric name on the first screen teaches the reader that
                // this is a thing that measures them.
                capability(
                    "calendar",
                    "Look back at how a block went, with no score and no verdict.")
            }
        case .ready:
            IntroPage(
                systemImage: "checkmark.circle.fill",
                title: "Take a quick look around.",
                body:
                    "The guided tour opens the live menu-bar interface and points to the controls you will use."
            )
        case .quickStart:
            VStack(alignment: .leading, spacing: 20) {
                IntroPage(
                    systemImage: "bolt.fill",
                    title: "Velvt in 30 seconds",
                    body: "Three things are enough to begin."
                )
                numberedPoint(1, "Start a work block.")
                numberedPoint(2, "Raw activity stays local.")
                numberedPoint(3, "Today shows an early local signal as evidence becomes available.")
            }
        }
    }

    private var footer: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: VelvtMetrics.spaceMD) {
                secondaryActions
                Spacer()
                primaryActions
            }
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: VelvtMetrics.spaceMD) {
                    secondaryActions
                }
                HStack(spacing: VelvtMetrics.spaceMD) {
                    Spacer()
                    primaryActions
                }
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 16)
    }

    @ViewBuilder private var secondaryActions: some View {
        if model.step == .quickStart {
            Button("View full intro") { model.showFullIntro() }
                .buttonStyle(VelvtQuietButtonStyle())
        } else {
            Button("Skip intro") {
                if followsLaunchSequence {
                    model.finishAndStartUsing()
                } else {
                    model.skipIntro()
                }
            }
                .buttonStyle(VelvtQuietButtonStyle())
            if model.canGoBack {
                Button("Back") { model.goBack() }
                    .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
            }
        }
    }

    @ViewBuilder private var primaryActions: some View {
        switch model.step {
        case .ready:
            if followsLaunchSequence {
                Button("Continue to Accessibility") { model.finishAndStartUsing() }
                    .buttonStyle(VelvtPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
            } else {
                Button("Skip tour and start using") { model.finishAndStartUsing() }
                    .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                Button("Start guided tour") { model.finishAndStartTour() }
                    .buttonStyle(VelvtPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
            }
        case .quickStart:
            Button(
                followsLaunchSequence
                    ? (continuesToTour ? "Start guided tour" : "Continue setup")
                    : "Start using Velvt"
            ) { model.finishAndStartUsing() }
                .buttonStyle(VelvtPrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
        case .capabilities where followsLaunchSequence:
            Button(continuesToTour ? "Start guided tour" : "Continue setup") {
                model.finishAndStartUsing()
            }
                .buttonStyle(VelvtPrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
        default:
            Button("Continue") { model.continueForward() }
                .buttonStyle(VelvtPrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
        }
    }

    private var introStepCount: Int {
        followsLaunchSequence ? 3 : 4
    }

    private func capability(_ systemImage: String, _ text: String) -> some View {
        Label {
            Text(text).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: systemImage)
                .foregroundStyle(VelvtPalette.signal)
                .frame(width: 24)
        }
        .velvtBody(13)
    }

    private func numberedPoint(_ number: Int, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 14) {
            Text("\(number)")
                .font(VelvtType.measurement(15))
                .foregroundStyle(VelvtInk.measurementOnInk)
            Text(text)
                .velvtBody(13)
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
    }
}

private struct IntroPage: View {
    let systemImage: String
    let title: String
    let detail: String

    init(systemImage: String, title: String, body: String) {
        self.systemImage = systemImage
        self.title = title
        detail = body
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Image(systemName: systemImage)
                .font(VelvtType.display(34))
                .foregroundStyle(VelvtPalette.signal)
                .accessibilityHidden(true)
            Text(title)
                .velvtDisplay()
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            Text(detail)
                .velvtBody(15)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

public struct AccessibilityPermissionExperienceView: View {
    @ObservedObject private var model: AccessibilityPromptModel
    @ObservedObject private var presentation: PermissionPresentationModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        model: AccessibilityPromptModel,
        presentation: PermissionPresentationModel
    ) {
        self.model = model
        self.presentation = presentation
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: VelvtMetrics.spaceMD) {
                Image("VelvtWordmark")
                    .resizable()
                    .renderingMode(.template)
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: 92, height: 36, alignment: .leading)
                    .foregroundStyle(VelvtInk.primaryOnInk)
                    .accessibilityLabel("Velvt")
                Spacer()
                VelvtEyebrow("Accessibility")
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)

            VelvtHairline()

            ScrollView {
                VStack(alignment: .leading, spacing: VelvtMetrics.spaceLG) {
                    Image(systemName: "accessibility")
                        .font(VelvtType.display(34))
                        .foregroundStyle(VelvtPalette.signal)
                        .accessibilityHidden(true)
                    Text("Allow Accessibility after the intro.")
                        .velvtDisplay()
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                    Text(
                        "This lets Velvt notice broad changes in the active work context. It does not send raw app names, window titles, URLs, filenames, or paths to the cloud."
                    )
                    .velvtBody(15)
                    .fixedSize(horizontal: false, vertical: true)

                    if currentStatus == .granted {
                        Label("Accessibility is already allowed.", systemImage: "checkmark.circle.fill")
                            .font(VelvtType.body())
                            .foregroundStyle(VelvtInk.affirmative)
                    } else if model.hasRequested {
                        VelvtCard {
                            Text(
                                "Accessibility is not enabled yet. Allow Velvt in System Settings, then return here to continue."
                            )
                            .velvtBody(13)
                            .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .frame(maxWidth: 620, alignment: .leading)
                .padding(36)
            }

            VelvtHairline()

            HStack(spacing: VelvtMetrics.spaceMD) {
                Text("Accessibility is required for Velvt to observe broad context changes.")
                    .font(VelvtType.caption())
                    .foregroundStyle(VelvtInk.tertiaryOnInk)
                Spacer()
                actionButtons
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(VelvtSurface.ground)
        .foregroundStyle(VelvtInk.primaryOnInk)
        .tint(VelvtPalette.crimson)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.18), value: model.hasRequested)
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder private var actionButtons: some View {
        if model.canContinue {
            Button("Start Local Collection") { model.continueToWalkthrough() }
                .buttonStyle(VelvtPrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
        } else {
            Button(requestActionLabel) {
                Task { await model.request() }
            }
            .buttonStyle(VelvtPrimaryButtonStyle())
            .disabled(model.isRequesting)
            .keyboardShortcut(.defaultAction)
            .accessibilityHint("Requests macOS Accessibility access")
        }
    }

    private var currentStatus: PermissionStatus {
        presentation.statuses[.accessibility] ?? model.status
    }

    private var requestActionLabel: String {
        switch currentStatus {
        case .denied, .restricted:
            "Open Accessibility Settings"
        case .unknown:
            "Allow Accessibility"
        case .granted:
            "Accessibility Allowed"
        }
    }
}

public struct NotificationPermissionExperienceView: View {
    @ObservedObject private var model: NotificationPromptModel
    @ObservedObject private var presentation: PermissionPresentationModel

    public init(
        model: NotificationPromptModel,
        presentation: PermissionPresentationModel
    ) {
        self.model = model
        self.presentation = presentation
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: VelvtMetrics.spaceMD) {
                Image("VelvtWordmark")
                    .resizable()
                    .renderingMode(.template)
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: 92, height: 36, alignment: .leading)
                    .foregroundStyle(VelvtInk.primaryOnInk)
                    .accessibilityLabel("Velvt")
                Spacer()
                VelvtEyebrow("Notifications")
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)

            VelvtHairline()

            VStack(alignment: .leading, spacing: VelvtMetrics.spaceLG) {
                Image(systemName: "bell.badge")
                    .font(VelvtType.display(34))
                    .foregroundStyle(VelvtPalette.signal)
                    .accessibilityHidden(true)
                Text("Allow insight notifications.")
                    .velvtDisplay()
                    .accessibilityAddTraits(.isHeader)
                Text(
                    "Velvt can notify you when a privacy-safe insight is ready. Notifications contain only broad observations—never app names, window titles, URLs, filenames, or paths."
                )
                .velvtBody(15)
                .fixedSize(horizontal: false, vertical: true)

                if currentStatus == .granted {
                    Label("Notifications are already allowed.", systemImage: "checkmark.circle.fill")
                        .font(VelvtType.body())
                        .foregroundStyle(VelvtInk.affirmative)
                } else if currentStatus == .denied || currentStatus == .restricted {
                    VelvtCard {
                        Text("Notifications are disabled. You can enable them in System Settings.")
                            .velvtBody(13)
                    }
                }
            }
            .frame(maxWidth: 620, maxHeight: .infinity, alignment: .topLeading)
            .padding(36)

            VelvtHairline()

            HStack(spacing: VelvtMetrics.spaceMD) {
                Button("Not now") { model.skip() }
                    .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                Spacer()
                Button(actionLabel) {
                    Task { await model.requestAndContinue() }
                }
                .buttonStyle(VelvtPrimaryButtonStyle())
                .disabled(model.isRequesting)
                .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(VelvtSurface.ground)
        .foregroundStyle(VelvtInk.primaryOnInk)
        .tint(VelvtPalette.crimson)
        .accessibilityElement(children: .contain)
    }

    private var actionLabel: String {
        switch currentStatus {
        case .unknown: "Allow Notifications"
        case .granted: "Continue"
        case .denied, .restricted: "Open Notification Settings"
        }
    }

    private var currentStatus: PermissionStatus {
        presentation.statuses[.notifications] ?? model.status
    }
}

/// The one-time Focus-allowance step. It states the budget promise plainly
/// and hands the actual change to the user's own macOS Focus settings —
/// Velvt never alters a Focus configuration programmatically.
public struct FocusAllowanceExperienceView: View {
    @ObservedObject private var model: FocusAllowancePromptModel

    public init(model: FocusAllowancePromptModel) {
        self.model = model
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: VelvtMetrics.spaceMD) {
                Image("VelvtWordmark")
                    .resizable()
                    .renderingMode(.template)
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: 92, height: 36, alignment: .leading)
                    .foregroundStyle(VelvtInk.primaryOnInk)
                    .accessibilityLabel("Velvt")
                Spacer()
                VelvtEyebrow("Focus")
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)

            VelvtHairline()

            VStack(alignment: .leading, spacing: VelvtMetrics.spaceLG) {
                Image(systemName: "moon.circle")
                    .font(VelvtType.display(34))
                    .foregroundStyle(VelvtPalette.signal)
                    .accessibilityHidden(true)
                Text("Let Velvt through your work Focus?")
                    .velvtDisplay()
                    .accessibilityAddTraits(.isHeader)
                Text(
                    "If you use a Focus mode while you work, you can allow Velvt through it in macOS Focus settings. Velvt spends that privilege sparingly: at most one bounded nudge inside a block you declared, and anything your Focus blocks is simply held and summarized after the block — never resent, never rerouted."
                )
                .velvtBody(15)
                .fixedSize(horizontal: false, vertical: true)
                VelvtInsetPanel {
                    Text(
                        "The change is yours to make in System Settings; Velvt never edits your Focus modes. Velvt only asks macOS whether some Focus is on — never which one, or what it allows. Declining changes nothing else."
                    )
                    .velvtBody(13, onPaper: true)
                    .fixedSize(horizontal: false, vertical: true)
                }

                if model.hasRequested {
                    VelvtCard {
                        Text(
                            "In System Settings, open your work Focus, then add Velvt under Allowed Notifications. Return here when you are done."
                        )
                        .velvtBody(13)
                        .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .frame(maxWidth: 620, maxHeight: .infinity, alignment: .topLeading)
            .padding(36)

            VelvtHairline()

            HStack(spacing: VelvtMetrics.spaceMD) {
                Button("Not now") { model.skip() }
                    .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                    .accessibilityHint("Continues without changing anything")
                Spacer()
                if model.hasRequested {
                    Button("Continue") { model.finish() }
                        .buttonStyle(VelvtPrimaryButtonStyle())
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Open Focus Settings") { model.allowAndOpenFocusSettings() }
                        .buttonStyle(VelvtPrimaryButtonStyle())
                        .disabled(model.isRequesting)
                        .keyboardShortcut(.defaultAction)
                        .accessibilityHint(
                            "Asks for the coarse Focus-status read and opens your Focus settings")
                }
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(VelvtSurface.ground)
        .foregroundStyle(VelvtInk.primaryOnInk)
        .tint(VelvtPalette.crimson)
        .accessibilityElement(children: .contain)
    }
}

public struct TourInvitationExperienceView: View {
    private let onContinue: () -> Void
    private let onStartTour: () -> Void

    public init(
        onContinue: @escaping () -> Void,
        onStartTour: @escaping () -> Void
    ) {
        self.onContinue = onContinue
        self.onStartTour = onStartTour
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: VelvtMetrics.spaceMD) {
                Image("VelvtWordmark")
                    .resizable()
                    .renderingMode(.template)
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: 92, height: 36, alignment: .leading)
                    .foregroundStyle(VelvtInk.primaryOnInk)
                    .accessibilityLabel("Velvt")
                Spacer()
                VelvtEyebrow("Ready")
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)

            VelvtHairline()

            IntroPage(
                systemImage: "checkmark.circle.fill",
                title: "Take a quick look around.",
                body:
                    "The guided tour opens the live menu-bar interface and points to the controls you will use."
            )
            .frame(maxWidth: 620, maxHeight: .infinity, alignment: .topLeading)
            .padding(36)

            VelvtHairline()

            HStack(spacing: VelvtMetrics.spaceMD) {
                Button("Start using Velvt", action: onContinue)
                    .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
                Spacer()
                Button("Show me around", action: onStartTour)
                    .buttonStyle(VelvtPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(VelvtSurface.ground)
        .foregroundStyle(VelvtInk.primaryOnInk)
        .tint(VelvtPalette.crimson)
    }
}

public struct OnboardingAccountExperienceView: View {
    @ObservedObject private var accountStateManager: AccountStateManager
    @StateObject private var authViewModel: AuthViewModel
    private let onAuthenticated: () -> Void

    public init(
        accountStateManager: AccountStateManager,
        authViewModel: AuthViewModel,
        onAuthenticated: @escaping () -> Void
    ) {
        self.accountStateManager = accountStateManager
        _authViewModel = StateObject(wrappedValue: authViewModel)
        self.onAuthenticated = onAuthenticated
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: VelvtMetrics.spaceMD) {
                Image("VelvtWordmark")
                    .resizable()
                    .renderingMode(.template)
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: 92, height: 36, alignment: .leading)
                    .foregroundStyle(VelvtInk.primaryOnInk)
                    .accessibilityLabel("Velvt")
                Spacer()
                VelvtEyebrow("Account")
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)

            VelvtHairline()

            VStack(alignment: .leading, spacing: VelvtMetrics.spaceLG) {
                Image(systemName: "person.crop.circle.badge.plus")
                    .font(VelvtType.display(34))
                    .foregroundStyle(VelvtPalette.signal)
                    .accessibilityHidden(true)
                Text(authViewModel.authMode == .signUp ? "Create your Velvt account." : "Welcome back.")
                    .velvtDisplay()
                    .accessibilityAddTraits(.isHeader)
                Text("Your account keeps private history and insight delivery connected across sessions.")
                    .velvtBody(15)
                    .fixedSize(horizontal: false, vertical: true)

                CredentialTextField(placeholder: "Email", text: $authViewModel.email)
                CredentialTextField(
                    placeholder: "Password",
                    text: $authViewModel.password,
                    isSecure: true
                )

                if authViewModel.authMode == .logIn {
                    Button("Forgot password?") {
                        authViewModel.openForgotPasswordPage()
                    }
                    .buttonStyle(VelvtQuietButtonStyle())
                    .accessibilityHint("Opens getvelvt.com in your browser")
                }

                if let error = authViewModel.errorMessage {
                    Text(error)
                        .font(VelvtType.caption())
                        .foregroundStyle(VelvtPalette.signal)
                }

                Button(
                    authViewModel.authMode == .signUp
                        ? "I already have an account"
                        : "Create a new account"
                ) {
                    authViewModel.toggleAuthMode()
                }
                .buttonStyle(VelvtQuietButtonStyle())
            }
            .frame(maxWidth: 620, maxHeight: .infinity, alignment: .topLeading)
            .padding(36)

            VelvtHairline()

            HStack {
                if authViewModel.connectionStatus != .connected {
                    Text("Waiting for the local Velvt service…")
                        .font(VelvtType.caption())
                        .foregroundStyle(VelvtInk.tertiaryOnInk)
                }
                Spacer()
                Button(authViewModel.authMode == .signUp ? "Create Account" : "Sign In") {
                    Task {
                        if authViewModel.authMode == .signUp {
                            await authViewModel.signUp()
                        } else {
                            await authViewModel.logIn()
                        }
                    }
                }
                .buttonStyle(VelvtPrimaryButtonStyle())
                .disabled(!authViewModel.canSubmitCredentials)
                .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(VelvtSurface.ground)
        .foregroundStyle(VelvtInk.primaryOnInk)
        .tint(VelvtPalette.crimson)
        .onChange(of: accountStateManager.accountState) { state in
            if case .loggedIn = state {
                onAuthenticated()
            }
        }
    }
}

public enum OnboardingSequencePolicy {
    public static func needsAccountStep(firstRun: Bool, accountState: AccountState) -> Bool {
        // Accounts are offered after local first value and never gate initial
        // Accessibility permission or local collection.
        _ = firstRun
        _ = accountState
        return false
    }
}

@MainActor
public final class OnboardingWindowController: NSObject, NSWindowDelegate {
    private enum LaunchStage {
        case manual
        case intro
        case account
        case accessibility
        case focusAllowance
        case notifications
        /// Shows the drift offer once before the user has to earn one. The
        /// offer is gated behind real evidence, so someone can use Velvt for a
        /// week without seeing the only thing it does.
        case nudgePreview
        case tourInvitation
    }

    private let presentation: PermissionPresentationModel
    private let permissionManager: any PermissionManagerProtocol
    private let accountStateManager: AccountStateManager
    private let ipcClient: any IPCClientProtocol
    private let onStartUsing: () -> Void
    private let onStartTour: () -> Void
    private var windowController: NSWindowController?
    private var flowModel: IntroFlowModel?
    private var accessibilityModel: AccessibilityPromptModel?
    private var focusAllowanceModel: FocusAllowancePromptModel?
    private var notificationModel: NotificationPromptModel?
    private var launchStage: LaunchStage = .manual
    private var isFirstRunSequence = false

    var hasPresentedWindow: Bool {
        windowController != nil
    }

    /// Which stage is on screen, observable without reaching into the stage
    /// models. The notification stage was orphaned once — `launchStage` never
    /// took `.notifications` and `presentNotificationStage` had no caller —
    /// and nothing failed, because every assertion available to a test was
    /// about whether *a* window was up, not which one.
    var presentedWindowTitle: String? {
        windowController?.window?.title
    }

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: any PermissionManagerProtocol,
        accountStateManager: AccountStateManager,
        ipcClient: any IPCClientProtocol,
        onStartUsing: @escaping () -> Void,
        onStartTour: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.onStartUsing = onStartUsing
        self.onStartTour = onStartTour
    }

    public func presentIfNeeded() {
        guard presentation.showsOnboarding else { return }
        launchStage = .manual
        presentIntro(replay: false)
    }

    /// Presents the first-run sequence, once, on the first launch that has not
    /// completed it.
    ///
    /// This previously called `replayOnboarding()` unconditionally, which
    /// forced `showsOnboarding` back to true and reintroduced the window on
    /// every single launch. An introduction that reappears after it has been
    /// read is not an introduction; it is a modal in the way of the app, and
    /// the one thing a returning user is certain not to need.
    ///
    /// Replaying deliberately is still available from Settings, which is where
    /// someone who actually wants it will look — see `presentReplay()`.
    public func presentOnLaunch() {
        guard presentation.showsOnboarding else { return }
        isFirstRunSequence = true
        launchStage = .intro
        presentIntro(replay: false)
    }

    public func presentReplay() {
        presentation.replayOnboarding()
        launchStage = .manual
        presentIntro(replay: true)
    }

    public func close() {
        launchStage = .manual
        dismissWindow()
    }

    public func windowShouldClose(_ sender: NSWindow) -> Bool {
        switch launchStage {
        case .intro:
            advanceFromIntro()
        case .account:
            guard case .loggedIn = accountStateManager.accountState else {
                NSSound.beep()
                return false
            }
            finishAccountStage()
        case .accessibility:
            guard accessibilityModel?.canContinue == true else {
                NSSound.beep()
                return false
            }
            finishAccessibilityStage()
        case .focusAllowance:
            // Closing the window is a decline: the ask is recorded and
            // nothing else changes.
            if let model = focusAllowanceModel {
                model.skip()
            } else {
                finishFocusAllowanceStage()
            }
        case .notifications:
            finishNotificationStage()
        case .nudgePreview:
            finishNudgePreviewStage()
        case .tourInvitation:
            finishWithoutTour()
        case .manual:
            presentation.completeOnboarding()
            dismissWindow()
            onStartUsing()
        }
        return true
    }

    private func presentIntro(replay: Bool) {
        if let window = windowController?.window {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let model = IntroFlowModel(
            persistCompletion: { [weak self] in
                if self?.launchStage != .intro {
                    self?.presentation.completeOnboarding()
                }
            },
            startUsing: { [weak self] in
                guard let self else { return }
                if self.launchStage == .intro {
                    self.advanceFromIntro()
                } else {
                    self.presentation.completeOnboarding()
                    self.dismissWindow()
                    self.onStartUsing()
                }
            },
            startTour: { [weak self] in
                guard let self else { return }
                if self.launchStage == .intro {
                    self.advanceFromIntro()
                } else {
                    self.presentation.completeOnboarding()
                    self.dismissWindow()
                    self.onStartTour()
                }
            }
        )
        flowModel = model
        presentWindow(
            FirstRunExperienceView(
                model: model,
                followsLaunchSequence: launchStage == .intro,
                continuesToTour: launchStage == .intro && !isFirstRunSequence
            ),
            title: replay ? "Velvt Intro" : "Welcome to Velvt"
        )
    }

    private func presentAccessibilityStage() {
        let model = AccessibilityPromptModel(
            presentation: presentation,
            permissionManager: permissionManager,
            onContinue: { [weak self] in self?.finishAccessibilityStage() }
        )
        accessibilityModel = model
        presentWindow(
            AccessibilityPermissionExperienceView(model: model, presentation: presentation),
            title: "Velvt Accessibility"
        )
    }

    private func presentAccountStage() {
        let authViewModel = AuthViewModel(
            accountStateManager: accountStateManager,
            ipcClient: ipcClient
        )
        presentWindow(
            OnboardingAccountExperienceView(
                accountStateManager: accountStateManager,
                authViewModel: authViewModel,
                onAuthenticated: { [weak self] in self?.finishAccountStage() }
            ),
            title: "Velvt Account"
        )
    }

    private func presentFocusAllowanceStage() {
        let model = FocusAllowancePromptModel(
            onContinue: { [weak self] in self?.finishFocusAllowanceStage() }
        )
        // One ask, ever: skip the stage entirely if a previous launch asked.
        guard !model.hasBeenAsked else {
            finishFocusAllowanceStage()
            return
        }
        focusAllowanceModel = model
        presentWindow(
            FocusAllowanceExperienceView(model: model),
            title: "Velvt and Focus"
        )
    }

    private func presentNotificationStage() {
        let model = NotificationPromptModel(
            presentation: presentation,
            permissionManager: permissionManager,
            onContinue: { [weak self] in self?.finishNotificationStage() }
        )
        notificationModel = model
        presentWindow(
            NotificationPermissionExperienceView(model: model, presentation: presentation),
            title: "Velvt Notifications"
        )
    }

    private func presentTourInvitationStage() {
        presentWindow(
            TourInvitationExperienceView(
                onContinue: { [weak self] in self?.finishWithoutTour() },
                onStartTour: { [weak self] in self?.finishWithTour() }
            ),
            title: "Welcome to Velvt"
        )
    }

    private func presentWindow<Content: View>(_ rootView: Content, title: String) {
        // The second of the client's two hosting roots. Selection propagates
        // from here through every onboarding stage, so permission explanations
        // and the account flow are copyable too.
        let hostingController = NSHostingController(
            rootView: rootView.textSelection(.enabled)
        )
        let window = NSWindow(contentViewController: hostingController)
        window.title = title
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        let contentSize = OnboardingWindowLayout.contentSize(
            for: NSScreen.main?.visibleFrame
        )
        window.setContentSize(contentSize)
        window.minSize = NSSize(
            width: min(OnboardingWindowLayout.minimumContentSize.width, contentSize.width),
            height: min(OnboardingWindowLayout.minimumContentSize.height, contentSize.height)
        )
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()
        let controller = NSWindowController(window: window)
        windowController = controller
        controller.showWindow(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func advanceFromIntro() {
        guard launchStage == .intro else { return }
        dismissWindow()
        guard isFirstRunSequence else {
            presentation.completeOnboarding()
            launchStage = .manual
            onStartTour()
            return
        }
        // Accounts never gate first value; `needsAccountStep` returns false so the
        // account stage stays reachable plumbing rather than an onboarding blocker.
        if OnboardingSequencePolicy.needsAccountStep(
            firstRun: isFirstRunSequence,
            accountState: accountStateManager.accountState
        ) {
            launchStage = .account
            presentAccountStage()
            return
        }
        launchStage = .accessibility
        presentAccessibilityStage()
    }

    private func finishAccountStage() {
        guard launchStage == .account else { return }
        guard case .loggedIn = accountStateManager.accountState else { return }
        dismissWindow()
        launchStage = .accessibility
        presentAccessibilityStage()
    }

    private func finishAccessibilityStage() {
        guard launchStage == .accessibility else { return }
        dismissWindow()
        // The notification stage used to sit here and was spliced out, leaving
        // `presentNotificationStage` with no caller and `.notifications` never
        // assigned to `launchStage`. The consequence was total: this is the
        // only reachable surface that calls `requestPermission(for:
        // .notifications)`, so the app could never ask, `checkStatus` answered
        // "not determined" forever, and every drift offer and insight was
        // dropped at the permission gate. A product whose one output is a
        // timely notification shipped with no way to earn the right to send
        // one. `finishNotificationStage` already routes onward to
        // `.focusAllowance`, so restoring the call restores the original chain.
        launchStage = .notifications
        presentNotificationStage()
    }

    private func finishNotificationStage() {
        guard launchStage == .notifications else { return }
        dismissWindow()
        // Only worth asking once ever. A returning user who already answered
        // continues straight to the tour.
        // Read the persisted flag directly: constructing the model here only to
        // ask whether it has been asked would require an onContinue closure
        // that can never run.
        guard !UserDefaults.standard.bool(forKey: FocusAllowancePromptModel.askedDefaultsKey)
        else {
            launchStage = .tourInvitation
            presentTourInvitationStage()
            return
        }
        launchStage = .focusAllowance
        presentFocusAllowanceStage()
    }

    private func finishFocusAllowanceStage() {
        guard launchStage == .focusAllowance else { return }
        dismissWindow()
        launchStage = .nudgePreview
        presentNudgePreviewStage()
    }

    private func presentNudgePreviewStage() {
        presentWindow(
            NudgePreviewView(onContinue: { [weak self] in self?.finishNudgePreviewStage() }),
            title: "What Velvt does"
        )
    }

    private func finishNudgePreviewStage() {
        guard launchStage == .nudgePreview else { return }
        dismissWindow()
        launchStage = .tourInvitation
        presentTourInvitationStage()
    }

    private func finishWithoutTour() {
        guard launchStage == .tourInvitation else { return }
        presentation.completeOnboarding()
        launchStage = .manual
        dismissWindow()
        onStartUsing()
    }

    private func finishWithTour() {
        guard launchStage == .tourInvitation else { return }
        presentation.completeOnboarding()
        launchStage = .manual
        dismissWindow()
        onStartTour()
    }

    private func dismissWindow() {
        windowController?.window?.delegate = nil
        windowController?.close()
        windowController = nil
        flowModel = nil
        accessibilityModel = nil
        focusAllowanceModel = nil
        notificationModel = nil
    }
}
