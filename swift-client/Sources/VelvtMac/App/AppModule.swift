import AppKit
import Combine
import ServiceManagement
import SwiftUI
import UserNotifications

/// App module - owns application lifecycle and menu bar setup.
/// Does NOT own event capture, IPC processing, abstraction, cloud calls, or
/// insight generation.

public protocol AppLifecycleManaging: AnyObject {
    func start() async throws
    func stop() async
}

/// Surfaced via `ServiceManager.state = .failed(...)` when the launch sequence
/// fails at a step ServiceManager itself doesn't know about (e.g. the IPC
/// socket never appearing, or config failing to load).
enum AppLaunchError: LocalizedError {
    case socketNotReady(path: String)

    var errorDescription: String? {
        switch self {
        case .socketNotReady(let path):
            return "The Velvt service did not create its IPC socket at \(path) in time."
        }
    }
}

/// AppKit delegate used by the SwiftUI application entry point.
public final class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    public let permissionManager: PermissionManager
    public let permissionPresentation: PermissionPresentationModel
    public let accountStateManager: AccountStateManager
    public private(set) var displayCoordinator: ConcreteDisplayDataCoordinator?

    /// Tracks the live IPC connection state so `VelvtMacApp.body` can gate the
    /// auth UI — preventing any `send()` call before the socket handshake is done.
    @Published public private(set) var ipcConnectionStatus: ConnectionStatus = .disconnected

    var ipcClient: (any IPCClientProtocol)?
    private var connectionStatusCancellable: AnyCancellable?
    private var eventRelay: (any EventRelayProtocol)?
    private var collectionAgent: (any CollectionAgentProtocol)?
    private var permissionCoordinator: PermissionCollectionCoordinator?
    private var menuBarController: MenuBarController?
    private var notificationDeliveryCoordinator: NotificationDeliveryCoordinator?
    private var notificationResponseRouter: NotificationResponseRouter?
    let serviceManager: ServiceManager

    @MainActor public override init() {
        let permissionManager = PermissionManager()
        self.permissionManager = permissionManager
        permissionPresentation = PermissionPresentationModel(
            permissionManager: permissionManager,
            onboardingStateStore: UserDefaultsOnboardingStateStore()
        )
        accountStateManager = AccountStateManager(keychain: KeychainService())
        serviceManager = ServiceManager()
        super.init()
    }

    public func applicationDidFinishLaunching(_ notification: Notification) {
        permissionManager.startMonitoring()
        Task {
            _ = await permissionManager.checkStatus(for: .accessibility)
            _ = await permissionManager.checkStatus(for: .notifications)
        }

        Task { @MainActor in
            await launchServiceAndConnect()
        }
    }

    /// Installs/starts the bundled Rust service, then waits for it to actually
    /// bind its IPC socket before connecting. `serviceManager.state == .running`
    /// only means SMAppService accepted the LaunchAgent registration — the daemon
    /// process still needs a moment to start and create the socket file. Connecting
    /// before that file exists throws `IPCError.socket(code: ENOENT)` immediately,
    /// which used to surface as a connection failure on every launch.
    ///
    /// Not private so `ServiceUnavailableView`'s "Try Again" action can re-run
    /// this full sequence rather than only the SMAppService steps.
    @MainActor
    func launchServiceAndConnect() async {
        await serviceManager.ensureInstalled()
        await serviceManager.ensureUpToDate()
        await serviceManager.start()
        guard case .running = serviceManager.state else {
            // ServiceUnavailableView is rendered via the @Published state in VelvtMacApp.body
            return
        }

        let config: FocusAgentConfig
        do {
            #if DEBUG
            config = try EnvironmentConfigLoader().load()
            #else
            config = try BundleConfigLoader().load()
            #endif
        } catch {
            serviceManager.state = .failed(error)
            return
        }

        guard await waitForSocket(at: config.socketPath) else {
            serviceManager.state = .failed(AppLaunchError.socketNotReady(path: config.socketPath))
            return
        }

        startIPC(config: config)
    }

    /// Polls for the IPC socket file's existence, since the daemon creates it
    /// asynchronously after launchd starts the process. Returns `true` as soon
    /// as the file appears, `false` if it never does within `timeout`.
    private func waitForSocket(
        at path: String,
        timeout: TimeInterval = 5,
        pollInterval: TimeInterval = 0.25
    ) async -> Bool {
        let expandedPath = NSString(string: path).expandingTildeInPath
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if FileManager.default.fileExists(atPath: expandedPath) {
                return true
            }
            try? await Task.sleep(for: .seconds(pollInterval))
        }
        return FileManager.default.fileExists(atPath: expandedPath)
    }

    @MainActor
    private func startIPC(config: FocusAgentConfig) {
        let client: any IPCClientProtocol = UnixSocketIPCClient(
            socketPath: config.socketPath,
            protocolVersion: config.protocolVersion,
            clientVersion: config.clientVersion
        )
        ipcClient = client
        connectionStatusCancellable = client.connectionStatus
            .receive(on: RunLoop.main)
            .sink { [weak self] in self?.ipcConnectionStatus = $0 }

        // AccountStateManager is the sole consumer of incomingMessages.
        // It re-publishes to serverMessages for downstream consumers.
        accountStateManager.startListening(to: client)

        let displayCoord = ConcreteDisplayDataCoordinator()
        displayCoord.start(
            serverMessages: accountStateManager.serverMessages,
            connectionStatus: client.connectionStatus
        )
        displayCoordinator = displayCoord

        let relay = EventRelay(ipcClient: client)
        let collectionAgent = AXCollectionAgent(eventSink: relay)
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissionManager,
            collectionAgent: collectionAgent
        )
        self.eventRelay = relay
        self.collectionAgent = collectionAgent
        permissionCoordinator = coordinator
        coordinator.start()

        let menuBar = MenuBarController(
            presentation: permissionPresentation,
            displayCoordinator: displayCoord
        )
        menuBar.install()
        menuBar.observe(
            collectionStatus: collectionAgent.status,
            connectionStatus: client.connectionStatus,
            accountStateManager: accountStateManager
        )
        menuBarController = menuBar

        let scheduler = UNNotificationScheduler()
        let notificationCoordinator = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissionManager
        )
        notificationCoordinator.start(serverMessages: accountStateManager.serverMessages)
        notificationDeliveryCoordinator = notificationCoordinator

        let responseRouter = NotificationResponseRouter(
            openPopover: { [weak menuBar] in menuBar?.showPopover() },
            scrollToDate: displayCoord.historyViewModel.scrollToDateAction
        )
        UNUserNotificationCenter.current().delegate = responseRouter
        notificationResponseRouter = responseRouter

        Task { await relay.start() }

        Task.detached {
            do {
                try await client.connect()
            } catch let IPCError.versionMismatch(expected, got) {
                await MainActor.run {
                    let alert = NSAlert()
                    alert.alertStyle = .critical
                    alert.messageText = "Velvt update required"
                    alert.informativeText =
                        "IPC protocol version \(got) is incompatible with required version \(expected)."
                    alert.runModal()
                }
            } catch {
                // The IPC client owns retry behavior for transport failures.
            }
        }
    }

    public func applicationWillTerminate(_ notification: Notification) {
        permissionManager.stopMonitoring()
        permissionCoordinator?.stop()
        accountStateManager.stopListening()
        menuBarController?.remove()
        let relay = eventRelay
        Task { await relay?.stop() }
        ipcClient?.disconnect()
    }
}

/// FocusAgent executable entry point.
@main
public struct VelvtMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    public init() {}

    public var body: some Scene {
        WindowGroup {
            if case .failed = appDelegate.serviceManager.state {
                ServiceUnavailableView(serviceManager: appDelegate.serviceManager) {
                    await appDelegate.launchServiceAndConnect()
                }
            } else if appDelegate.ipcConnectionStatus == .connected,
                      let ipcClient = appDelegate.ipcClient {
                PermissionRootView(
                    presentation: appDelegate.permissionPresentation,
                    permissionManager: appDelegate.permissionManager,
                    accountStateManager: appDelegate.accountStateManager,
                    ipcClient: ipcClient
                )
            } else {
                ConnectingView()
            }
        }
    }
}

private struct ConnectingView: View {
    var body: some View {
        VStack(spacing: 12) {
            ProgressView()
            Text("Connecting to Velvt service…")
                .foregroundStyle(.secondary)
        }
        .padding(32)
        .frame(minWidth: 320)
    }
}
