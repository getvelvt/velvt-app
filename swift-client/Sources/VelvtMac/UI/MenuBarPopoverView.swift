import AppKit
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

@MainActor
public final class CollectionActivityStatusModel: ObservableObject {
    @Published public private(set) var status: CollectionStatus = .idle
    private var cancellable: AnyCancellable?

    public init(collectionStatus: AnyPublisher<CollectionStatus, Never>) {
        cancellable = collectionStatus.receive(on: RunLoop.main).sink { [weak self] in self?.status = $0 }
    }
}

public struct CurrentActivity: Equatable, Sendable {
    public let appName: String
    public let windowTitle: String

    public init(appName: String, windowTitle: String) {
        self.appName = appName
        self.windowTitle = windowTitle
    }
}

public final class CurrentActivityModel: ObservableObject, EventSink {
    @Published public private(set) var activity: CurrentActivity?

    public init() {}

    public func receive(_ event: RawEvent) {
        let activity = CurrentActivity(appName: event.appName, windowTitle: event.windowTitle)
        DispatchQueue.main.async { [weak self] in
            self?.activity = activity
        }
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
}

private enum SettingsSubmenu: Equatable {
    case appInfo
    case queuedEvents
}

public enum MenuBarPopoverDirection: Equatable { case forward, backward }

public struct MenuBarPopoverNavigator {
    public private(set) var route: MenuBarPopoverRoute = .main
    public private(set) var direction: MenuBarPopoverDirection = .forward

    public init() {}
    public mutating func showSettings() { move(to: .settings) }
    public mutating func goBack() {
        direction = .backward
        route = switch route {
        case .main: .main
        case .settings: .main
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
    @ObservedObject private var collectionActivityStatus: CollectionActivityStatusModel
    @ObservedObject private var currentActivity: CurrentActivityModel
    private let accountStateManager: AccountStateManager?
    private let ipcClient: (any IPCClientProtocol)?
    private let menuStatusViewModel: MenuStatusViewModel?
    @ObservedObject private var metricsStore: AppMetricsStore
    private let onEscape: () -> Void
    private let onTerminate: () -> Void
    @State private var navigator = MenuBarPopoverNavigator()
    @State private var showsAppInfoSubmenu = false
    @State private var showsQueuedEventsSubmenu = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)? = nil,
        coordinator: ConcreteDisplayDataCoordinator,
        serviceConnectionStatus: ServiceConnectionStatusModel,
        collectionActivityStatus: CollectionActivityStatusModel,
        currentActivity: CurrentActivityModel,
        accountStateManager: AccountStateManager? = nil,
        ipcClient: (any IPCClientProtocol)? = nil,
        menuStatusViewModel: MenuStatusViewModel? = nil,
        metricsStore: AppMetricsStore = AppMetricsStore(defaults: UserDefaults(suiteName: "MenuBarPopoverView.preview") ?? .standard),
        onEscape: @escaping () -> Void,
        onTerminate: @escaping () -> Void = { }
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.collectionActivityStatus = collectionActivityStatus
        self.currentActivity = currentActivity
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.menuStatusViewModel = menuStatusViewModel
        self.metricsStore = metricsStore
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
        }
    }

    private var mainContent: some View {
        VStack(spacing: 0) {
            mainHeader
            Divider().opacity(0.2)
            if collectionActivityStatus.status == .running {
                gatheringInfoStatus
                if let activity = currentActivity.activity {
                    currentActivityStatus(activity)
                }
                Divider().opacity(0.15)
            }
            if presentation.showsAccessibilityRecovery {
                PermissionRecoveryView().padding(16)
            } else {
                VelvtPopoverContentView(coordinator: coordinator)
            }
            metricsRow
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
        .task {
            _ = await permissionManager?.checkStatus(for: .accessibility)
        }
    }

    private var metricsRow: some View {
        HStack(spacing: 10) {
            metricCounter(title: "Actions Logged", value: metricsStore.actionsLogged)
            metricCounter(title: "Interventions", value: metricsStore.interventions)
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 10)
    }

    private func metricCounter(title: String, value: Int) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("\(value)")
                .font(.headline.monospacedDigit())
            Text(title)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var gatheringInfoStatus: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
                .frame(width: 14, height: 14)
            Text("Gathering info")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    private func currentActivityStatus(_ activity: CurrentActivity) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("Currently on")
                .font(.caption2)
                .foregroundStyle(.secondary)
            Text(activity.appName)
                .font(.caption.bold())
                .lineLimit(1)
            if !activity.windowTitle.isEmpty {
                Text(activity.windowTitle)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.bottom, 9)
    }

