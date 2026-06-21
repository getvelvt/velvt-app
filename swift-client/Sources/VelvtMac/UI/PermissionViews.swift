import AppKit
import Combine
import SwiftUI

public protocol OnboardingStateStoring: AnyObject {
    var hasCompletedPermissionOnboarding: Bool { get set }
}

public final class UserDefaultsOnboardingStateStore: OnboardingStateStoring {
    private let defaults: UserDefaults
    private let key: String

    public init(
        defaults: UserDefaults = .standard,
        key: String = "hasCompletedPermissionOnboarding"
    ) {
        self.defaults = defaults
        self.key = key
    }

    public var hasCompletedPermissionOnboarding: Bool {
        get { defaults.bool(forKey: key) }
        set { defaults.set(newValue, forKey: key) }
    }
}

public final class InMemoryOnboardingStateStore: OnboardingStateStoring {
    public var hasCompletedPermissionOnboarding: Bool

    public init(hasCompletedOnboarding: Bool = false) {
        hasCompletedPermissionOnboarding = hasCompletedOnboarding
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
