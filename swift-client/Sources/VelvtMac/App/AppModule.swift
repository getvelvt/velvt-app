import AppKit
import Combine
import SwiftUI
import UserNotifications

/// App module - owns application lifecycle and menu bar setup.
/// Does NOT own event capture, IPC processing, abstraction, cloud calls, or
/// insight generation.

/// Coordinates startup and shutdown of the application.
public protocol AppLifecycleManaging: AnyObject {
    /// Starts application-owned services.
    func start() async throws

    /// Stops application-owned services.
    func stop() async
}

/// AppKit delegate used by the SwiftUI application entry point.
public final class AppDelegate: NSObject, NSApplicationDelegate {
    public let permissionManager: PermissionManager
    public let permissionPresentation: PermissionPresentationModel
    public let accountStateManager: AccountStateManager
    public private(set) var displayCoordinator: ConcreteDisplayDataCoordinator?

    let ipcClient: any IPCClientProtocol
    private var eventRelay: (any EventRelayProtocol)?
    private var collectionAgent: (any CollectionAgentProtocol)?
    private var permissionCoordinator: PermissionCollectionCoordinator?
    private var menuBarController: MenuBarController?
    private var notificationDeliveryCoordinator: NotificationDeliveryCoordinator?
    private var notificationResponseRouter: NotificationResponseRouter?
    private let serviceProcessLauncher = ServiceProcessLauncher()

    public override convenience init() {
        self.init(
            permissionManager: PermissionManager(),
            accountStateManager: AccountStateManager(keychain: KeychainService()),
            ipcClientFactory: Self.makeIPCClient
        )
    }

    init(
        permissionManager: PermissionManager,
        accountStateManager: AccountStateManager,
        ipcClientFactory: @escaping () throws -> any IPCClientProtocol
    ) {
        self.permissionManager = permissionManager
        permissionPresentation = PermissionPresentationModel(
            permissionManager: permissionManager,
            onboardingStateStore: UserDefaultsOnboardingStateStore()
        )
        self.accountStateManager = accountStateManager
        ipcClient = (try? ipcClientFactory()) ?? UnavailableIPCClient()
        super.init()
    }

    public func applicationDidFinishLaunching(_ notification: Notification) {
        // Starts the bundled Rust helper (Contents/MacOS/velvt-service) when
        // running as a packaged .app; a no-op under `swift run`, where the
        // service is started separately per README development instructions.
        serviceProcessLauncher.start()

        permissionManager.startMonitoring()
        Task {
            _ = await permissionManager.checkStatus(for: .accessibility)
            _ = await permissionManager.checkStatus(for: .notifications)
        }

        let client = ipcClient

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
            displayCoordinator: displayCoord,
            connectionStatus: client.connectionStatus
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
                    alert.informativeText = "IPC protocol version \(got) is incompatible with required version \(expected)."
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
        ipcClient.disconnect()
        serviceProcessLauncher.stop()
    }

    private static func makeIPCClient() throws -> any IPCClientProtocol {
        let config = try EnvironmentConfigLoader().load()
        return UnixSocketIPCClient(
            socketPath: config.socketPath,
            protocolVersion: config.protocolVersion,
            clientVersion: config.clientVersion
        )
    }
}

private final class UnavailableIPCClient: IPCClientProtocol {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { $0.finish() }

    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        Just(.disconnected).eraseToAnyPublisher()
    }

    func connect() async throws {
        throw IPCError.notConnected
    }

    func disconnect() {}

    func send(_ message: ClientMessage) async throws {
        throw IPCError.notConnected
    }
}

/// FocusAgent executable entry point.
@main
public struct VelvtMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    public init() {}

    public var body: some Scene {
        WindowGroup {
            PermissionRootView(
                presentation: appDelegate.permissionPresentation,
                permissionManager: appDelegate.permissionManager,
                accountStateManager: appDelegate.accountStateManager,
                ipcClient: appDelegate.ipcClient
            )
        }
    }
}
