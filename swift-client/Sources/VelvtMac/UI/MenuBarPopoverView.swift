import Combine
import SwiftUI

public enum MenuBarAccountAction: Equatable {
    case authenticate(AuthViewModel.AuthMode)
    case logOut
}

public enum MenuBarAccountActionResolver {
    public static func actions(for accountState: AccountState) -> [MenuBarAccountAction] {
        switch accountState {
        case .loggedOut:
            return [.authenticate(.logIn), .authenticate(.signUp)]
        case .loggedIn:
            return [.logOut]
        case .loggingIn, .loggingOut, .pendingErasure:
            return []
        }
    }
}

@MainActor
public final class ServiceConnectionStatusModel: ObservableObject {
    @Published public private(set) var status: ConnectionStatus = .disconnected

    private var cancellable: AnyCancellable?

    public init(connectionStatus: AnyPublisher<ConnectionStatus, Never>) {
        cancellable = connectionStatus
            .receive(on: RunLoop.main)
            .sink { [weak self] in
                self?.status = $0
            }
    }
}

struct ServiceConnectionStatusLabel: View {
    let status: ConnectionStatus

    var body: some View {
        Label(text, systemImage: symbolName)
            .font(.caption2)
            .foregroundStyle(color)
    }

    private var text: String {
        switch status {
        case .connected:
            return "Service connected"
        case .connecting, .handshaking:
            return "Starting service"
        case .reconnecting:
            return "Reconnecting service"
        case .disconnected:
            return "Service unavailable"
        }
    }

    private var symbolName: String {
        switch status {
        case .connected:
            return "checkmark.circle.fill"
        case .connecting, .handshaking, .reconnecting:
            return "arrow.triangle.2.circlepath"
        case .disconnected:
            return "wifi.slash"
        }
    }

    private var color: Color {
        switch status {
        case .connected:
            return .green
        case .connecting, .handshaking, .reconnecting:
            return .orange
        case .disconnected:
            return .secondary
        }
    }
}

// MARK: - MenuBarPopoverView

/// Root content hosted by the `NSPopover` owned by `MenuBarController`.
///
/// Embeds the S6 display content (`InsightCardView` and `HistoryListView`,
/// via `VelvtPopoverContentView`) and the accessibility/notification status
/// banners previously shown in the menu bar window. Tab order follows
/// declaration order in `VelvtPopoverContentView` — insight card, then each
/// history row — since every focusable element there opts in via
/// `.focusable()`. Escape closes the popover via `onEscape`.
public struct MenuBarPopoverView: View {
    @ObservedObject private var presentation: PermissionPresentationModel
    private let permissionManager: (any PermissionManagerProtocol)?
    @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject private var serviceConnectionStatus: ServiceConnectionStatusModel
    private let accountStateManager: AccountStateManager?
    private let ipcClient: (any IPCClientProtocol)?
    private let menuStatusViewModel: MenuStatusViewModel?
    private let onEscape: () -> Void
    @State private var showsSettings = false

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)? = nil,
        coordinator: ConcreteDisplayDataCoordinator,
        serviceConnectionStatus: ServiceConnectionStatusModel,
        accountStateManager: AccountStateManager? = nil,
        ipcClient: (any IPCClientProtocol)? = nil,
        menuStatusViewModel: MenuStatusViewModel? = nil,
        onEscape: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.menuStatusViewModel = menuStatusViewModel
        self.onEscape = onEscape
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Velvt")
                    .font(.headline)
                    .foregroundStyle(.primary)
                Spacer()
                Circle().fill(serviceConnectionStatus.status == .connected ? .green : .orange).frame(width: 7, height: 7)
                Button { showsSettings.toggle() } label: { Image(systemName: "gearshape") }
                    .buttonStyle(.plain)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)

            Divider().opacity(0.2)

            if showsSettings, let menuStatusViewModel {
                MenuBarSettingsView(model: menuStatusViewModel, localStatus: serviceConnectionStatus.status) { showsSettings = false }
            } else if let permissionManager, let accountStateManager, let ipcClient, shouldShowOnboarding {
                PermissionRootView(
                    presentation: presentation,
                    permissionManager: permissionManager,
                    accountStateManager: accountStateManager,
                    ipcClient: ipcClient
                )
                .padding(16)
            } else if presentation.showsAccessibilityRecovery {
                PermissionRecoveryView()
                    .padding(16)
            } else {
                VelvtPopoverContentView(coordinator: coordinator)
            }

            if let accountStateManager, let ipcClient {
                Divider().opacity(0.15)
                MenuBarAccountControls(
                    accountStateManager: accountStateManager,
                    ipcClient: ipcClient
                )
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
            }

            if presentation.statuses[.notifications] == .denied {
                Divider().opacity(0.15)
                Text("Notifications are off. Daily insights remain available here.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
            }
        }
        .frame(width: 280)
        .preferredColorScheme(.dark)
        .onExitCommand(perform: onEscape)
    }

    private var shouldShowOnboarding: Bool {
        guard let accountStateManager else { return false }
        if presentation.showsOnboarding { return true }
        if case .loggedIn = accountStateManager.accountState { return false }
        return true
    }
}

