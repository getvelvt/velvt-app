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
@MainActor
public final class AppDelegate: NSObject, NSApplicationDelegate {
    public let permissionManager: PermissionManager
    public let permissionPresentation: PermissionPresentationModel
    public let accountStateManager: AccountStateManager
    public private(set) var displayCoordinator: ConcreteDisplayDataCoordinator?

    let ipcClient: any IPCClientProtocol
    private var eventRelay: (any EventRelayProtocol)?
    private var eventSinkFanout: EventSinkFanout?
    private var collectionAgent: (any CollectionAgentProtocol)?
    private var permissionCoordinator: PermissionCollectionCoordinator?
    private var menuBarController: MenuBarController?
    private var notificationDeliveryCoordinator: NotificationDeliveryCoordinator?
    private var notificationResponseRouter: NotificationResponseRouter?
    private var menuBarDataLoader: MenuBarDataLoader?
    private var menuStatusViewModel: MenuStatusViewModel?
    private var collectionAuthCancellable: AnyCancellable?
    private var authGatedCollectionController: AuthGatedCollectionController?
    private let metricsStore = AppMetricsStore()
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
        // Starts the bundled Rust helper (Contents/Resources/velvt-service) when
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
            connectionStatus: client.connectionStatus,
            accountState: accountStateManager.$accountState.eraseToAnyPublisher()
        )
        displayCoordinator = displayCoord

        let dataLoader = MenuBarDataLoader(ipcClient: client)
        dataLoader.start(accountState: accountStateManager.$accountState.eraseToAnyPublisher())
        menuBarDataLoader = dataLoader

        let statusViewModel = MenuStatusViewModel(ipcClient: client, messages: accountStateManager.serverMessages)
        statusViewModel.start()
        menuStatusViewModel = statusViewModel
        let serviceAlertModel = ServiceAlertModel(messages: accountStateManager.serverMessages)

        let relay = EventRelay(ipcClient: client, metrics: metricsStore)
        let currentActivity = CurrentActivityModel()
        let collectionSettings = CollectionSettingsModel()
        let eventSinkFanout = EventSinkFanout([relay, currentActivity])
        let collectionAgent = AXCollectionAgent(eventSink: eventSinkFanout)
        let coordinator = PermissionCollectionCoordinator(
            permissionManager: permissionManager,
            collectionAgent: collectionAgent,
            connectionStatus: client.connectionStatus,
            collectionSettings: collectionSettings
        )
        self.eventRelay = relay
        self.eventSinkFanout = eventSinkFanout
        self.collectionAgent = collectionAgent
        permissionCoordinator = coordinator

        let scheduler = UNNotificationScheduler(metrics: metricsStore)
        let notificationCoordinator = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissionManager
        )
        notificationCoordinator.start(serverMessages: accountStateManager.serverMessages)
        notificationDeliveryCoordinator = notificationCoordinator

        let menuBar = MenuBarController(
            presentation: permissionPresentation,
            permissionManager: permissionManager,
            displayCoordinator: displayCoord,
            accountStateManager: accountStateManager,
            ipcClient: client,
            menuStatusViewModel: statusViewModel,
            metricsStore: metricsStore,
            currentActivity: currentActivity,
            serviceAlertModel: serviceAlertModel,
            collectionSettings: collectionSettings,
            collectionStatus: collectionAgent.status,
            connectionStatus: client.connectionStatus,
            simulateNotification: {
                _ = notificationCoordinator.simulateDebugInsightReceipt()
            },
            terminateApp: { NSApp.terminate(nil) }
        )
        menuBar.install()
        menuBar.observe(
            collectionStatus: collectionAgent.status,
            connectionStatus: client.connectionStatus,
            accountStateManager: accountStateManager
        )
        menuBarController = menuBar

        let responseRouter = NotificationResponseRouter(
            openPopover: { [weak menuBar] in menuBar?.showPopover() },
            scrollToDate: displayCoord.historyViewModel.scrollToDateAction
        )
        UNUserNotificationCenter.current().delegate = responseRouter
        notificationResponseRouter = responseRouter

        let collectionController = AuthGatedCollectionController(
            startCollection: {
                Task { @MainActor in
                    await relay.start()
                    coordinator.start()
                }
            },
            stopCollection: {
                coordinator.stop()
                Task { await relay.stop() }
            }
        )
        authGatedCollectionController = collectionController
        collectionController.apply(accountState: accountStateManager.accountState)
        collectionAuthCancellable = accountStateManager.$accountState
            .dropFirst()
            .sink { [weak collectionController] state in
                collectionController?.apply(accountState: state)
            }

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
        authGatedCollectionController?.stop()
        collectionAuthCancellable?.cancel()
        accountStateManager.stopListening()
        menuBarController?.remove()
        let relay = eventRelay
        Task { await relay?.stop() }
        ipcClient.disconnect()
        serviceProcessLauncher.stop()
    }

    private static func makeIPCClient() throws -> any IPCClientProtocol {
        let config: FocusAgentConfig
        #if DEBUG
        do {
            config = try BundleConfigLoader().load()
        } catch {
            config = try EnvironmentConfigLoader().load()
        }
        #else
        config = try BundleConfigLoader().load()
        #endif
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

/// Menu-bar-only executable entry point. No `WindowGroup` is created: every
/// user-facing route is hosted by `MenuBarController`'s popover.
@main
public enum VelvtMacApp {
    public static func main() {
        let application = NSApplication.shared
        let delegate = AppDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.accessory)
        application.run()
    }
}
