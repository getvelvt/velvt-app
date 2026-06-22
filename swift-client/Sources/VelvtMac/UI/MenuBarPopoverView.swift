import Combine
import SwiftUI

public enum MenuBarAccountAction: Equatable {
    case authenticate(AuthViewModel.AuthMode)
    case logOut
}

public enum MenuBarAccountActionResolver {
    public static func actions(for accountState: AccountState) -> [MenuBarAccountAction] {
        switch accountState {
        case .loggedOut: return [.authenticate(.logIn), .authenticate(.signUp)]
        case .loggedIn: return [.logOut]
        case .loggingIn, .loggingOut, .pendingErasure: return []
        }
    }
}

@MainActor
public final class ServiceConnectionStatusModel: ObservableObject {
    @Published public private(set) var status: ConnectionStatus = .disconnected
    private var cancellable: AnyCancellable?

    public init(connectionStatus: AnyPublisher<ConnectionStatus, Never>) {
        cancellable = connectionStatus.receive(on: RunLoop.main).sink { [weak self] in self?.status = $0 }
    }
}

public struct PopoverConnectionPresentation {
    public let label: String
    public let color: Color

    public init(status: ConnectionStatus) {
        switch status {
        case .connected:
            label = "Connected"
            color = .green
        case .disconnected:
            label = "Disconnected"
            color = .red
        case .connecting, .handshaking, .reconnecting:
            label = "Connecting"
            color = .yellow
        }
    }
}

public enum MenuBarPopoverRoute: Equatable {
    case main
    case settings
    case appInfo
    case queuedEvents
}

public enum MenuBarPopoverDirection: Equatable { case forward, backward }

public struct MenuBarPopoverNavigator {
    public private(set) var route: MenuBarPopoverRoute = .main
    public private(set) var direction: MenuBarPopoverDirection = .forward

    public init() {}
    public mutating func showSettings() { move(to: .settings) }
    public mutating func showAppInfo() { move(to: .appInfo) }
    public mutating func showQueuedEvents() { move(to: .queuedEvents) }
    public mutating func goBack() {
        direction = .backward
        route = switch route {
        case .main: .main
        case .settings: .main
        case .appInfo, .queuedEvents: .settings
        }
    }
    private mutating func move(to route: MenuBarPopoverRoute) {
        direction = .forward
        self.route = route
    }
}

