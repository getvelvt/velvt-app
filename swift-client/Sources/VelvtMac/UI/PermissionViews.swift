import AppKit
import Combine
import SwiftUI

public protocol OnboardingStateStoring: AnyObject {
    var hasCompletedPermissionOnboarding: Bool { get set }
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
    private let defaults: UserDefaults
    private let key: String
    private let intensityKey: String
    private let purposeKey: String

    public init(
        defaults: UserDefaults = .standard,
        key: String = "hasCompletedPermissionOnboarding",
        intensityKey: String = "attentionIntensity",
        purposeKey: String = "attentionPurpose"
    ) {
        self.defaults = defaults
        self.key = key
        self.intensityKey = intensityKey
        self.purposeKey = purposeKey
    }

    public var hasCompletedPermissionOnboarding: Bool {
        get { defaults.bool(forKey: key) }
        set { defaults.set(newValue, forKey: key) }
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
    public var attentionIntensity: AttentionIntensity
    public var attentionPurpose: AttentionPurpose

    public init(
        hasCompletedOnboarding: Bool = false,
        attentionIntensity: AttentionIntensity = .medium,
        attentionPurpose: AttentionPurpose = .deepWork
    ) {
        hasCompletedPermissionOnboarding = hasCompletedOnboarding
        self.attentionIntensity = attentionIntensity
        self.attentionPurpose = attentionPurpose
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
    }
}