    private var settingsContent: some View {
        VStack(spacing: 0) {
            SettingsTitle(title: "Settings", goBack: {
                dismissSettingsSubmenus()
                navigator.goBack()
            })
            settingsSubmenuRow("App Info", submenu: .appInfo)
            settingsSubmenuRow("Queued Events (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))", submenu: .queuedEvents)
            Divider().padding(.vertical, 8)
            HStack {
                Button("Quit Velvt", role: .destructive, action: onTerminate)
                    .buttonStyle(.bordered)
                Spacer(minLength: 12)
                Text("Velvt \(appVersion)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 16).padding(.vertical, 8)
        }
        .padding(.bottom, 12)
        .onAppear { dismissSettingsSubmenus() }
    }

    @ViewBuilder
    private func settingsSubmenuContent(for submenu: SettingsSubmenu) -> some View {
        switch submenu {
        case .appInfo:
            VStack(spacing: 0) {
                submenuTitle("App Info")
                infoRow("Version", appVersion)
                infoRow("Device ID", menuStatusViewModel?.status?.deviceID ?? "Not registered")
                authenticationInfoRow()
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

        case .queuedEvents:
            VStack(spacing: 0) {
                submenuTitle("Queued Events (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))")
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        let queuedEvents = Array((menuStatusViewModel?.status?.queuedEvents ?? []).prefix(10))
                        ForEach(queuedEvents) { event in
                            queuedEventRow(event)
                        }
                        if queuedEvents.isEmpty {
                            Text("No queued events")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 16)
                                .padding(.vertical, 10)
                        }
                    }
                }
                .frame(height: 180)
                if let sendError = menuStatusViewModel?.sendError {
                    Text(sendError)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.top, 8)
                }
                Divider().padding(.top, 8)
                Button("Send All Now") { menuStatusViewModel?.sendAllNow() }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
            }
            .onAppear { menuStatusViewModel?.refresh() }
        }
    }

    private var transition: AnyTransition {
        guard !reduceMotion else { return .identity }
        return navigator.direction == .forward
            ? .asymmetric(insertion: .move(edge: .trailing), removal: .move(edge: .trailing))
            : .asymmetric(insertion: .move(edge: .leading), removal: .move(edge: .leading))
    }

    private var connectionPresentation: PopoverConnectionPresentation {
        PopoverConnectionPresentation(status: serviceConnectionStatus.status)
    }

    private var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            ?? Bundle.main.object(forInfoDictionaryKey: "VelvtClientVersion") as? String
            ?? "Development"
    }

    private var authenticationPresentation: AuthenticationStatusPresentation {
        guard let accountStateManager else {
            return AuthenticationStatusPresentation(accountState: .loggedOut, email: nil)
        }
        return AuthenticationStatusPresentation(
            accountState: accountStateManager.accountState,
            email: accountStateManager.accountEmail
        )
    }

    private func settingsRow(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack { Text(title); Spacer(); Image(systemName: "chevron.right").foregroundStyle(.secondary) }
                .contentShape(Rectangle()).padding(.horizontal, 16).padding(.vertical, 12)
        }.buttonStyle(.plain).frame(maxWidth: .infinity)
    }

    private func settingsSubmenuRow(_ title: String, submenu: SettingsSubmenu) -> some View {
        Button { showSettingsSubmenu(submenu) } label: {
            HStack { Text(title); Spacer(); Image(systemName: "chevron.right").foregroundStyle(.secondary) }
                .contentShape(Rectangle())
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
        }
        .buttonStyle(.plain)
        .frame(maxWidth: .infinity)
        .onHover { if $0 { showSettingsSubmenu(submenu) } }
        .background(
            SubmenuPopoverAnchor(
                isPresented: submenu == .appInfo ? $showsAppInfoSubmenu : $showsQueuedEventsSubmenu
            ) {
                settingsSubmenuContent(for: submenu)
                    .frame(width: 280)
                    .preferredColorScheme(.dark)
            }
        )
    }

    private func showSettingsSubmenu(_ submenu: SettingsSubmenu) {
        showsAppInfoSubmenu = submenu == .appInfo
        showsQueuedEventsSubmenu = submenu == .queuedEvents
    }

    private func dismissSettingsSubmenus() {
        showsAppInfoSubmenu = false
        showsQueuedEventsSubmenu = false
    }
    private func infoRow(_ title: String, _ value: String) -> some View {
        HStack { Text(title).foregroundStyle(.secondary); Spacer(); Text(value).lineLimit(1).truncationMode(.middle) }
            .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }
    private func authenticationInfoRow() -> some View {
        let presentation = authenticationPresentation
        return HStack(spacing: 7) {
            Text("Authentication").foregroundStyle(.secondary)
            Spacer()
            Text(presentation.text)
                .lineLimit(1)
                .truncationMode(.middle)
            Circle()
                .fill(presentation.indicatorColor == .green ? Color.green : Color.red)
                .frame(width: 7, height: 7)
        }
        .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }
    private func queuedEventRow(_ event: QueuedEventSummary) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(event.localLabel ?? event.label)
                .font(.subheadline)
                .lineLimit(1)
                .truncationMode(.tail)
            Text(event.category.replacingOccurrences(of: "_", with: " ").capitalized)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
    }
    private func statusRow(_ title: String, presentation: PopoverConnectionPresentation, refresh: @escaping () -> Void) -> some View {
        HStack(spacing: 7) {
            Text(title).foregroundStyle(.secondary)
            Spacer()
            Button(action: refresh) {
                Text(presentation.label)
                    .foregroundStyle(presentation.color)
            }
            .buttonStyle(.plain)
            .help("Click to refresh status")
            Circle().fill(presentation.color).frame(width: 7, height: 7)
        }
        .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }

    private func submenuTitle(_ title: String) -> some View {
        Text(title)
            .font(.headline)
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
    }
}