public struct MenuBarPopoverView: View {
    @ObservedObject private var presentation: PermissionPresentationModel
    private let permissionManager: (any PermissionManagerProtocol)?
    @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject private var serviceConnectionStatus: ServiceConnectionStatusModel
    private let accountStateManager: AccountStateManager?
    private let ipcClient: (any IPCClientProtocol)?
    private let menuStatusViewModel: MenuStatusViewModel?
    private let onEscape: () -> Void
    private let onTerminate: () -> Void
    @State private var navigator = MenuBarPopoverNavigator()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)? = nil,
        coordinator: ConcreteDisplayDataCoordinator,
        serviceConnectionStatus: ServiceConnectionStatusModel,
        accountStateManager: AccountStateManager? = nil,
        ipcClient: (any IPCClientProtocol)? = nil,
        menuStatusViewModel: MenuStatusViewModel? = nil,
        onEscape: @escaping () -> Void,
        onTerminate: @escaping () -> Void = { }
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.menuStatusViewModel = menuStatusViewModel
        self.onEscape = onEscape
        self.onTerminate = onTerminate
    }

    public var body: some View {
        ZStack {
            routeContent
                .id(navigator.route)
                .transition(transition)
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.18), value: navigator.route)
        .frame(width: 300)
        .preferredColorScheme(.dark)
        .onExitCommand(perform: onEscape)
    }

    private var mainHeader: some View {
        HStack(spacing: 8) {
            Text("Velvt").font(.headline)
            Spacer()
            Text(connectionPresentation.label)
                .font(.caption)
                .foregroundStyle(connectionPresentation.color)
            Circle().fill(connectionPresentation.color).frame(width: 7, height: 7)
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
    }

    @ViewBuilder private var routeContent: some View {
        switch navigator.route {
        case .main: mainContent
        case .settings: settingsContent
        case .appInfo: appInfoContent
        case .queuedEvents: queuedEventsContent
        }
    }

    private var mainContent: some View {
        VStack(spacing: 0) {
            mainHeader
            Divider().opacity(0.2)
            if presentation.showsAccessibilityRecovery {
                PermissionRecoveryView().padding(16)
            } else {
                VelvtPopoverContentView(coordinator: coordinator)
            }
            Divider().opacity(0.15)
            HStack {
                if let accountStateManager, let ipcClient {
                    MenuBarAccountControls(accountStateManager: accountStateManager, ipcClient: ipcClient)
                }
                Spacer(minLength: 0)
                Button("Settings") { navigator.showSettings() }.buttonStyle(.plain)
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
        }
    }

    private var settingsContent: some View {
        VStack(spacing: 0) {
            SettingsTitle(title: "Settings", goBack: { navigator.goBack() })
            settingsRow("App Info") { navigator.showAppInfo() }
            settingsRow("Queued Events (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))") { navigator.showQueuedEvents() }
            Divider().padding(.vertical, 8)
            Button("Quit velvt", role: .destructive, action: onTerminate)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16).padding(.vertical, 8)
        }
        .padding(.bottom, 12)
    }

    private var appInfoContent: some View {
        VStack(spacing: 0) {
            SettingsTitle(title: "App Info", goBack: { navigator.goBack() })
            infoRow("Version", Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "Development")
            infoRow("Device ID", menuStatusViewModel?.status?.deviceID ?? "Not registered")
            infoRow("Authentication", authenticationDescription)
            statusRow(
                "Local service",
                presentation: connectionPresentation,
                refresh: { menuStatusViewModel?.refresh() }
            )
            statusRow(
                "Cloud server",
                presentation: menuStatusViewModel?.status?.cloudReady == true
                    ? PopoverConnectionPresentation(status: .connected)
                    : PopoverConnectionPresentation(status: .disconnected),
                refresh: { menuStatusViewModel?.refresh() }
            )
        }
        .onAppear { menuStatusViewModel?.refresh() }
    }

    private var queuedEventsContent: some View {
        VStack(spacing: 0) {
            SettingsTitle(title: "Queued Events (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))", goBack: { navigator.goBack() })
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array((menuStatusViewModel?.status?.queuedEvents ?? []).prefix(10))) { event in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(event.localLabel ?? event.label).font(.subheadline)
                            Text(event.category.replacingOccurrences(of: "_", with: " ").capitalized)
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16).padding(.vertical, 8)
                    }
                    if menuStatusViewModel?.status?.queuedEvents.isEmpty ?? true {
                        Text("No queued events").foregroundStyle(.secondary).padding(16)
                    }
                }
            }.frame(height: 180)
            Divider().padding(.top, 8)
            Button("Send All Now") { menuStatusViewModel?.sendAllNow() }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
        }
        .onAppear { menuStatusViewModel?.refresh() }
    }

    private var transition: AnyTransition {
        guard !reduceMotion else { return .identity }
        return navigator.direction == .forward
            ? .asymmetric(insertion: .move(edge: .trailing), removal: .move(edge: .leading))
            : .asymmetric(insertion: .move(edge: .leading), removal: .move(edge: .trailing))
    }

    private var connectionPresentation: PopoverConnectionPresentation {
        PopoverConnectionPresentation(status: serviceConnectionStatus.status)
    }

    private var authenticationDescription: String {
        guard let accountStateManager else { return "Not signed in" }
        if let email = accountStateManager.accountEmail {
            return "Authenticated · \(email)"
        }
        return "Not signed in"
    }

    private func settingsRow(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack { Text(title); Spacer(); Image(systemName: "chevron.right").foregroundStyle(.secondary) }
                .contentShape(Rectangle()).padding(.horizontal, 16).padding(.vertical, 12)
        }.buttonStyle(.plain).frame(maxWidth: .infinity)
    }
    private func infoRow(_ title: String, _ value: String) -> some View {
        HStack { Text(title).foregroundStyle(.secondary); Spacer(); Text(value).lineLimit(1).truncationMode(.middle) }
            .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }
    private func statusRow(_ title: String, presentation: PopoverConnectionPresentation, refresh: @escaping () -> Void) -> some View {
        HStack(spacing: 7) {
            Text(title).foregroundStyle(.secondary)
            Spacer()
            Text(presentation.label).foregroundStyle(presentation.color)
            Circle().fill(presentation.color).frame(width: 7, height: 7)
            Button("Refresh", action: refresh).buttonStyle(.plain)
        }
        .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }
}

