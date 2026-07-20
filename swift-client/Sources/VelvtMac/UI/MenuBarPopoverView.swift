import AppKit
import Combine
import SwiftUI

public enum MenuBarAccountAction: Equatable {
    case authenticate(AuthViewModel.AuthMode)
    case logOut
    case deleteAccount
}
public enum MenuBarAccountActionResolver {
    public static func actions(for accountState: AccountState) -> [MenuBarAccountAction] {
        switch accountState {
        case .loggedOut: return [.authenticate(.logIn), .authenticate(.signUp)]
        case .loggedIn: return [.logOut, .deleteAccount]
        case .loggingIn, .loggingOut, .pendingErasure: return []
        }
    }
}

@MainActor
public final class ServiceConnectionStatusModel: ObservableObject {
    @Published public private(set) var status: ConnectionStatus = .disconnected
    @Published public private(set) var phase: LocalServiceConnectionPhase = .starting
    private var cancellable: AnyCancellable?
    private var graceTimer: AnyCancellable?
    private var lifecycleCancellables = Set<AnyCancellable>()
    private let scheduler: any ConnectionGraceScheduling
    private let graceInterval: TimeInterval
    private var hasConfirmedHandshake = false
    private var isWaking = false

    public init(
        connectionStatus: AnyPublisher<ConnectionStatus, Never>,
        scheduler: (any ConnectionGraceScheduling)? = nil,
        graceInterval: TimeInterval = 4,
        workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter
    ) {
        self.scheduler = scheduler ?? DispatchConnectionGraceScheduler()
        self.graceInterval = graceInterval
        cancellable = connectionStatus.receive(on: RunLoop.main).sink { [weak self] status in
            self?.handle(status)
        }
        workspaceNotifications.publisher(for: NSWorkspace.willSleepNotification)
            .sink { [weak self] _ in self?.prepareForSleep() }
            .store(in: &lifecycleCancellables)
        workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
            .sink { [weak self] _ in self?.handleWake() }
            .store(in: &lifecycleCancellables)
        scheduleTimeout()
    }

    private func handle(_ status: ConnectionStatus) {
        self.status = status
        if status == .connected {
            graceTimer?.cancel()
            hasConfirmedHandshake = true
            isWaking = false
            phase = .connected
            return
        }

        if isWaking {
            phase = .waking
        } else if !hasConfirmedHandshake {
            phase = .starting
        }
        scheduleTimeout()
    }

    private func prepareForSleep() {
        graceTimer?.cancel()
        isWaking = true
    }

    private func handleWake() {
        isWaking = true
        phase = .waking
        scheduleTimeout()
    }

    private func scheduleTimeout() {
        graceTimer?.cancel()
        graceTimer = scheduler.schedule(after: graceInterval) { [weak self] in
            guard let self, self.status != .connected else { return }
            self.isWaking = false
            self.phase = .unavailable
        }
    }
}

public enum LocalServiceConnectionPhase: Equatable, Sendable {
    case starting
    case waking
    case connected
    case unavailable
}

@MainActor
public protocol ConnectionGraceScheduling: AnyObject {
  func schedule(after interval: TimeInterval, action: @escaping @MainActor () -> Void)
    -> AnyCancellable
}

@MainActor
public final class DispatchConnectionGraceScheduler: ConnectionGraceScheduling {
    public init() {}

    public func schedule(
        after interval: TimeInterval,
        action: @escaping @MainActor () -> Void
    ) -> AnyCancellable {
        let workItem = DispatchWorkItem { Task { @MainActor in action() } }
        DispatchQueue.main.asyncAfter(deadline: .now() + interval, execute: workItem)
        return AnyCancellable { workItem.cancel() }
    }
}

@MainActor
public final class CollectionActivityStatusModel: ObservableObject {
    @Published public private(set) var status: CollectionStatus = .idle
    private var cancellable: AnyCancellable?

