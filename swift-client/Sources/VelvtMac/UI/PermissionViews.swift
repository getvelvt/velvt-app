import AppKit
import Combine
import SwiftUI

public protocol OnboardingStateStoring: AnyObject {
    var hasCompletedPermissionOnboarding: Bool { get set }
    var hasSeenValueProposition: Bool { get set }
    var hasRequestedAccessibilityPermission: Bool { get set }
    var hasRequestedNotificationsPermission: Bool { get set }
    var attentionIntensity: AttentionIntensity { get set }
    var attentionPurpose: AttentionPurpose { get set }
}

public enum AttentionIntensity: String, CaseIterable, Identifiable {
    case light
    case medium
    case intense

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .light: "Light"
        case .medium: "Medium"
        case .intense: "Intense"
        }
    }
}

public enum AttentionPurpose: String, CaseIterable, Identifiable {
    case deepWork = "deep_work"
    case study
    case healthyTechUse = "healthy_tech_use"
    case creativePractice = "creative_practice"
    case workLifeBoundary = "work_life_boundary"

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .deepWork: "Deep work"
        case .study: "Study"
        case .healthyTechUse: "Healthy tech use"
        case .creativePractice: "Creative practice"
        case .workLifeBoundary: "Work-life boundary"
        }
    }
}

public final class UserDefaultsOnboardingStateStore: OnboardingStateStoring {
    public static let currentIntroVersion = 1

    private let defaults: UserDefaults
    private let key: String
    private let completionVersionKey: String
    private let intensityKey: String
    private let purposeKey: String
    private let valueKey: String
    private let accessibilityRequestKey: String
    private let notificationsRequestKey: String

    public init(
        defaults: UserDefaults = .standard,
        key: String = "hasCompletedPermissionOnboarding",
        completionVersionKey: String = "velvt.onboarding.completed_version",
        intensityKey: String = "attentionIntensity",
        purposeKey: String = "attentionPurpose",
        valueKey: String = "hasSeenValueProposition",
        accessibilityRequestKey: String = "hasRequestedAccessibilityPermission",
        notificationsRequestKey: String = "hasRequestedNotificationsPermission"
    ) {
        self.defaults = defaults
        self.key = key
        self.completionVersionKey = completionVersionKey
        self.intensityKey = intensityKey
        self.purposeKey = purposeKey
        self.valueKey = valueKey
        self.accessibilityRequestKey = accessibilityRequestKey
        self.notificationsRequestKey = notificationsRequestKey

        let establishedInstallationKeys = [
            valueKey,
            accessibilityRequestKey,
            notificationsRequestKey,
            intensityKey,
            purposeKey,
            "velvt.collection.offline_events_enabled",
            "velvt.metrics.actions_logged",
            "velvt.metrics.interventions",
        ]
        if defaults.object(forKey: key) == nil,
            defaults.object(forKey: completionVersionKey) == nil,
            establishedInstallationKeys.contains(where: { defaults.object(forKey: $0) != nil })
        {
            defaults.set(Self.currentIntroVersion, forKey: completionVersionKey)
        }
    }

    public var hasCompletedPermissionOnboarding: Bool {
        get {
            defaults.bool(forKey: key)
                || defaults.integer(forKey: completionVersionKey) >= Self.currentIntroVersion
        }
        set {
            defaults.set(newValue, forKey: key)
            if newValue {
                defaults.set(Self.currentIntroVersion, forKey: completionVersionKey)
            } else {
                defaults.removeObject(forKey: completionVersionKey)
            }
        }
    }

    public var hasSeenValueProposition: Bool {
        get { defaults.bool(forKey: valueKey) }
        set { defaults.set(newValue, forKey: valueKey) }
    }

    public var hasRequestedAccessibilityPermission: Bool {
        get { defaults.bool(forKey: accessibilityRequestKey) }
        set { defaults.set(newValue, forKey: accessibilityRequestKey) }
    }

    public var hasRequestedNotificationsPermission: Bool {
        get { defaults.bool(forKey: notificationsRequestKey) }
        set { defaults.set(newValue, forKey: notificationsRequestKey) }
    }

