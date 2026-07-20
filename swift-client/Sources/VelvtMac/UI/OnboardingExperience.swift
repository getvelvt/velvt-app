import AppKit
import SwiftUI

public enum OnboardingCopy {
    public static let privacySummary =
        "Raw app names, window titles, URLs, filenames, paths, contacts, and work-block intentions stay on this Mac. Approved broad categories, coarse durations, timestamps, and safe summaries may synchronize for beta insights."
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

public enum GuidedTourStep: Int, CaseIterable, Equatable, Sendable {
    case today
    case earlySignal
    case yourWeek
    case activity
    case statusAndRecovery
    case settings

    public var title: String {
        switch self {
        case .today: "Today and work blocks"
        case .earlySignal: "Early local signal"
        case .yourWeek: "Your Week"
        case .activity: "Activity"
        case .statusAndRecovery: "Status and recovery"
        case .settings: "Settings"
        }
    }

    public var detail: String {
        switch self {
        case .today:
            "Start or end a meaningful work block here. Its optional intention remains local."
        case .earlySignal:
            "Today adds an early local signal as evidence arrives, including its observation window and freshness."
        case .yourWeek:
            "Review recent privacy-safe summaries without scores, streaks, or moral judgment."
        case .activity:
            "See broad context switching and uninterrupted stretches without exposing raw activity."
        case .statusAndRecovery:
            "The header and inline controls show scoped collection, sync, permission, and recovery status."
        case .settings:
            "Manage local collection and your account, or replay this intro and tour at any time."
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

public struct FirstRunExperienceView: View {
    @ObservedObject private var model: IntroFlowModel
    @ObservedObject private var presentation: PermissionPresentationModel
    private let permissionManager: any PermissionManagerProtocol
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        model: IntroFlowModel,
        presentation: PermissionPresentationModel,
        permissionManager: any PermissionManagerProtocol
    ) {
        self.model = model
        self.presentation = presentation
        self.permissionManager = permissionManager
    }

    public var body: some View {
        VStack(spacing: 0) {
            header
            Divider().opacity(0.2)
            ScrollView {
                page
                    .frame(maxWidth: 620, alignment: .leading)
                    .padding(36)
            }
            Divider().opacity(0.2)
            footer
        }
        .frame(
            minWidth: OnboardingWindowLayout.minimumContentSize.width,
            minHeight: OnboardingWindowLayout.minimumContentSize.height
        )
        .background(Color.velvtSurface)
        .foregroundStyle(Color.velvtText)
        .preferredColorScheme(.dark)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.18), value: model.step)
        .onExitCommand {
            if model.step == .quickStart {
                model.finishAndStartUsing()
            } else {
                model.skipIntro()
            }
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image("VelvtMenuBarIcon")
                .resizable()
                .renderingMode(.template)
                .interpolation(.high)
                .frame(width: 30, height: 30)
                .foregroundStyle(Color.velvtText)
                .accessibilityHidden(true)
            Text("Velvt")
                .font(.headline)
            Spacer()
            if model.step != .quickStart {
                Text("Step \(model.step.rawValue + 1) of 4")
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                    .accessibilityLabel("Intro step \(model.step.rawValue + 1) of 4")
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
                title: "Protect meaningful work from digital drift.",
                body:
                    "Velvt is a private focus coach for the Mac. It helps you notice broad patterns, protect a useful stretch of work, and recover without judgment."
            )
        case .privacy:
            VStack(alignment: .leading, spacing: 18) {
                IntroPage(
                    systemImage: "lock.shield",
                    title: "Private by design, clear about synchronization.",
                    body:
                        "Accessibility lets Velvt notice when the active work context changes so it can build broad, privacy-safe patterns."
                )
                Text(OnboardingCopy.privacySummary)
                    .font(.body)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button(accessibilityActionLabel) {
                        presentation.markPermissionRequested(.accessibility)
                        requestAccessibility()
                    }
                    .buttonStyle(.bordered)
                    .disabled(presentation.statuses[.accessibility] == .granted)
                    .accessibilityHint("Opens the macOS Accessibility permission flow")
                    Button(notificationsActionLabel) {
                        presentation.markPermissionRequested(.notifications)
                        Task { _ = await permissionManager.requestPermission(for: .notifications) }
                    }
                    .buttonStyle(.bordered)
                    .disabled(presentation.statuses[.notifications] == .granted)
                    .accessibilityHint("Requests optional notification permission")
                }
            }
        case .capabilities:
            VStack(alignment: .leading, spacing: 18) {
                IntroPage(
                    systemImage: "sparkles",
                    title: "A modest loop for meaningful work.",
                    body: "Velvt supports the work you already do; it does not grade it."
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
                capability(
                    "calendar",
                    "Review recent Activity and Your Week without scores or moral judgment.")
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
            HStack(spacing: 12) {
                secondaryActions
                Spacer()
                primaryActions
            }
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 12) {
                    secondaryActions
                }
                HStack(spacing: 12) {
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
                .buttonStyle(.plain)
        } else {
            Button("Skip intro") { model.skipIntro() }
                .buttonStyle(.plain)
            if model.canGoBack {
                Button("Back") { model.goBack() }
            }
        }
    }

    @ViewBuilder private var primaryActions: some View {
        switch model.step {
        case .ready:
            Button("Skip tour and start using") { model.finishAndStartUsing() }
                .buttonStyle(.plain)
            Button("Start guided tour") { model.finishAndStartTour() }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
        case .quickStart:
            Button("Start using Velvt") { model.finishAndStartUsing() }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
        default:
            Button("Continue") { model.continueForward() }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
        }
    }

    private var accessibilityActionLabel: String {
        presentation.statuses[.accessibility] == .granted
            ? "Accessibility Allowed"
            : "Allow Accessibility"
    }

    private var notificationsActionLabel: String {
        presentation.statuses[.notifications] == .granted
            ? "Notifications Allowed"
            : "Allow Notifications"
    }

    private func requestAccessibility() {
        switch presentation.statuses[.accessibility] ?? .unknown {
        case .denied, .restricted:
            PermissionRecoveryView.openAccessibilitySettings()
        case .unknown, .granted:
            Task { _ = await permissionManager.requestPermission(for: .accessibility) }
        }
    }

    private func capability(_ systemImage: String, _ text: String) -> some View {
        Label {
            Text(text).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: systemImage)
                .foregroundStyle(Color.velvtPink)
                .frame(width: 24)
        }
        .font(.body)
    }