private struct MenuBarSettingsView: View {
    @ObservedObject var model: MenuStatusViewModel
    let localStatus: ConnectionStatus
    let close: () -> Void
    @State private var page = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack { Button(action: close) { Image(systemName: "chevron.left") }.buttonStyle(.plain); Text(page == 0 ? "Settings" : "Queued Events").font(.headline); Spacer() }
            if page == 0 {
                Text("App Info").font(.subheadline.bold())
                statusRow("Local service", ready: localStatus == .connected)
                statusRow("Cloud server", ready: model.status?.cloudReady == true)
                HStack { Text("Device").foregroundStyle(.secondary); Spacer(); Text(model.status?.deviceID ?? "Not registered").lineLimit(1) }
                HStack { Button("Refresh", action: model.refresh); Spacer(); Button("Queued events (\(model.status?.queuedEventCount ?? 0))") { page = 1 } }
            } else {
                ScrollView { LazyVStack(alignment: .leading) { ForEach(model.status?.queuedEvents ?? []) { event in Text("\(event.category) · \(event.label)").font(.caption) } } }.frame(height: 160)
                Button("Refresh", action: model.refresh)
            }
        }.padding(16)
    }

    private func statusRow(_ title: String, ready: Bool) -> some View {
        HStack { Circle().fill(ready ? .green : .orange).frame(width: 7, height: 7); Text("\(title): \(ready ? "Ready" : "Unavailable")").font(.caption); Spacer() }
    }
}

private struct MenuBarAccountControls: View {
    @ObservedObject private var accountStateManager: AccountStateManager
    @StateObject private var authViewModel: AuthViewModel
    @State private var authenticationMode: AuthViewModel.AuthMode = .logIn
    @State private var showsAuthentication = false

    init(accountStateManager: AccountStateManager, ipcClient: any IPCClientProtocol) {
        self.accountStateManager = accountStateManager
        _authViewModel = StateObject(wrappedValue: AuthViewModel(
            accountStateManager: accountStateManager,
            ipcClient: ipcClient
        ))
    }

    var body: some View {
        Group {
            switch accountStateManager.accountState {
            case .loggingIn:
                ProgressView("Signing in")
                    .controlSize(.small)
            case .loggingOut:
                ProgressView("Signing out")
                    .controlSize(.small)
            case .pendingErasure:
                Text("Account deletion in progress")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            default:
                HStack(spacing: 8) {
                    ForEach(Array(MenuBarAccountActionResolver.actions(for: accountStateManager.accountState).enumerated()), id: \.offset) { _, action in
                        actionButton(for: action)
                    }
                    Spacer(minLength: 0)
                }
            }
        }
        .sheet(isPresented: $showsAuthentication) {
            MenuBarAuthenticationView(
                authViewModel: authViewModel,
                accountStateManager: accountStateManager,
                initialMode: authenticationMode,
                dismiss: { showsAuthentication = false }
            )
        }
    }

    @ViewBuilder
    private func actionButton(for action: MenuBarAccountAction) -> some View {
        switch action {
        case .authenticate(let mode):
            Button(mode == .logIn ? "Sign In" : "Sign Up") {
                authenticationMode = mode
                authViewModel.authMode = mode
                showsAuthentication = true
            }
        case .logOut:
            Button("Log Out", role: .destructive) {
                authViewModel.logOut()
            }
        }
    }
}

private struct MenuBarAuthenticationView: View {
    @ObservedObject var authViewModel: AuthViewModel
    @ObservedObject var accountStateManager: AccountStateManager
    let initialMode: AuthViewModel.AuthMode
    let dismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(authViewModel.authMode == .signUp ? "Create your account" : "Welcome back")
                .font(.title3.bold())

            CredentialTextField(placeholder: "Email", text: $authViewModel.email)
            CredentialTextField(placeholder: "Password", text: $authViewModel.password, isSecure: true)

            if let error = authViewModel.errorMessage {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            HStack {
                Button("Cancel", action: dismiss)
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
                .buttonStyle(.borderedProminent)
                .disabled(
                    authViewModel.isLoading
                        || authViewModel.email.isEmpty
                        || authViewModel.password.isEmpty
                        || authViewModel.connectionStatus != .connected
                )
            }

            Button(authViewModel.authMode == .signUp ? "I already have an account" : "Create a new account") {
                authViewModel.toggleAuthMode()
            }
            .buttonStyle(.plain)
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(24)
        .frame(width: 360)
        .onAppear { authViewModel.authMode = initialMode }
        .onChange(of: accountStateManager.accountState) { accountState in
            if case .loggedIn = accountState {
                dismiss()
            }
        }
    }
}