    public var attentionIntensity: AttentionIntensity {
        get {
            AttentionIntensity(rawValue: defaults.string(forKey: intensityKey) ?? "")
                ?? .medium
        }
        set { defaults.set(newValue.rawValue, forKey: intensityKey) }
    }

    public var attentionPurpose: AttentionPurpose {
        get {
            AttentionPurpose(rawValue: defaults.string(forKey: purposeKey) ?? "")
                ?? .deepWork
        }
        set { defaults.set(newValue.rawValue, forKey: purposeKey) }
    }
}

public final class InMemoryOnboardingStateStore: OnboardingStateStoring {
    public var hasCompletedPermissionOnboarding: Bool
    public var hasSeenValueProposition: Bool
    public var hasRequestedAccessibilityPermission: Bool
    public var hasRequestedNotificationsPermission: Bool
    public var attentionIntensity: AttentionIntensity
    public var attentionPurpose: AttentionPurpose

    public init(
        hasCompletedOnboarding: Bool = false,
        attentionIntensity: AttentionIntensity = .medium,
        attentionPurpose: AttentionPurpose = .deepWork,
        hasSeenValueProposition: Bool = false,
        hasRequestedAccessibilityPermission: Bool = false,
        hasRequestedNotificationsPermission: Bool = false
    ) {
        hasCompletedPermissionOnboarding = hasCompletedOnboarding
        self.attentionIntensity = attentionIntensity
        self.attentionPurpose = attentionPurpose
        self.hasSeenValueProposition = hasSeenValueProposition
        self.hasRequestedAccessibilityPermission = hasRequestedAccessibilityPermission
        self.hasRequestedNotificationsPermission = hasRequestedNotificationsPermission
    }
}

public final class PermissionPresentationModel: ObservableObject {
    @Published public private(set) var showsOnboarding: Bool
    @Published public private(set) var statuses: [PermissionType: PermissionStatus] = [
        .accessibility: .unknown,
        .notifications: .unknown
    ]

    public var showsAccessibilityRecovery: Bool {
        switch statuses[.accessibility] ?? .unknown {
        case .denied, .restricted:
            return true
        case .unknown, .granted:
            return false
        }
    }

    public var hasSeenValueProposition: Bool {
        onboardingStateStore.hasSeenValueProposition
    }

    public var hasRequestedAccessibilityPermission: Bool {
        onboardingStateStore.hasRequestedAccessibilityPermission
    }

    public var hasRequestedNotificationsPermission: Bool {
        onboardingStateStore.hasRequestedNotificationsPermission
    }

    private let onboardingStateStore: any OnboardingStateStoring
    private var cancellable: AnyCancellable?

    public init(
        permissionManager: any PermissionManagerProtocol,
        onboardingStateStore: any OnboardingStateStoring
    ) {
        self.onboardingStateStore = onboardingStateStore
        showsOnboarding = !onboardingStateStore.hasCompletedPermissionOnboarding
        cancellable = permissionManager.statusPublisher.sink { [weak self] statuses in
            if Thread.isMainThread {
                self?.statuses = statuses
            } else {
                DispatchQueue.main.async {
                    self?.statuses = statuses
                }
            }
        }
    }

    public func completeOnboarding() {
        onboardingStateStore.hasCompletedPermissionOnboarding = true
        showsOnboarding = false
    }

    public func replayOnboarding() {
        showsOnboarding = true
    }

    public func acknowledgeValueProposition() {
        onboardingStateStore.hasSeenValueProposition = true
        objectWillChange.send()
    }

    public func markPermissionRequested(_ permission: PermissionType) {
        switch permission {
        case .accessibility:
            onboardingStateStore.hasRequestedAccessibilityPermission = true
        case .notifications:
            onboardingStateStore.hasRequestedNotificationsPermission = true
        }
        objectWillChange.send()
    }

    public func saveGoal(intensity: AttentionIntensity, purpose: AttentionPurpose) {
        onboardingStateStore.attentionIntensity = intensity
        onboardingStateStore.attentionPurpose = purpose
        completeOnboarding()
    }
}

