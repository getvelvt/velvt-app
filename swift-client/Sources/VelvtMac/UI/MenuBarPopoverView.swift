import AppKit
import Combine
import SwiftUI

public enum MenuBarAccountAction: Equatable {
    case authenticate(AuthViewModel.AuthMode)
    case logOut
}

private struct HistoryWorkspaceView: View {
    @ObservedObject var coordinator: ConcreteDisplayDataCoordinator
    let accountStateManager: AccountStateManager?

    var body: some View {
        if let accountStateManager {
            AccountGatedHistoryWorkspaceView(
                coordinator: coordinator,
                accountStateManager: accountStateManager
            )
        } else {
            VelvtPopoverContentView(coordinator: coordinator)
        }
    }
}

private struct AccountGatedHistoryWorkspaceView: View {
    @ObservedObject var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject var accountStateManager: AccountStateManager

    var body: some View {
        switch accountStateManager.accountState {
        case .loggedIn:
            VelvtPopoverContentView(coordinator: coordinator)
        case .loggedOut, .loggingIn, .loggingOut, .pendingErasure:
            EmptyDeliveryState(
                text: "Sign in to view your 7-day history",
                systemImage: "person.crop.circle.badge.exclamationmark"
            )
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityLabel("Sign in to view 7-Day History")
        }
    }
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

public final class CollectionSettingsModel: ObservableObject {
    @Published public var offlineEventCollectionEnabled: Bool {
        didSet {
            defaults.set(offlineEventCollectionEnabled, forKey: Self.offlineEventCollectionKey)
        }
    }

    private static let offlineEventCollectionKey = "velvt.collection.offline_events_enabled"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if defaults.object(forKey: Self.offlineEventCollectionKey) == nil {
            offlineEventCollectionEnabled = true
        } else {
            offlineEventCollectionEnabled = defaults.bool(forKey: Self.offlineEventCollectionKey)
        }
    }
}

public struct ServiceAlert: Equatable, Sendable {
    public enum Severity: Equatable, Sendable {
        case warning
        case error
    }

    public let severity: Severity
    public let title: String
    public let message: String

    public init(severity: Severity, title: String, message: String) {
        self.severity = severity
        self.title = title
        self.message = message
    }
}

@MainActor
public final class ServiceAlertModel: ObservableObject {
    @Published public private(set) var alert: ServiceAlert?
    private var cancellable: AnyCancellable?

    public init(messages: some Publisher<ServerMessage, Never>) {
        cancellable = messages
            .receive(on: RunLoop.main)
            .compactMap(Self.alert(for:))
            .sink { [weak self] in self?.alert = $0 }
    }

    public func dismiss() {
        alert = nil
    }