private struct SettingsTitle: View {
    let title: String
    let goBack: () -> Void
    var body: some View {
        HStack {
            Button(action: goBack) { Image(systemName: "chevron.left").padding(8) }
                .buttonStyle(.plain)
                .contentShape(Rectangle())
            Spacer()
            Text(title).font(.headline)
        }.padding(.horizontal, 10).padding(.vertical, 8)
    }
}

private struct MenuBarAccountControls: View {
    @ObservedObject private var accountStateManager: AccountStateManager
    @StateObject private var authViewModel: AuthViewModel
    @State private var authenticationMode: AuthViewModel.AuthMode = .logIn
    @State private var showsAuthentication = false
    init(accountStateManager: AccountStateManager, ipcClient: any IPCClientProtocol) {
        self.accountStateManager = accountStateManager
        _authViewModel = StateObject(wrappedValue: AuthViewModel(accountStateManager: accountStateManager, ipcClient: ipcClient))
    }
    var body: some View {
        Group {
            switch accountStateManager.accountState {
            case .loggingIn: ProgressView("Signing in").controlSize(.small)
            case .loggingOut: ProgressView("Signing out").controlSize(.small)
            case .pendingErasure: Text("Account deletion in progress").font(.caption).foregroundStyle(.secondary)
            default:
                HStack(spacing: 8) {
                    ForEach(Array(MenuBarAccountActionResolver.actions(for: accountStateManager.accountState).enumerated()), id: \.offset) { _, action in actionButton(for: action) }
                }
            }
        }
        .sheet(isPresented: $showsAuthentication) {
            MenuBarAuthenticationView(authViewModel: authViewModel, accountStateManager: accountStateManager, initialMode: authenticationMode, dismiss: { showsAuthentication = false })
        }
    }
    @ViewBuilder private func actionButton(for action: MenuBarAccountAction) -> some View {
        switch action {
        case .authenticate(let mode): Button(mode == .logIn ? "Sign In" : "Sign Up") { authenticationMode = mode; authViewModel.authMode = mode; showsAuthentication = true }
        case .logOut: Button("Log Out", role: .destructive) { authViewModel.logOut() }
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
            Text(authViewModel.authMode == .signUp ? "Create your account" : "Welcome back").font(.title3.bold())
            CredentialTextField(placeholder: "Email", text: $authViewModel.email)
            CredentialTextField(placeholder: "Password", text: $authViewModel.password, isSecure: true)
            if let error = authViewModel.errorMessage { Text(error).font(.caption).foregroundStyle(.red) }
            HStack { Button("Cancel", action: dismiss); Spacer(); Button(authViewModel.authMode == .signUp ? "Create Account" : "Sign In") { Task { if authViewModel.authMode == .signUp { await authViewModel.signUp() } else { await authViewModel.logIn() } } }.buttonStyle(.borderedProminent).disabled(authViewModel.isLoading || authViewModel.email.isEmpty || authViewModel.password.isEmpty || authViewModel.connectionStatus != .connected) }
            Button(authViewModel.authMode == .signUp ? "I already have an account" : "Create a new account") { authViewModel.toggleAuthMode() }.buttonStyle(.plain).font(.caption).foregroundStyle(.secondary)
        }.padding(24).frame(width: 360).onAppear { authViewModel.authMode = initialMode }.onChange(of: accountStateManager.accountState) { if case .loggedIn = $0 { dismiss() } }
    }
}