@MainActor
public final class PermissionOnboardingModel: ObservableObject {
    public enum Step: Equatable {
        case accessibility
        case notifications
    }

    @Published public private(set) var step: Step = .accessibility
    @Published public private(set) var isRequesting = false
    @Published public private(set) var isComplete = false

    private let permissionManager: any PermissionManagerProtocol
    private let onCompletion: () -> Void

    public init(
        permissionManager: any PermissionManagerProtocol,
        onCompletion: @escaping () -> Void
    ) {
        self.permissionManager = permissionManager
        self.onCompletion = onCompletion
    }

    public func requestCurrentPermission() async {
        guard !isRequesting else {
            return
        }
        isRequesting = true
        switch step {
        case .accessibility:
            _ = await permissionManager.requestPermission(for: .accessibility)
            step = .notifications
        case .notifications:
            _ = await permissionManager.requestPermission(for: .notifications)
            isComplete = true
            onCompletion()
        }
        isRequesting = false
    }
}

public struct PermissionRecoveryView: View {
    private let openSettings: () -> Void

    public init(openSettings: @escaping () -> Void = Self.openAccessibilitySettings) {
        self.openSettings = openSettings
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Accessibility permission required", systemImage: "exclamationmark.triangle")
                .font(.headline)
            Text("Collection is paused. Re-grant Accessibility access in System Settings.")
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("Open Accessibility Settings", action: openSettings)
        }
    }

    public static func openAccessibilitySettings() {
        guard let url = URL(
            string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        ) else {
            return
        }
        NSWorkspace.shared.open(url)
    }
}

public struct GoalOnboardingView: View {
    @State private var intensity: AttentionIntensity = .medium
    @State private var purpose: AttentionPurpose = .deepWork
    private let save: (AttentionIntensity, AttentionPurpose) -> Void

    public init(save: @escaping (AttentionIntensity, AttentionPurpose) -> Void) {
        self.save = save
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Set your Velvt mode")
                .font(.headline)
            Picker("Intensity", selection: $intensity) {
                ForEach(AttentionIntensity.allCases) { option in
                    Text(option.label).tag(option)
                }
            }
            Picker("Purpose", selection: $purpose) {
                ForEach(AttentionPurpose.allCases) { option in
                    Text(option.label).tag(option)
                }
            }
            Button("Continue") {
                save(intensity, purpose)
            }
            .buttonStyle(.borderedProminent)
        }
        .pickerStyle(.menu)
        .tint(Color.velvtPink)
    }
}

public enum FirstRunOnboardingState: Equatable, Sendable {
    case valueProposition
    case accessibilityExplanation
    case accessibilityDenied
    case notificationsExplanation
    case authenticationRequired
    case serviceStarting
    case serviceUnavailable
    case collectionStarting
    case collectionActive(BaselineProgress)

    public static func resolve(
        hasSeenValueProposition: Bool,
        accessibilityStatus: PermissionStatus,
        hasRequestedAccessibility: Bool,
        notificationsStatus: PermissionStatus,
        hasRequestedNotifications: Bool,
        isAuthenticated: Bool,
        servicePhase: LocalServiceConnectionPhase,
        collectionIsRunning: Bool,
        baselineProgress: BaselineProgress
    ) -> FirstRunOnboardingState {
        guard hasSeenValueProposition else { return .valueProposition }
        guard accessibilityStatus == .granted else {
            return hasRequestedAccessibility ? .accessibilityDenied : .accessibilityExplanation
        }
        // Notifications and an account are optional until after local first
        // value. Keep these inputs for source compatibility with existing UI
        // callers while resolving first run solely from local readiness.
        _ = notificationsStatus
        _ = hasRequestedNotifications
        _ = isAuthenticated
        switch servicePhase {
        case .starting, .waking:
            return .serviceStarting
        case .unavailable:
            return .serviceUnavailable
        case .connected:
            break
        }
        guard collectionIsRunning else { return .collectionStarting }
        return .collectionActive(baselineProgress)
    }
}