    private static func alert(for message: ServerMessage) -> ServiceAlert? {
        switch message {
        case .malformedMessage:
            return ServiceAlert(
                severity: .warning,
                title: "Message rejected",
                message: "The local service rejected an invalid message."
            )
        case .privacyViolationAlert(let alert):
            return ServiceAlert(
                severity: .error,
                title: "Privacy guard blocked data",
                message: alert.message
            )
        case .shuttingDown:
            return ServiceAlert(
                severity: .warning,
                title: "Service restarting",
                message: "Velvt is reconnecting to the local service."
            )
        case .errorResponse(let error):
            return ServiceAlert(
                severity: .error,
                title: "Service error",
                message: error.message
            )
        default:
            return nil
        }
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
    @Published public private(set) var collectedEventCount = 0

    public init() {}

    public func receive(_ event: RawEvent) {
        let activity = CurrentActivity(appName: event.appName, windowTitle: event.windowTitle)
        DispatchQueue.main.async { [weak self] in
            self?.activity = activity
            self?.collectedEventCount += 1
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

public enum MenuBarPopoverLayout {
    public static let preferredContentSize = CGSize(width: 620, height: 580)
    public static let screenInset: CGFloat = 24

    public static func contentSize(for visibleFrame: CGRect?) -> CGSize {
        guard let visibleFrame else { return preferredContentSize }
        return CGSize(
            width: min(preferredContentSize.width, max(1, visibleFrame.width - screenInset)),
            height: min(preferredContentSize.height, max(1, visibleFrame.height - screenInset))
        )
    }
}

public enum MenuBarWorkspaceTab: CaseIterable, Equatable, Hashable {
    case workBlock
    case history
    case recentActivity

    public var title: String {
        switch self {
        case .workBlock: return "Work Block"
        case .history: return "7-Day History"
        case .recentActivity: return "Recent Activity"
        }
    }

    fileprivate var systemImage: String {
        switch self {
        case .workBlock: return "timer"
        case .history: return "calendar"
        case .recentActivity: return "waveform.path.ecg"
        }
    }

    fileprivate var keyboardShortcut: KeyEquivalent {
        switch self {
        case .workBlock: return "1"
        case .history: return "2"
        case .recentActivity: return "3"
        }
    }
}

enum SettingsSubmenu: CaseIterable, Equatable {
    case appInfo
    case queuedEvents
    case collectionSettings
    case debug

    var title: String {
        switch self {
        case .appInfo: return "App Info"
        case .queuedEvents: return "Queued Events"
        case .collectionSettings: return "Collection Settings"
        case .debug: return "Debug"
        }
    }
}

public enum MenuBarPopoverDirection: Equatable { case forward, backward }

public struct MenuBarPopoverNavigator {
    public private(set) var route: MenuBarPopoverRoute = .main
    public private(set) var direction: MenuBarPopoverDirection = .forward
    public private(set) var selectedWorkspaceTab: MenuBarWorkspaceTab = .workBlock

    public init() {}
    public mutating func selectWorkspaceTab(_ tab: MenuBarWorkspaceTab) {
        selectedWorkspaceTab = tab
    }
    public mutating func showSettings() { move(to: .settings) }
    public mutating func goBack() {
        direction = .backward
        route = switch route {
        case .main: .main
        case .settings: .main
        }
    }
    public mutating func resetForPopoverOpening() {
        route = .main
        direction = .backward
        selectedWorkspaceTab = .workBlock
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
    @ObservedObject private var serviceAlertModel: ServiceAlertModel
    @ObservedObject private var collectionSettings: CollectionSettingsModel
    @ObservedObject private var workBlockCoordinator: WorkBlockCoordinator
    @ObservedObject private var localDashboardCoordinator: LocalDashboardCoordinator
    private let accountStateManager: AccountStateManager?
    private let ipcClient: (any IPCClientProtocol)?
    private let menuStatusViewModel: MenuStatusViewModel?
    private let simulateNotification: (() -> Void)?
    @ObservedObject private var metricsStore: AppMetricsStore
    private let popoverWillOpen: AnyPublisher<Void, Never>
    private let onEscape: () -> Void
    private let onTerminate: () -> Void
    @State private var navigator = MenuBarPopoverNavigator()
    @State private var showsAppInfoSubmenu = false
    @State private var showsQueuedEventsSubmenu = false
    @State private var showsCollectionSettingsSubmenu = false
    @State private var showsDebugSubmenu = false
    @State private var confirmsClassificationReset = false
    @State private var confirmsWorkBlockClear = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)? = nil,
        coordinator: ConcreteDisplayDataCoordinator,
        serviceConnectionStatus: ServiceConnectionStatusModel,
        collectionActivityStatus: CollectionActivityStatusModel,
        currentActivity: CurrentActivityModel,
        serviceAlertModel: ServiceAlertModel,
        collectionSettings: CollectionSettingsModel = CollectionSettingsModel(),
        workBlockCoordinator: WorkBlockCoordinator? = nil,
        localDashboardCoordinator: LocalDashboardCoordinator? = nil,
        accountStateManager: AccountStateManager? = nil,
        ipcClient: (any IPCClientProtocol)? = nil,
        menuStatusViewModel: MenuStatusViewModel? = nil,
        simulateNotification: (() -> Void)? = nil,
        metricsStore: AppMetricsStore = AppMetricsStore(defaults: UserDefaults(suiteName: "MenuBarPopoverView.preview") ?? .standard),
        popoverWillOpen: AnyPublisher<Void, Never> = Empty().eraseToAnyPublisher(),
        onEscape: @escaping () -> Void,
        onTerminate: @escaping () -> Void = { }
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.collectionActivityStatus = collectionActivityStatus
        self.currentActivity = currentActivity
        self.serviceAlertModel = serviceAlertModel
        self.collectionSettings = collectionSettings
        self.workBlockCoordinator = workBlockCoordinator ?? WorkBlockCoordinator(ipcClient: UnavailableWorkBlockIPCClient())
        self.localDashboardCoordinator = localDashboardCoordinator ?? LocalDashboardCoordinator(ipcClient: UnavailableLocalDashboardIPCClient())
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.menuStatusViewModel = menuStatusViewModel
        self.simulateNotification = simulateNotification
        self.metricsStore = metricsStore
        self.popoverWillOpen = popoverWillOpen
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
        .frame(
            idealWidth: MenuBarPopoverLayout.preferredContentSize.width,
            maxWidth: .infinity,
            idealHeight: MenuBarPopoverLayout.preferredContentSize.height,
            maxHeight: .infinity,
            alignment: .top
        )
        .preferredColorScheme(.dark)
        .onExitCommand(perform: onEscape)
        .onReceive(popoverWillOpen) {
            dismissSettingsSubmenus()
            navigator.resetForPopoverOpening()
        }
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
            if let alert = serviceAlertModel.alert {
                serviceAlertRow(alert)
                Divider().opacity(0.15)
            }
            if collectionActivityStatus.status == .running {
                gatheringInfoStatus
                Divider().opacity(0.15)
            }
            workspace
            if metricsStore.isAuthenticated {
                metricsRow
                Divider().opacity(0.15)
            }
            HStack {
                if let accountStateManager, let ipcClient {
                    MenuBarAccountControls(accountStateManager: accountStateManager, ipcClient: ipcClient)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16).padding(.vertical, 10)
        }
        .task {
            _ = await permissionManager?.checkStatus(for: .accessibility)
        }
    }

    private var workspace: some View {
        HStack(spacing: 0) {
            workspaceNavigationRail
                .frame(width: 152)

            Divider().opacity(0.2)

            ScrollView {
                selectedWorkspaceContent
                    .frame(maxWidth: .infinity, alignment: .topLeading)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    private var workspaceNavigationRail: some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(MenuBarWorkspaceTab.allCases, id: \.self) { tab in
                workspaceNavigationButton(tab)
            }

            Spacer(minLength: 12)
            Divider().opacity(0.2)
            Button {
                navigator.showSettings()
            } label: {
                Label("Settings", systemImage: "gearshape")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .keyboardShortcut(",", modifiers: .command)
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .accessibilityLabel("Open Settings")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 10)
        .background(Color.velvtSurface.opacity(0.55))
    }

    private func workspaceNavigationButton(_ tab: MenuBarWorkspaceTab) -> some View {
        let isSelected = navigator.selectedWorkspaceTab == tab
        return Button {
            navigator.selectWorkspaceTab(tab)
        } label: {
            Label(tab.title, systemImage: tab.systemImage)
                .font(.caption)
                .foregroundStyle(isSelected ? Color.velvtText : Color.velvtMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .padding(.horizontal, 10)
                .padding(.vertical, 9)
                .background(isSelected ? Color.velvtPanelHighlight : Color.clear)
                .clipShape(RoundedRectangle(cornerRadius: 7))
        }
        .buttonStyle(.plain)
        .keyboardShortcut(tab.keyboardShortcut, modifiers: .command)
        .accessibilityLabel(tab.title)
        .accessibilityValue(isSelected ? "Selected" : "")
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    @ViewBuilder
    private var selectedWorkspaceContent: some View {
        if presentation.showsOnboarding {
            GoalOnboardingView { intensity, purpose in
                presentation.saveGoal(intensity: intensity, purpose: purpose)
            }
            .padding(16)
        } else if presentation.showsAccessibilityRecovery {
            PermissionRecoveryView().padding(16)
        } else {
            switch navigator.selectedWorkspaceTab {
            case .workBlock:
                WorkBlockView(coordinator: workBlockCoordinator)
            case .history:
                HistoryWorkspaceView(
                    coordinator: coordinator,
                    accountStateManager: accountStateManager
                )
            case .recentActivity:
                LocalDashboardView(coordinator: localDashboardCoordinator)
            }
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
            Text("\(currentActivity.collectedEventCount) events")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    private func serviceAlertRow(_ alert: ServiceAlert) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(alert.severity == .error ? Color.red : Color.yellow)
                .frame(width: 7, height: 7)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 2) {
                Text(alert.title)
                    .font(.caption.bold())
                Text(alert.message)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
            Button("Dismiss") {
                serviceAlertModel.dismiss()
            }
            .buttonStyle(.plain)
            .font(.caption2)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    private var settingsContent: some View {
        VStack(spacing: 0) {
            SettingsTitle(title: "Settings", goBack: {
                dismissSettingsSubmenus()
                navigator.goBack()
            })
            settingsSubmenuRow(SettingsSubmenu.appInfo.title, submenu: .appInfo)
            settingsSubmenuRow(
                "\(SettingsSubmenu.queuedEvents.title) (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))",
                submenu: .queuedEvents
            )
            settingsSubmenuRow(SettingsSubmenu.collectionSettings.title, submenu: .collectionSettings)
            #if DEBUG
            if simulateNotification != nil {
                settingsSubmenuRow(SettingsSubmenu.debug.title, submenu: .debug)
            }
            #endif
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
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .onAppear { dismissSettingsSubmenus() }
    }

    @ViewBuilder
    private func settingsSubmenuContent(for submenu: SettingsSubmenu) -> some View {
        switch submenu {
        case .appInfo:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
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
                infoRow("Uploads", uploadStatusDescription)
                infoRow("Events collected", "\(currentActivity.collectedEventCount)")
            }
            .onAppear { menuStatusViewModel?.refresh() }

        case .queuedEvents:
            VStack(spacing: 0) {
                submenuTitle("\(submenu.title) (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))")
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
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                Button("Reset Classification Learning", role: .destructive) {
                    confirmsClassificationReset = true
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .onAppear { menuStatusViewModel?.refresh() }
            .confirmationDialog(
                "Reset all classification corrections on this Mac?",
                isPresented: $confirmsClassificationReset,
                titleVisibility: .visible
            ) {
                Button("Reset Learning", role: .destructive) {
                    menuStatusViewModel?.resetClassificationLearning()
                }
                Button("Cancel", role: .cancel) { }
            }

        case .collectionSettings:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
                Toggle("Offline Event Collection", isOn: $collectionSettings.offlineEventCollectionEnabled)
                    .toggleStyle(.switch)
                    .font(.caption)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                Button("Clear Local Work Blocks", role: .destructive) {
                    confirmsWorkBlockClear = true
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .confirmationDialog(
                    "Clear local intentions, work blocks, and results from this Mac?",
                    isPresented: $confirmsWorkBlockClear,
                    titleVisibility: .visible
                ) {
                    Button("Clear Local Work Blocks", role: .destructive) {
                        workBlockCoordinator.clearLocalData()
                    }
                    Button("Cancel", role: .cancel) { }
                }
            }

        case .debug:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
                Button {
                    runDebugInsightSimulation()
                } label: {
                    HStack {
                        Image(systemName: "bell.badge")
                        Text("Simulate Insight")
                        Spacer()
                    }
                    .contentShape(Rectangle())
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                }
                .buttonStyle(.plain)
                .frame(maxWidth: .infinity)
            }
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

    private var uploadStatusDescription: String {
        guard let status = menuStatusViewModel?.status else { return "Unknown" }
        switch status.uploadStatus {
        case "ready":
            return "Ready"
        case "pending":
            return withNextUploadAttempt("\(status.pendingUploadBatchCount) pending", status)
        case "retrying":
            return retryDescription(status)
        case "auth_required":
            return withNextUploadAttempt("Sign in required", status)
        case "network_unavailable":
            return withNextUploadAttempt("Network unavailable", status)
        case "rate_limited":
            return retryDescription(status)
        case "privacy_rejected":
            return withNextUploadAttempt("Privacy check failed", status)
        default:
            return withNextUploadAttempt(status.lastUploadErrorCode ?? status.uploadStatus, status)
        }
    }

    private func retryDescription(_ status: MenuStatus) -> String {
        let prefix: String
        if let error = status.lastUploadErrorCode, !error.isEmpty {
            prefix = "\(status.failedUploadBatchCount) retrying · \(error)"
        } else {
            prefix = "\(status.failedUploadBatchCount) retrying"
        }
        return withNextUploadAttempt(prefix, status)
    }

    private func withNextUploadAttempt(_ description: String, _ status: MenuStatus) -> String {
        guard let retryAt = status.nextUploadAttemptAt else { return description }
        return "\(description) · next retry \(retryAt.formatted(date: .omitted, time: .shortened))"
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
        .overlay(alignment: .trailing) {
            SubmenuPopoverAnchor(
                isPresented: submenuBinding(for: submenu)
            ) {
                settingsSubmenuContent(for: submenu)
                    .frame(width: 280)
                    .preferredColorScheme(.dark)
            }
            .frame(width: 1, height: 1)
            .allowsHitTesting(false)
        }
    }

    private func showSettingsSubmenu(_ submenu: SettingsSubmenu) {
        showsAppInfoSubmenu = submenu == .appInfo
        showsQueuedEventsSubmenu = submenu == .queuedEvents
        showsCollectionSettingsSubmenu = submenu == .collectionSettings
        showsDebugSubmenu = submenu == .debug
    }

    private func dismissSettingsSubmenus() {
        showsAppInfoSubmenu = false
        showsQueuedEventsSubmenu = false
        showsCollectionSettingsSubmenu = false
        showsDebugSubmenu = false
    }

    private func runDebugInsightSimulation() {
        simulateNotification?()
        dismissSettingsSubmenus()
        navigator.goBack()
        onEscape()
    }

    private func submenuBinding(for submenu: SettingsSubmenu) -> Binding<Bool> {
        switch submenu {
        case .appInfo:
            return $showsAppInfoSubmenu
        case .queuedEvents:
            return $showsQueuedEventsSubmenu
        case .collectionSettings:
            return $showsCollectionSettingsSubmenu
        case .debug:
            return $showsDebugSubmenu
        }
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
            if event.classificationSource == .userRule {
                Button("Undo correction") {
                    menuStatusViewModel?.undoCorrection(event)
                }
                .buttonStyle(.plain)
                .font(.caption)
            } else if event.classificationStatus != .classified || event.classificationConfidence == .low {
                Picker(
                    "Correct category",
                    selection: Binding(
                        get: { event.category },
                        set: { menuStatusViewModel?.correct(event, category: $0) }
                    )
                ) {
                    ForEach(Self.classificationCategories, id: \.self) { category in
                        Text(category.replacingOccurrences(of: "_", with: " ").capitalized)
                            .tag(category)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .controlSize(.small)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
    }

    private static let classificationCategories = [
        "FOCUS_WORK",
        "PASSIVE_CONSUMPTION",
        "SOCIAL_FEED",
        "COMMUNICATION",
        "TASK_MANAGEMENT",
        "REFERENCE",
        "SYSTEM",
        "UNLOGGED",
    ]
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
        let contentViewController = NSHostingController(rootView: content())
        popover.contentViewController = contentViewController

        if isPresented {
            let targetView = nsView.bounds.isEmpty ? (nsView.superview ?? nsView) : nsView
            let sourceRect = NSRect(
                x: targetView.bounds.maxX - 1,
                y: targetView.bounds.midY,
                width: 1,
                height: 1
            )
            contentViewController.view.layoutSubtreeIfNeeded()
            let contentSize = contentViewController.view.fittingSize
            popover.contentSize = contentSize

            if !popover.isShown {
                popover.show(relativeTo: sourceRect, of: targetView, preferredEdge: .maxX)
            }
            if let window = popover.contentViewController?.view.window,
               let sourceFrame = targetView.window?.convertToScreen(targetView.convert(targetView.bounds, to: nil)) {
                window.setFrame(
                    SubmenuPopoverPlacement.frame(
                        sourceFrameInScreen: sourceFrame,
                        submenuContentSize: contentSize,
                        sourceMenuFrameInScreen: targetView.window?.frame,
                        currentWindowFrame: window.frame
                    ),
                    display: true
                )
            }
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

struct SubmenuPopoverPlacement {
    static func frame(
        sourceFrameInScreen: CGRect,
        submenuContentSize: CGSize,
        sourceMenuFrameInScreen: CGRect? = nil,
        currentWindowFrame: CGRect? = nil
    ) -> CGRect {
        let x = currentWindowFrame?.minX ?? sourceFrameInScreen.maxX
        let centeredY = sourceFrameInScreen.midY - submenuContentSize.height / 2
        let y: CGFloat
        if let sourceMenuFrameInScreen,
           centeredY + submenuContentSize.height > sourceMenuFrameInScreen.maxY {
            y = sourceMenuFrameInScreen.maxY - submenuContentSize.height
        } else {
            y = centeredY
        }
        return CGRect(
            x: x,
            y: y,
            width: submenuContentSize.width,
            height: submenuContentSize.height
        )
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