    public init(collectionStatus: AnyPublisher<CollectionStatus, Never>) {
    cancellable = collectionStatus.receive(on: RunLoop.main).sink { [weak self] in self?.status = $0
    }
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
    cancellable =
      messages
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

    public init(label: String, color: Color) {
        self.label = label
        self.color = color
    }

    public init(status: ConnectionStatus) {
        switch status {
        case .connected:
            label = "Local service connected"
            color = .green
        case .disconnected:
            label = "Disconnected"
            color = .red
        case .connecting, .handshaking, .reconnecting:
            label = "Connecting"
            color = .yellow
        }
    }

    public init(phase: LocalServiceConnectionPhase) {
        switch phase {
        case .starting:
            label = "Starting local service…"
            color = .yellow
        case .waking:
            label = "Waking local service…"
            color = .yellow
        case .connected:
            label = "Local service connected"
            color = .green
        case .unavailable:
            label = "Local service unavailable"
            color = .red
        }
    }
}

public enum MenuBarPopoverLayout {
    public static let preferredContentSize = CGSize(width: 660, height: 450)
    public static let screenInset: CGFloat = 24

    public static func contentSize(for visibleFrame: CGRect?) -> CGSize {
        guard let visibleFrame else { return preferredContentSize }
        return CGSize(
            width: min(preferredContentSize.width, max(1, visibleFrame.width - screenInset)),
            height: min(preferredContentSize.height, max(1, visibleFrame.height - screenInset))
        )
    }
}

public enum MenuBarMotionPolicy {
    public static func shouldAnimate(reduceMotion: Bool) -> Bool {
        !reduceMotion
    }
}

enum SettingsSubmenu: CaseIterable, Equatable {
    case appInfo
    case queuedEvents
    case collectionSettings
    case onboarding
    #if DEBUG
        case debug
    #endif

    var title: String {
        switch self {
        case .appInfo: return "App Info"
        case .queuedEvents: return "Queued Events"
        case .collectionSettings: return "Collection Settings"
        case .onboarding: return "Onboarding & Tour"
        #if DEBUG
            case .debug: return "Debug/Testing"
        #endif
        }
    }

    var preferredHeight: CGFloat {
        switch self {
        case .appInfo: return 420
        case .queuedEvents: return 400
        case .collectionSettings: return 140
        case .onboarding: return 210
        #if DEBUG
        case .debug: return 90
        #endif
        }
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
    private let restartLocalService: (() -> Void)?
    private let replayOnboarding: (() -> Void)?
    private let startGuidedTour: (() -> Void)?
    @ObservedObject private var guidedTour: GuidedTourModel
    @ObservedObject private var metricsStore: AppMetricsStore
    private let popoverWillOpen: AnyPublisher<Void, Never>
    private let onEscape: () -> Void
    private let onTerminate: () -> Void
    @State private var presentedSettingsSubmenu: SettingsSubmenu?
    @State private var confirmsClassificationReset = false
    @State private var confirmsWorkBlockClear = false
    @State private var diagnosticsCopied = false
    @State private var showsFocusSession = false
  @State private var showsSystemState = false
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
        restartLocalService: (() -> Void)? = nil,
        replayOnboarding: (() -> Void)? = nil,
        startGuidedTour: (() -> Void)? = nil,
        guidedTour: GuidedTourModel = GuidedTourModel(),
    metricsStore: AppMetricsStore = AppMetricsStore(
      defaults: UserDefaults(suiteName: "MenuBarPopoverView.preview") ?? .standard),
        popoverWillOpen: AnyPublisher<Void, Never> = Empty().eraseToAnyPublisher(),
        onEscape: @escaping () -> Void,
        onTerminate: @escaping () -> Void = {}
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.collectionActivityStatus = collectionActivityStatus
        self.currentActivity = currentActivity
        self.serviceAlertModel = serviceAlertModel
        self.collectionSettings = collectionSettings
    self.workBlockCoordinator =
      workBlockCoordinator ?? WorkBlockCoordinator(ipcClient: UnavailableWorkBlockIPCClient())
    self.localDashboardCoordinator =
      localDashboardCoordinator
      ?? LocalDashboardCoordinator(ipcClient: UnavailableLocalDashboardIPCClient())
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.menuStatusViewModel = menuStatusViewModel
        self.simulateNotification = simulateNotification
        self.restartLocalService = restartLocalService
        self.replayOnboarding = replayOnboarding
        self.startGuidedTour = startGuidedTour
        self.guidedTour = guidedTour
        self.metricsStore = metricsStore
        self.popoverWillOpen = popoverWillOpen
        self.onEscape = onEscape
        self.onTerminate = onTerminate
    }

