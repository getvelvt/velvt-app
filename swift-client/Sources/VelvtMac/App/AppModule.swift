import AppKit
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

/// AppKit delegate used by the SwiftUI application entry point.
public final class AppDelegate: NSObject, NSApplicationDelegate {
    public let permissionManager: PermissionManager
    public let permissionPresentation: PermissionPresentationModel
    public let accountStateManager: AccountStateManager
    public private(set) var displayCoordinator: ConcreteDisplayDataCoordinator?

    var ipcClient: (any IPCClientProtocol)?
    private var eventRelay: (any EventRelayProtocol)?
    private var collectionAgent: (any CollectionAgentProtocol)?
    private var permissionCoordinator: PermissionCollectionCoordinator?
    private var menuBarController: MenuBarController?
    private var notificationDeliveryCoordinator: NotificationDeliveryCoordinator?
    private var notificationResponseRouter: NotificationResponseRouter?
    let serviceManager = ServiceManager()

    public override init() {
        let permissionManager = PermissionManager()
        self.permissionManager = permissionManager
        permissionPresentation = PermissionPresentationModel(
            permissionManager: permissionManager,
            onboardingStateStore: UserDefaultsOnboardingStateStore()
        )
        accountStateManager = AccountStateManager(keychain: KeychainService())
        super.init()
    }

    public func applicationDidFinishLaunching(_ notification: Notification) {
        permissionManager.startMonitoring()
        Task {
            _ = await permissionManager.checkStatus(for: .accessibility)
            _ = await permissionManager.checkStatus(for: .notifications)
        }

        Task { @MainActor in
            await serviceManager.ensureInstalled()
            await serviceManager.ensureUpToDate()
            await serviceManager.start()
            guard case .running = serviceManager.state else {
                // ServiceUnavailableView is rendered via the @Published state in VelvtMacApp.body
                return
            }
            startIPC()
        }
    }

    @MainActor
    private func startIPC() {
        do {
            #if DEBUG
            let config = try EnvironmentConfigLoader().load()
            #else
            let config = try BundleConfigLoader().load()
            #endif
            let client: any IPCClientProtocol = UnixSocketIPCClient(
                socketPath: config.socketPath,
                protocolVersion: config.protocolVersion,
                clientVersion: config.clientVersion
            )
            ipcClient = client

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
        } catch {
            ipcClient = nil
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
                ServiceUnavailableView(serviceManager: appDelegate.serviceManager)
            } else {
                PermissionRootView(
                    presentation: appDelegate.permissionPresentation,
                    permissionManager: appDelegate.permissionManager,
                    accountStateManager: appDelegate.accountStateManager,
                    ipcClient: appDelegate.ipcClient ?? FakeIPCClient()
                )
            }
        }
    }
}
