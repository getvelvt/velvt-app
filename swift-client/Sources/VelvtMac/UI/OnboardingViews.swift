import Combine
import SwiftUI

// MARK: - Navigation route

public enum OnboardingRoute: Hashable {
    case permissions
    case auth
    case complete
}

// MARK: - OnboardingCoordinator

/// Drives the multi-step onboarding NavigationStack.
///
/// Steps in order: WelcomeView (root) → PermissionsStepView → AuthStepView →
/// OnboardingCompleteView. Navigation is programmatic; users cannot skip steps
/// unless via the explicit "I already have an account" affordance on the auth
/// step.
@MainActor
public final class OnboardingCoordinator: ObservableObject {
    @Published public var path: [OnboardingRoute] = []

    private let permissionManager: any PermissionManagerProtocol
    let accountStateManager: AccountStateManager
    let authViewModel: AuthViewModel
    private let onComplete: () -> Void

    public init(
        permissionManager: any PermissionManagerProtocol,
        accountStateManager: AccountStateManager,
        authViewModel: AuthViewModel,
        onComplete: @escaping () -> Void
    ) {
        self.permissionManager = permissionManager
        self.accountStateManager = accountStateManager
        self.authViewModel = authViewModel
        self.onComplete = onComplete
    }

    public func advanceFromWelcome() {
        path.append(.permissions)
    }

    public func advanceFromPermissions() {
        path.append(.auth)
    }

    public func authDidComplete() {
        path.append(.complete)
    }

    public func finishOnboarding() {
        onComplete()
    }

    /// Skips to the auth step directly when the user is already onboarded for
    /// permissions but needs to re-authenticate. The caller is responsible for
    /// ensuring permissions are already granted before using this path.
    public func skipToAuth() {
        path = [.auth]
    }

    public var permissionManager_: any PermissionManagerProtocol { permissionManager }
}

// MARK: - OnboardingFlowView

/// Root of the onboarding window. Wraps a NavigationStack driven by
/// `OnboardingCoordinator`. Lifecycle is owned by the parent `OnboardingContainer`.
public struct OnboardingFlowView: View {
    @ObservedObject private var coordinator: OnboardingCoordinator

    public init(coordinator: OnboardingCoordinator) {
        _coordinator = ObservedObject(wrappedValue: coordinator)
    }

    public var body: some View {
        NavigationStack(path: $coordinator.path) {
            WelcomeView(coordinator: coordinator)
                .navigationDestination(for: OnboardingRoute.self) { route in
                    switch route {
                    case .permissions:
                        PermissionsStepView(coordinator: coordinator)
                    case .auth:
                        AuthStepView(coordinator: coordinator)
                    case .complete:
                        OnboardingCompleteView(coordinator: coordinator)
                    }
                }
        }
        .frame(minWidth: 420, minHeight: 320)
    }
}

// MARK: - WelcomeView

public struct WelcomeView: View {
    private let coordinator: OnboardingCoordinator

    public init(coordinator: OnboardingCoordinator) {
        self.coordinator = coordinator
    }

    public var body: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "eye.slash.circle.fill")
                .font(.system(size: 56))
                .foregroundStyle(.blue)

            VStack(spacing: 8) {
                Text("Welcome to Velvt")
                    .font(.largeTitle)
                    .bold()
                Text("Privacy-first passive productivity intelligence.\nRaw activity never leaves your Mac.")
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
            }

            Button("Get Started") {
                coordinator.advanceFromWelcome()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)

            Spacer()
        }
        .padding(40)
        .navigationBarBackButtonHidden(true)
    }
}

// MARK: - PermissionsStepView

public struct PermissionsStepView: View {
    private let coordinator: OnboardingCoordinator
    @StateObject private var model: PermissionOnboardingModel

    public init(coordinator: OnboardingCoordinator) {
        self.coordinator = coordinator
        _model = StateObject(wrappedValue: PermissionOnboardingModel(
            permissionManager: coordinator.permissionManager_,
            onCompletion: {
                // Advancing is handled via coordinator in the button action below
                // so we don't call it here; this closure satisfies the existing
                // PermissionOnboardingModel API when used standalone.
            }
        ))
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Allow access")
                .font(.title2)
                .bold()

            Text(explanation)
                .foregroundStyle(.secondary)

            Button(buttonTitle) {
                Task {
                    await requestAndAdvanceIfDone()
                }
            }
            .disabled(model.isRequesting)
            .buttonStyle(.borderedProminent)
        }
        .padding(32)
        .frame(maxWidth: .infinity, alignment: .leading)
        .navigationBarBackButtonHidden(true)
    }

    private var explanation: String {
        switch model.step {
        case .accessibility:
            return "Accessibility lets Velvt detect which app is focused — nothing more."
        case .notifications:
            return "Notifications deliver your daily productivity insights."
        }
    }

    private var buttonTitle: String {
        switch model.step {
        case .accessibility: return "Allow Accessibility"
        case .notifications: return "Allow Notifications"
        }
    }

    private func requestAndAdvanceIfDone() async {
        await model.requestCurrentPermission()
        if model.step == .notifications {
            // Both permissions have been requested; advance regardless of grant
            // status (users can deny and still use the app with degraded features).
            coordinator.advanceFromPermissions()
        }
    }
}