private struct SettingsTitle: View {
    let title: String
    let goBack: () -> Void
    var body: some View {
        ZStack {
            Text(title)
                .font(.headline)
            .frame(maxWidth: .infinity, alignment: .center)

            HStack {
                Button(action: goBack) {
                    Image(systemName: "chevron.left")
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                Spacer()
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
    }
}

private struct SubmenuPopoverAnchor<Content: View>: NSViewRepresentable {
    @Binding var isPresented: Bool
    let content: () -> Content

    func makeCoordinator() -> Coordinator {
        Coordinator(isPresented: $isPresented)
    }

    func makeNSView(context: Context) -> NSView {
        NSView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        context.coordinator.updateBinding($isPresented)
        let popover = context.coordinator.popover
        popover.contentViewController = NSHostingController(rootView: content())

        if isPresented, !popover.isShown {
            popover.show(relativeTo: nsView.bounds, of: nsView, preferredEdge: .maxX)
        } else if !isPresented, popover.isShown {
            popover.performClose(nil)
        }
    }

    final class Coordinator: NSObject, NSPopoverDelegate {
        let popover = NSPopover()
        private var setPresented: (Bool) -> Void

        init(isPresented: Binding<Bool>) {
            setPresented = { isPresented.wrappedValue = $0 }
            super.init()
            popover.behavior = .semitransient
            popover.delegate = self
        }

        func updateBinding(_ isPresented: Binding<Bool>) {
            setPresented = { isPresented.wrappedValue = $0 }
        }

        func popoverDidClose(_ notification: Notification) {
            setPresented(false)
        }
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