    public var body: some View {
        VStack(spacing: 0) {
            mainContent
            if guidedTour.isPresented {
                Divider().opacity(0.2)
                GuidedTourBar(model: guidedTour)
                    .transition(.opacity)
            }
        }
        .animation(
            MenuBarMotionPolicy.shouldAnimate(reduceMotion: reduceMotion)
                ? .easeInOut(duration: 0.18)
                : nil,
            value: guidedTour.isPresented
        )
        .frame(
            idealWidth: MenuBarPopoverLayout.preferredContentSize.width,
            maxWidth: .infinity,
            idealHeight: MenuBarPopoverLayout.preferredContentSize.height,
            maxHeight: .infinity,
            alignment: .top
        )
        .preferredColorScheme(.dark)
        .onExitCommand {
            if guidedTour.isPresented {
                guidedTour.dismiss()
            } else {
                onEscape()
            }
        }
        .onChange(of: guidedTour.isPresented) { isPresented in
            if !isPresented {
                dismissSettingsSubmenus()
            }
        }
        .onReceive(popoverWillOpen) {
            dismissSettingsSubmenus()
        }
    }

    private var mainHeader: some View {
        HStack(alignment: .top, spacing: 8) {
            Image("VelvtWordmark")
                .resizable()
                .renderingMode(.template)
                .scaledToFit()
                .foregroundStyle(Color.velvtText)
                .frame(width: 76, height: 30, alignment: .leading)
                .accessibilityLabel("Velvt")
                .layoutPriority(1)
            Spacer()
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .overlay {
            if guidedTour.isPresented, guidedTour.step == .statusAndRecovery {
                RoundedRectangle(cornerRadius: 7)
                    .stroke(Color.velvtPink, lineWidth: 2)
                    .padding(4)
                    .allowsHitTesting(false)
            }
        }
    }

    private var mainContent: some View {
        VStack(spacing: 0) {
            mainHeader
            Divider().opacity(0.2)
            workspace
        }
        .task {
            _ = await permissionManager?.checkStatus(for: .accessibility)
        }
    }

    private var workspace: some View {
            workspaceDetailPane
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    private var workspaceDetailPane: some View {
        VStack(spacing: 0) {
            if let alert = serviceAlertModel.alert {
                serviceAlertRow(alert)
                Divider().opacity(0.15)
            }
            ScrollView {
                    selectedWorkspaceContent
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .layoutPriority(1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.black.opacity(0.08))
    }

    @ViewBuilder
    private var selectedWorkspaceContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            if presentation.showsAccessibilityRecovery {
                PermissionRecoveryView()
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        Color.velvtSurface.opacity(0.92),
                        in: RoundedRectangle(cornerRadius: 10)
                    )
                    .overlay {
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(Color.white.opacity(0.08), lineWidth: 1)
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
            }
      MinimalDashboardWorkspaceView(
                    coordinator: coordinator,
                    workBlockCoordinator: workBlockCoordinator,
                    localDashboardCoordinator: localDashboardCoordinator,
        onStartWorkBlock: { showsFocusSession = true }
                )

      DisclosureGroup("Settings, privacy & system state", isExpanded: $showsSystemState) {
        VStack(alignment: .leading, spacing: 8) {
          Label(localCollectionPresentation.label, systemImage: "lock.shield")
          Text(backendStatusLabel)
          Text(
            "Raw work activity and local display labels stay on this Mac. Privacy-safe abstractions may synchronize for summaries and insights."
                )
          if collectionActivityStatus.status == .running {
            Text("Local collection active")
          }
          if let accountStateManager, let ipcClient {
            MenuBarAccountControls(accountStateManager: accountStateManager, ipcClient: ipcClient)
          }
                settingsContent
            }
        .font(.caption)
        .foregroundStyle(Color.velvtMuted)
        .padding(.top, 8)
      }
      .font(.caption)
      .tint(Color.velvtText)
      .padding(.horizontal, 12)
      .padding(.bottom, 12)
      .accessibilityHint(
        "Expands settings, privacy wording, collection status, and account controls")
    }
    .popover(isPresented: $showsFocusSession, arrowEdge: .bottom) {
      ScrollView {
        WorkBlockView(coordinator: workBlockCoordinator)
      }
      .frame(width: 400, height: 390, alignment: .top)
      .background(Color.velvtSurface)
      .preferredColorScheme(.dark)
        }
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
            Text("Local collection active")
                .font(.caption)
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
            Text("Settings")
                .font(.headline)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            settingsSubmenuRow(SettingsSubmenu.appInfo.title, submenu: .appInfo)
            settingsSubmenuRow(
                "\(SettingsSubmenu.queuedEvents.title) (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))",
                submenu: .queuedEvents
            )
            settingsSubmenuRow(SettingsSubmenu.collectionSettings.title, submenu: .collectionSettings)
            settingsSubmenuRow(SettingsSubmenu.onboarding.title, submenu: .onboarding)
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
                    "Local privacy service",
                    presentation: connectionPresentation,
                    refresh: { menuStatusViewModel?.refresh() }
                )
                infoRow("Collection", localCollectionPresentation.label)
                infoRow("Cloud sync", uploadStatusDescription)
                infoRow("Last synchronized", lastSuccessfulSyncDescription)
                infoRow("Queued", "\(menuStatusViewModel?.status?.queuedEventCount ?? 0) events")
                infoRow("Next retry", nextRetryDescription)
                #if DEBUG
                    infoRow("Events collected", "\(currentActivity.collectedEventCount)")
                    infoRow("Actions logged", "\(metricsStore.actionsLogged)")
                    infoRow("Interventions", "\(metricsStore.interventions)")
                #endif
                Divider().padding(.vertical, 6)
                Button("Retry Cloud Synchronization") {
                    menuStatusViewModel?.sendAllNow()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                if restartLocalService != nil {
                    Button("Restart Local Service") { restartLocalService?() }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 6)
                }
                Button(diagnosticsCopied ? "Diagnostics Copied" : "Copy Diagnostics") {
                    copyDiagnostics()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
            }
            .onAppear { menuStatusViewModel?.refresh() }

        case .queuedEvents:
            VStack(spacing: 0) {
                submenuTitle("\(submenu.title) (\(menuStatusViewModel?.status?.queuedEventCount ?? 0))")
                classificationExplanation
                let queuedEvents = Array((menuStatusViewModel?.status?.queuedEvents ?? []).prefix(10))
                if queuedEvents.isEmpty {
                    Text("No queued events")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(queuedEvents) { event in
                                queuedEventRow(event)
                            }
                        }
                    }
                    .frame(height: 150)
                }
                if let sendError = menuStatusViewModel?.sendError {
                    Text(sendError)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.top, 8)
                }
                Divider().padding(.top, 8)
                Button("Retry Cloud Synchronization") { menuStatusViewModel?.sendAllNow() }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
                Button("Reset Local Category Corrections", role: .destructive) {
                    confirmsClassificationReset = true
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .onAppear { menuStatusViewModel?.refresh() }
            .confirmationDialog(
                "Reset all local category corrections on this Mac?",
                isPresented: $confirmsClassificationReset,
                titleVisibility: .visible
            ) {
                Button("Reset Learning", role: .destructive) {
                    menuStatusViewModel?.resetClassificationLearning()
                }
                Button("Cancel", role: .cancel) {}
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
                    Button("Cancel", role: .cancel) {}
                }
            }

