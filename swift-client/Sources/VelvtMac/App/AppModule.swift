import AppKit
import SwiftUI

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

/// Creates and manages the menu bar status item.
public protocol StatusItemManaging: AnyObject {
    /// Installs the status item in the system menu bar.
    func install()

    /// Removes the status item from the system menu bar.
    func remove()
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

        do {
            let config = try EnvironmentConfigLoader().load()
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
        } catch {
            ipcClient = nil
        }
    }

    public func applicationWillTerminate(_ notification: Notification) {
        permissionManager.stopMonitoring()
        permissionCoordinator?.stop()
        accountStateManager.stopListening()
        let relay = eventRelay
        Task { await relay?.stop() }
        ipcClient?.disconnect()
    }
}

/// Concrete placeholder for menu bar status-item ownership.
public final class StatusItemController: StatusItemManaging {
    public init() {}

    public func install() {}
    public func remove() {}
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
                ipcClient: appDelegate.ipcClient ?? FakeIPCClient()
            )
        }
        MenuBarExtra {
            MenuBarView(
                presentation: appDelegate.permissionPresentation,
                displayCoordinator: appDelegate.displayCoordinator
            )
        } label: {
            PermissionMenuBarLabel(presentation: appDelegate.permissionPresentation)
        }
        .menuBarExtraStyle(.window)
    }
}