public struct FirstRunOnboardingView: View {
    @ObservedObject private var presentation: PermissionPresentationModel
    private let permissionManager: (any PermissionManagerProtocol)?
    private let servicePhase: LocalServiceConnectionPhase
    private let collectionIsRunning: Bool
    private let isAuthenticated: Bool
    private let baselineProgress: BaselineProgress

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)?,
        servicePhase: LocalServiceConnectionPhase,
        collectionIsRunning: Bool,
        isAuthenticated: Bool,
        baselineProgress: BaselineProgress
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.servicePhase = servicePhase
        self.collectionIsRunning = collectionIsRunning
        self.isAuthenticated = isAuthenticated
        self.baselineProgress = baselineProgress
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            content
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.velvtPanel, in: RoundedRectangle(cornerRadius: 10))
        .tint(Color.velvtPink)
        .accessibilityElement(children: .contain)
    }

    private var state: FirstRunOnboardingState {
        FirstRunOnboardingState.resolve(
            hasSeenValueProposition: presentation.hasSeenValueProposition,
            accessibilityStatus: presentation.statuses[.accessibility] ?? .unknown,
            hasRequestedAccessibility: presentation.hasRequestedAccessibilityPermission,
            notificationsStatus: presentation.statuses[.notifications] ?? .unknown,
            hasRequestedNotifications: presentation.hasRequestedNotificationsPermission,
            isAuthenticated: isAuthenticated,
            servicePhase: servicePhase,
            collectionIsRunning: collectionIsRunning,
            baselineProgress: baselineProgress
        )
    }

    @ViewBuilder
    private var content: some View {
        switch state {
        case .valueProposition:
            Text("See when work became fragmented — and what to protect next")
                .font(.title3.bold())
            Text("Velvt shows evidence of when your work became fragmented and one realistic way to protect your next focus block.")
            .font(.body)
            .fixedSize(horizontal: false, vertical: true)
            Button("Set up Velvt") { presentation.acknowledgeValueProposition() }
                .buttonStyle(.borderedProminent)

        case .accessibilityExplanation:
            Label("Allow local activity collection", systemImage: "hand.raised")
                .font(.headline)
            Text("Accessibility lets Velvt notice broad work changes on this Mac. Raw app names, window titles, URLs, and local labels never leave your device.")
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            Button("Continue to System Settings") {
                presentation.markPermissionRequested(.accessibility)
                Task { _ = await permissionManager?.requestPermission(for: .accessibility) }
            }
            .buttonStyle(.borderedProminent)

        case .accessibilityDenied:
            PermissionRecoveryView()

        case .notificationsExplanation:
            Label("Choose whether Velvt can notify you", systemImage: "bell")
                .font(.headline)
            Text("Notifications can surface a concise, evidence-grounded observation. Saying no will not block collection.")
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            HStack {
                Button("Not now") { presentation.markPermissionRequested(.notifications) }
                Button("Allow Notifications") {
                    presentation.markPermissionRequested(.notifications)
                    Task { _ = await permissionManager?.requestPermission(for: .notifications) }
                }
                .buttonStyle(.borderedProminent)
            }

        case .authenticationRequired:
            Label("Cloud features are optional", systemImage: "person.crop.circle")
                .font(.headline)
            Text("Local collection works without an account. Sign in later if you want synchronized history and cloud insight delivery.")
            .font(.caption)
            .foregroundStyle(.secondary)

        case .serviceStarting:
            ProgressView("Starting local service…")
                .controlSize(.small)
            Text("This normally takes only a moment.")
                .font(.caption)
                .foregroundStyle(.secondary)

        case .serviceUnavailable:
            Label("Local service unavailable", systemImage: "exclamationmark.triangle")
                .font(.headline)
            Text("Quit and reopen Velvt to restart the local service. Your existing local data is preserved.")
            .font(.caption)
            .foregroundStyle(.secondary)

        case .collectionStarting:
            ProgressView("Starting local collection…")
                .controlSize(.small)

        case .collectionActive(let progress):
            Label("Local collection has started", systemImage: "checkmark.circle.fill")
                .font(.headline)
                .foregroundStyle(Color.velvtGreen)
            Text(progress.label)
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("Open Today") { presentation.completeOnboarding() }
                .buttonStyle(.borderedProminent)
        }
    }
}