    private func numberedPoint(_ number: Int, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 14) {
            Text("\(number)")
                .font(.headline.monospacedDigit())
                .foregroundStyle(Color.velvtPink)
            Text(text)
                .font(.body)
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
                .font(.system(size: 36, weight: .medium))
                .foregroundStyle(Color.velvtPink)
                .accessibilityHidden(true)
            Text(title)
                .font(.largeTitle.bold())
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            Text(detail)
                .font(.title3)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

@MainActor
public final class OnboardingWindowController: NSObject, NSWindowDelegate {
    private let presentation: PermissionPresentationModel
    private let permissionManager: any PermissionManagerProtocol
    private let onStartUsing: () -> Void
    private let onStartTour: () -> Void
    private var windowController: NSWindowController?
    private var flowModel: IntroFlowModel?

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: any PermissionManagerProtocol,
        onStartUsing: @escaping () -> Void,
        onStartTour: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.onStartUsing = onStartUsing
        self.onStartTour = onStartTour
    }

    public func presentIfNeeded() {
        guard presentation.showsOnboarding else { return }
        present(replay: false)
    }

    public func presentReplay() {
        presentation.replayOnboarding()
        present(replay: true)
    }

    public func close() {
        windowController?.window?.delegate = nil
        windowController?.close()
        windowController = nil
        flowModel = nil
    }

    public func windowShouldClose(_ sender: NSWindow) -> Bool {
        presentation.completeOnboarding()
        onStartUsing()
        windowController = nil
        flowModel = nil
        return true
    }

    private func present(replay: Bool) {
        if let window = windowController?.window {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let model = IntroFlowModel(
            persistCompletion: { [weak presentation] in presentation?.completeOnboarding() },
            startUsing: { [weak self] in
                self?.close()
                self?.onStartUsing()
            },
            startTour: { [weak self] in
                self?.close()
                self?.onStartTour()
            }
        )
        flowModel = model
        let rootView = FirstRunExperienceView(
            model: model,
            presentation: presentation,
            permissionManager: permissionManager
        )
        let hostingController = NSHostingController(rootView: rootView)
        let window = NSWindow(contentViewController: hostingController)
        window.title = replay ? "Velvt Intro" : "Welcome to Velvt"
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
}