        case .onboarding:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
                Text(
                    "Replay the full first-run explanation or tour the live menu-bar interface again."
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.bottom, 10)
                Button("Replay Full Intro") {
                    dismissSettingsSubmenus()
                    replayOnboarding?()
                    onEscape()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                Button("Take Guided Tour") {
                    dismissSettingsSubmenus()
                    startGuidedTour?()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }

        #if DEBUG
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
        #endif
        }
    }

    private var connectionPresentation: PopoverConnectionPresentation {
        PopoverConnectionPresentation(phase: serviceConnectionStatus.phase)
    }

    private var localCollectionPresentation: PopoverConnectionPresentation {
        switch presentation.statuses[.accessibility] ?? .unknown {
        case .unknown:
            return PopoverConnectionPresentation(
                label: "Checking Accessibility…",
                color: .gray
            )
        case .denied, .restricted:
            return PopoverConnectionPresentation(
                label: "Collection paused: Accessibility permission required",
                color: .yellow
            )
        case .granted:
            break
        }
        if collectionActivityStatus.status == .running {
            return PopoverConnectionPresentation(label: "Collection active", color: .green)
        }
        return PopoverConnectionPresentation(label: "Collection paused", color: .yellow)
    }

    private var backendStatusLabel: String {
        guard let accountStateManager else { return "Cloud status unavailable" }
        if accountStateManager.requiresReauthentication {
            return "Sign in required"
        }
        guard case .loggedIn = accountStateManager.accountState else {
            return "Sign in required for synchronization"
        }
        guard let status = menuStatusViewModel?.status else {
            return "Checking cloud synchronization…"
        }
        if !status.cloudReady {
            return status.queuedEventCount > 0
                ? "Working offline · \(status.queuedEventCount) queued"
                : "Cloud unreachable"
        }
        if status.uploadStatus == "retrying" || status.uploadStatus == "rate_limited" {
            return "Cloud synchronization retrying"
        }
        return "Cloud synchronized"
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
            email: accountStateManager.accountEmail,
            requiresReauthentication: accountStateManager.requiresReauthentication
        )
    }

    private var isAuthenticated: Bool {
        guard let accountStateManager else { return true }
        if case .loggedIn = accountStateManager.accountState {
            return true
        }
        return false
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

    private var lastSuccessfulSyncDescription: String {
        guard let date = menuStatusViewModel?.status?.lastSuccessfulSyncAt else {
            return "Not yet"
        }
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private var nextRetryDescription: String {
        guard let date = menuStatusViewModel?.status?.nextUploadAttemptAt else {
            return "No retry scheduled"
        }
        return date.formatted(date: .omitted, time: .shortened)
    }

    private func copyDiagnostics() {
        let status = menuStatusViewModel?.status
        let accountStatus: String
        if accountStateManager?.requiresReauthentication == true {
            accountStatus = "sign_in_required"
        } else if isAuthenticated {
            accountStatus = "authenticated"
        } else {
            accountStatus = "signed_out"
        }
    let protocolVersion =
      Bundle.main.object(
                forInfoDictionaryKey: "VelvtProtocolVersion"
            ) as? String ?? "unknown"
        let lines = [
            "Velvt privacy-safe diagnostics",
            "app_version=\(appVersion)",
            "protocol_version=\(protocolVersion)",
            "local_service=\(String(describing: serviceConnectionStatus.phase))",
            "collection=\(collectionDiagnosticCode)",
            "account=\(accountStatus)",
            "backend=\(status?.uploadStatus ?? "unknown")",
            "queued_event_count=\(status?.queuedEventCount ?? 0)",
            "last_successful_sync=\(status?.lastSuccessfulSyncAt?.ISO8601Format() ?? "none")",
            "next_retry=\(status?.nextUploadAttemptAt?.ISO8601Format() ?? "none")",
            "last_error_code=\(status?.lastUploadErrorCode ?? "none")",
        ]
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
        diagnosticsCopied = true
    }

    private var collectionDiagnosticCode: String {
        switch collectionActivityStatus.status {
        case .idle: "idle"
        case .running: "active"
        case .permissionRevoked: "permission_required"
        case .error: "error"
        }
    }

    private func settingsRow(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
      HStack {
        Text(title)
        Spacer()
        Image(systemName: "chevron.right").foregroundStyle(.secondary)
      }
            .contentShape(Rectangle()).padding(.horizontal, 16).padding(.vertical, 12)
        }.buttonStyle(.plain).frame(maxWidth: .infinity)
    }

    private func settingsSubmenuRow(_ title: String, submenu: SettingsSubmenu) -> some View {
    Button {
      showSettingsSubmenu(submenu)
    } label: {
      HStack {
        Text(title)
        Spacer()
        Image(systemName: "chevron.right").foregroundStyle(.secondary)
      }
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
                ScrollView {
                    settingsSubmenuContent(for: submenu)
                }
                .frame(width: 300, height: submenu.preferredHeight, alignment: .top)
                .preferredColorScheme(.dark)
            }
            .frame(width: 1, height: 1)
            .allowsHitTesting(false)
        }
    }

    private func showSettingsSubmenu(_ submenu: SettingsSubmenu) {
        guard presentedSettingsSubmenu != submenu else { return }
        presentedSettingsSubmenu = submenu
    }

    private func dismissSettingsSubmenus() {
        presentedSettingsSubmenu = nil
    }

    private func runDebugInsightSimulation() {
        simulateNotification?()
        dismissSettingsSubmenus()
        onEscape()
    }

    private func submenuBinding(for submenu: SettingsSubmenu) -> Binding<Bool> {
        Binding(
            get: { presentedSettingsSubmenu == submenu },
            set: { isPresented in
                if isPresented {
                    showSettingsSubmenu(submenu)
                } else if presentedSettingsSubmenu == submenu {
                    dismissSettingsSubmenus()
                }
            }
        )
    }
    private func infoRow(_ title: String, _ value: String) -> some View {
    HStack {
      Text(title).foregroundStyle(.secondary)
      Spacer()
      Text(value).lineLimit(1).truncationMode(.middle)
    }
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
            Text(event.category.replacingOccurrences(of: "_", with: " ").capitalized)
                .font(.subheadline)
                .lineLimit(1)
                .truncationMode(.tail)
            Text("Queued \(event.occurredAt.formatted(date: .omitted, time: .shortened))")
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            if event.classificationSource == .userRule {
                Button("Undo correction") {
                    menuStatusViewModel?.undoCorrection(event)
                }
                .buttonStyle(.plain)
                .font(.caption)
      } else if event.classificationStatus != .classified || event.classificationConfidence == .low
      {
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

    private var classificationExplanation: some View {
        Text(
            "Velvt categorizes activity on this Mac. Unclassified means it is not sure. "
                + "Correct category saves a local correction for similar activity; "
                + "raw app and window details stay on this Mac."
        )
        .font(.caption2)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
        .accessibilityLabel(
            "How categories work. Velvt categorizes activity on this Mac. "
                + "Unclassified means it is not sure. Correct category saves a local correction "
                + "for similar activity. Raw app and window details stay on this Mac."
        )
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
  private func statusRow(
    _ title: String, presentation: PopoverConnectionPresentation, refresh: @escaping () -> Void
  ) -> some View {
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

private struct GuidedTourBar: View {
    @ObservedObject var model: GuidedTourModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            tourCopy
            controls
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .background(Color.velvtPanel)
        .accessibilityElement(children: .contain)
    }

    private var tourCopy: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("\(model.progressLabel) · \(model.step.title)")
                .font(.caption.bold())
                .foregroundStyle(Color.velvtText)
            Text(model.step.detail)
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .lineLimit(2)
                .accessibilityLabel(model.step.detail)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Tour step \(model.progressLabel), \(model.step.title)")
        .accessibilityValue(model.step.detail)
    }

    private var controls: some View {
        HStack(alignment: .center) {
            Button("Skip tour") { model.dismiss() }
                .buttonStyle(.plain)
            Spacer(minLength: 16)
            Button("Back") { model.goBack() }
                .disabled(!model.canGoBack)
            Spacer(minLength: 16)
            Button(model.isLastStep ? "Done" : "Next") { model.advance() }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
        }
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
        let contentViewController = context.coordinator.host(content())

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
        let sourceFrame = targetView.window?.convertToScreen(
          targetView.convert(targetView.bounds, to: nil))
      {
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

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        coordinator.dismantle()
    }

    final class Coordinator: NSObject, NSPopoverDelegate {
        let popover = NSPopover()
        private var contentViewController: NSHostingController<Content>?
        private var setPresented: (Bool) -> Void
        private var isDismantling = false

        init(isPresented: Binding<Bool>) {
            setPresented = { isPresented.wrappedValue = $0 }
            super.init()
            popover.behavior = .semitransient
            popover.delegate = self
        }

        func host(_ content: Content) -> NSHostingController<Content> {
            if let contentViewController {
                contentViewController.rootView = content
                return contentViewController
            }
            let contentViewController = NSHostingController(rootView: content)
            self.contentViewController = contentViewController
            popover.contentViewController = contentViewController
            return contentViewController
        }

        func updateBinding(_ isPresented: Binding<Bool>) {
            setPresented = { isPresented.wrappedValue = $0 }
        }

        func dismantle() {
            isDismantling = true
            popover.delegate = nil
            popover.close()
            popover.contentViewController = nil
            contentViewController = nil
        }

        func popoverDidClose(_ notification: Notification) {
            guard !isDismantling else { return }
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
      centeredY + submenuContentSize.height > sourceMenuFrameInScreen.maxY
    {
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
    _authViewModel = StateObject(
      wrappedValue: AuthViewModel(accountStateManager: accountStateManager, ipcClient: ipcClient))
    }
    var body: some View {
        Group {
            switch accountStateManager.accountState {
            case .loggingIn: ProgressView("Signing in").controlSize(.small)
            case .loggingOut: ProgressView("Signing out").controlSize(.small)
      case .pendingErasure:
        Text("Account deletion in progress").font(.caption).foregroundStyle(.secondary)
            default:
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 8) {
            ForEach(
              Array(
                MenuBarAccountActionResolver.actions(for: accountStateManager.accountState)
                  .enumerated()), id: \.offset
            ) { _, action in actionButton(for: action) }
                    }
                    if let error = authViewModel.errorMessage {
                        Text(error)
                            .font(.caption2)
                            .foregroundStyle(.red)
                    }
                }
            }
        }
        .sheet(isPresented: $showsAuthentication) {
      MenuBarAuthenticationView(
        authViewModel: authViewModel, accountStateManager: accountStateManager,
        initialMode: authenticationMode, dismiss: { showsAuthentication = false })
        }
        .confirmationDialog(
            "Delete your Velvt account? This request cannot be undone.",
            isPresented: Binding(
                get: { authViewModel.showDeleteConfirmation },
                set: { if !$0 { authViewModel.cancelAccountDeletion() } }
            ),
            titleVisibility: .visible
        ) {
            Button("Delete Account", role: .destructive) {
                Task { await authViewModel.confirmAccountDeletion() }
            }
            Button("Cancel", role: .cancel) { authViewModel.cancelAccountDeletion() }
        }
    }
    @ViewBuilder private func actionButton(for action: MenuBarAccountAction) -> some View {
        switch action {
    case .authenticate(let mode):
      Button(mode == .logIn ? signInLabel : "Sign Up") {
        authenticationMode = mode
        authViewModel.authMode = mode
        showsAuthentication = true
      }
        case .logOut: Button("Log Out", role: .destructive) { authViewModel.logOut() }
        case .deleteAccount:
            Button("Delete Account", role: .destructive) {
                authViewModel.requestAccountDeletion()
            }
        }
    }
    private var signInLabel: String {
        accountStateManager.requiresReauthentication ? "Reauthenticate" : "Sign In"
    }
}

private struct MenuBarAuthenticationView: View {
    @ObservedObject var authViewModel: AuthViewModel
    @ObservedObject var accountStateManager: AccountStateManager
    let initialMode: AuthViewModel.AuthMode
    let dismiss: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
      Text(authViewModel.authMode == .signUp ? "Create your account" : "Welcome back").font(
        .title3.bold())
            CredentialTextField(placeholder: "Email", text: $authViewModel.email)
            CredentialTextField(placeholder: "Password", text: $authViewModel.password, isSecure: true)
            if let error = authViewModel.errorMessage { Text(error).font(.caption).foregroundStyle(.red) }
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
        }.buttonStyle(.borderedProminent).disabled(
          authViewModel.isLoading || authViewModel.email.isEmpty || authViewModel.password.isEmpty
            || authViewModel.connectionStatus != .connected)
      }
      Button(
        authViewModel.authMode == .signUp ? "I already have an account" : "Create a new account"
      ) { authViewModel.toggleAuthMode() }.buttonStyle(.plain).font(.caption).foregroundStyle(
        .secondary)
    }.padding(24).frame(width: 360).onAppear { authViewModel.authMode = initialMode }.onChange(
      of: accountStateManager.accountState
    ) { if case .loggedIn = $0 { dismiss() } }
    }
}