// MARK: - AuthStepView

public struct AuthStepView: View {
    private let coordinator: OnboardingCoordinator
    @ObservedObject private var authViewModel: AuthViewModel
    @ObservedObject private var accountStateManager: AccountStateManager

    public init(coordinator: OnboardingCoordinator) {
        self.coordinator = coordinator
        self.authViewModel = coordinator.authViewModel
        self.accountStateManager = coordinator.accountStateManager
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text(authViewModel.authMode == .signUp ? "Create your account" : "Welcome back")
                .font(.title2)
                .bold()

            VStack(alignment: .leading, spacing: 12) {
                TextField("Email", text: $authViewModel.email)
                    .textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled()

                SecureField("Password", text: $authViewModel.password)
                    .textFieldStyle(.roundedBorder)
            }

            if let error = authViewModel.errorMessage {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            VStack(spacing: 8) {
                Button(authViewModel.authMode == .signUp ? "Create Account" : "Log In") {
                    Task {
                        if authViewModel.authMode == .signUp {
                            await authViewModel.signUp()
                        } else {
                            await authViewModel.logIn()
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(authViewModel.isLoading || authViewModel.email.isEmpty || authViewModel.password.isEmpty)
                .frame(maxWidth: .infinity)

                Button(authViewModel.authMode == .signUp ? "I already have an account" : "Create a new account") {
                    authViewModel.toggleAuthMode()
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .font(.callout)
            }

            if authViewModel.isLoading {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, alignment: .leading)
        .navigationBarBackButtonHidden(true)
        .onChange(of: accountStateManager.accountState) { newState in
            if case .loggedIn = newState {
                coordinator.authDidComplete()
            }
        }
    }
}

// MARK: - OnboardingCompleteView

public struct OnboardingCompleteView: View {
    private let coordinator: OnboardingCoordinator

    public init(coordinator: OnboardingCoordinator) {
        self.coordinator = coordinator
    }

    public var body: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 56))
                .foregroundStyle(.green)

            VStack(spacing: 8) {
                Text("You're all set")
                    .font(.largeTitle)
                    .bold()
                Text("Velvt is now running in your menu bar.")
                    .foregroundStyle(.secondary)
            }

            Button("Done") {
                coordinator.finishOnboarding()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)

            Spacer()
        }
        .padding(40)
        .navigationBarBackButtonHidden(true)
    }
}

// MARK: - DeviceRevokedView

/// Shown when the Rust service pushes a `device_revoked` message. Non-dismissible
/// until the user explicitly chooses to sign in again.
public struct DeviceRevokedView: View {
    private let onSignInAgain: () -> Void

    public init(onSignInAgain: @escaping () -> Void) {
        self.onSignInAgain = onSignInAgain
    }

    public var body: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "lock.slash.fill")
                .font(.system(size: 56))
                .foregroundStyle(.orange)

            VStack(spacing: 12) {
                Text("Device access revoked")
                    .font(.title2)
                    .bold()

                Text(
                    "This device's access was revoked, either from another device or by your account owner.\n\nYour local data remains private and on-device."
                )
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
            }

            Button("Sign In Again") {
                onSignInAgain()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)

            Spacer()
        }
        .padding(40)
        .frame(minWidth: 380, minHeight: 300)
    }
}

// MARK: - PendingDeletionView

/// Shown when the app relaunches while an account-deletion request is still
/// in flight. Blocks all normal use until the Rust service acknowledges the
/// erasure or the request is explicitly cancelled.
public struct PendingDeletionView: View {
    private let onCancel: () -> Void

    public init(onCancel: @escaping () -> Void) {
        self.onCancel = onCancel
    }

    public var body: some View {
        VStack(spacing: 20) {
            Spacer()
            ProgressView()
                .scaleEffect(1.5)

            VStack(spacing: 8) {
                Text("Account deletion in progress")
                    .font(.title2)
                    .bold()
                Text("Your account data is being erased.\nThis may take a moment.")
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)
            }

            Button("Cancel Deletion") {
                onCancel()
            }
            .buttonStyle(.bordered)
            .foregroundStyle(.red)

            Spacer()
        }
        .padding(40)
        .frame(minWidth: 360, minHeight: 260)
    }
}

// MARK: - NeedsReauthView

/// Shown when the account is in `.loggedOut` after a session expired.
/// Provides a shortcut to the login screen without repeating onboarding.
public struct NeedsReauthView: View {
    private let onSignIn: () -> Void

    public init(onSignIn: @escaping () -> Void) {
        self.onSignIn = onSignIn
    }

    public var body: some View {
        VStack(spacing: 20) {
            Spacer()
            Image(systemName: "person.crop.circle.badge.exclamationmark")
                .font(.system(size: 48))
                .foregroundStyle(.yellow)

            VStack(spacing: 8) {
                Text("Session expired")
                    .font(.title2)
                    .bold()
                Text("Please sign in to continue.")
                    .foregroundStyle(.secondary)
            }

            Button("Sign In") { onSignIn() }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)

            Spacer()
        }
        .padding(40)
        .frame(minWidth: 360, minHeight: 260)
    }
}
