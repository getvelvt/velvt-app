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
            onCompletion()
        }
        isRequesting = false
    }
}

public struct PermissionRootView: View {
    @ObservedObject private var presentation: PermissionPresentationModel
    @ObservedObject private var accountStateManager: AccountStateManager
    private let permissionManager: any PermissionManagerProtocol
    private let ipcClient: any IPCClientProtocol

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: any PermissionManagerProtocol,
        accountStateManager: AccountStateManager,
        ipcClient: any IPCClientProtocol
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
    }

    public var body: some View {
        if accountStateManager.isDeviceRevoked {
            DeviceRevokedView {
                accountStateManager.clearDeviceRevokedFlag()
            }
        } else if case .pendingErasure = accountStateManager.accountState {
            PendingDeletionView {
                accountStateManager.cancelPendingErasure()
            }
        } else if presentation.showsOnboarding || !isLoggedIn {
            // skipToAuth is true when permissions are done but the user is
            // logged out (re-auth after session expiry).
            OnboardingContainer(
                permissionManager: permissionManager,
                accountStateManager: accountStateManager,
                ipcClient: ipcClient,
                skipToAuth: !presentation.showsOnboarding && !isLoggedIn,
                onComplete: { presentation.completeOnboarding() }
            )
        } else {
            Text("Velvt is available from the menu bar.")
                .foregroundStyle(.secondary)
                .frame(minWidth: 360, minHeight: 240)
        }
    }

    private var isLoggedIn: Bool {
        if case .loggedIn = accountStateManager.accountState { return true }
        return false
    }
}

/// Creates and owns the `OnboardingCoordinator` and `AuthViewModel` for the
/// duration of the onboarding session. Using `@StateObject` ensures they are
/// not recreated on re-renders.
private struct OnboardingContainer: View {
    @StateObject private var coordinator: OnboardingCoordinator

    init(
        permissionManager: any PermissionManagerProtocol,
        accountStateManager: AccountStateManager,
        ipcClient: any IPCClientProtocol,
        skipToAuth: Bool,
        onComplete: @escaping () -> Void
    ) {
        let authVM = AuthViewModel(
            accountStateManager: accountStateManager,
            ipcClient: ipcClient
        )
        let coord = OnboardingCoordinator(
            permissionManager: permissionManager,
            accountStateManager: accountStateManager,
            authViewModel: authVM,
            onComplete: onComplete
        )
        if skipToAuth { coord.skipToAuth() }
        _coordinator = StateObject(wrappedValue: coord)
    }

    var body: some View {
        OnboardingFlowView(coordinator: coordinator)
    }
}

public struct OnboardingView: View {
    @StateObject private var model: PermissionOnboardingModel

    public init(
        permissionManager: any PermissionManagerProtocol,
        onCompletion: @escaping () -> Void
    ) {
        _model = StateObject(
            wrappedValue: PermissionOnboardingModel(
                permissionManager: permissionManager,
                onCompletion: onCompletion
            )
        )
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Set up Velvt")
                .font(.title2)
                .bold()
            Text(explanation)
                .foregroundStyle(.secondary)
            Button(buttonTitle) {
                Task {
                    await model.requestCurrentPermission()
                }
            }
            .disabled(model.isRequesting)
        }
        .padding(24)
    }

    private var explanation: String {
        switch model.step {
        case .accessibility:
            return "Accessibility is used to detect which app is focused."
        case .notifications:
            return "Notifications are used to deliver daily insights."
        }
    }

    private var buttonTitle: String {
        switch model.step {
        case .accessibility:
            return "Continue to Accessibility"
        case .notifications:
            return "Continue to Notifications"
        }
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
