import AppKit
import Combine
import Darwin
import SwiftUI
import UserNotifications
import os

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
    public let updateController: AppUpdateController
    public private(set) var displayCoordinator: ConcreteDisplayDataCoordinator?

    let ipcClient: any IPCClientProtocol
    private var eventRelay: (any EventRelayProtocol)?
    private var eventSinkFanout: EventSinkFanout?
    private var collectionAgent: (any CollectionAgentProtocol)?
    private var permissionCoordinator: PermissionCollectionCoordinator?
    private var menuBarController: MenuBarController?
    private var onboardingWindowController: OnboardingWindowController?
    private var notificationDeliveryCoordinator: NotificationDeliveryCoordinator?
    private var interventionNotifier: InterventionNotifier?
    private var notificationResponseRouter: NotificationResponseRouter?
    private var menuBarDataLoader: MenuBarDataLoader?
    private var menuStatusViewModel: MenuStatusViewModel?
    private var workBlockCoordinator: WorkBlockCoordinator?
    private var focusStateObserver: FocusStateObserver?
    private var localDashboardCoordinator: LocalDashboardCoordinator?
    private var accountMetricsCancellable: AnyCancellable?
    private let metricsStore = AppMetricsStore()
    private let serviceProcessLauncher = ServiceProcessLauncher()

    public override convenience init() {
        self.init(
            permissionManager: PermissionManager(),
            accountStateManager: AccountStateManager(keychain: KeychainService()),
            updateController: .live(),
            ipcClientFactory: Self.makeIPCClient
        )
    }

    init(
        permissionManager: PermissionManager,
        accountStateManager: AccountStateManager,
        updateController: AppUpdateController? = nil,
        ipcClientFactory: @escaping () throws -> any IPCClientProtocol
    ) {
        self.permissionManager = permissionManager
        permissionPresentation = PermissionPresentationModel(
            permissionManager: permissionManager,
            onboardingStateStore: UserDefaultsOnboardingStateStore()
        )
        self.accountStateManager = accountStateManager
        self.updateController = updateController ?? .disabled()
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
            // Read the current state without opening the macOS prompt. The
            // onboarding window owns the explicit Accessibility request after
            // the intro has been shown.
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
        let workBlocks = WorkBlockCoordinator(ipcClient: client)
        workBlocks.start(
            messages: accountStateManager.serverMessages,
            connectionStatus: client.connectionStatus
        )
        workBlockCoordinator = workBlocks
        // Swift observes system Focus/DND coarsely and reports transitions;
        // the Rust service owns the evidence record and every decision.
        let focusObserver = FocusStateObserver(ipcClient: client)
        focusObserver.start(connectionStatus: client.connectionStatus)
        focusStateObserver = focusObserver
        let localDashboard = LocalDashboardCoordinator(ipcClient: client)
        localDashboard.start(
            messages: accountStateManager.serverMessages,
            connectionStatus: client.connectionStatus
        )
        localDashboardCoordinator = localDashboard

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
        // One line on the unified log per delivery attempt, on both surfaces.
        // A notification that is never posted is otherwise invisible from
        // inside the app, which is how an installation can go its whole life
        // without delivering anything and still look healthy.
        let deliveryReporter = OSLogNotificationDeliveryReporter()
        let notificationCoordinator = NotificationDeliveryCoordinator(
            scheduler: scheduler,
            permissionManager: permissionManager,
            reporter: deliveryReporter
        )
        notificationCoordinator.start(serverMessages: accountStateManager.serverMessages)
        notificationDeliveryCoordinator = notificationCoordinator

        // A drift offer renders as an in-app card, which is a surface the
        // person is not looking at when they have drifted. This carries the
        // same offer to the notification centre.
        let interventionNotifier = InterventionNotifier(
            scheduler: scheduler,
            permissionManager: permissionManager,
            reporter: deliveryReporter
        )
        interventionNotifier.start(snapshots: workBlocks.$snapshot)
        self.interventionNotifier = interventionNotifier

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
            workBlockCoordinator: workBlocks,
            localDashboardCoordinator: localDashboard,
            collectionStatus: collectionAgent.status,
            connectionStatus: client.connectionStatus,
            simulateNotification: {
                #if DEBUG
                    displayCoord.updateInsight(
                        InsightPayload(
                            date: HistoryViewModel.localDateString(),
                            text:
                                "Your simulated insight is working. This preview follows the same UI path as a delivered insight.",
                            confidenceLevel: .medium,
                            lowConfidence: false,
                            generatedAt: Date()
                        )
                    )
                #endif
                return await notificationCoordinator.simulateDebugInsightReceipt().value
            },
            restartLocalService: { [weak serviceProcessLauncher] in
                serviceProcessLauncher?.restart()
                // A restarted helper does not bring the socket back on its own.
                // A versionMismatch handshake leaves the client disconnected
                // with no reconnect armed, and this is the only other place in
                // the app that re-dials, so it has to do both.
                Task.detached { try? await client.connect() }
            },
            replayOnboarding: { [weak self] in
                self?.onboardingWindowController?.presentReplay()
            },
            startGuidedTour: { [weak self] in
                self?.menuBarController?.beginGuidedTour()
            },
            updateController: updateController,
            terminateApp: { NSApp.terminate(nil) }
        )
        menuBar.install()
        menuBar.observe(
            collectionStatus: collectionAgent.status,
            connectionStatus: client.connectionStatus,
            accountStateManager: accountStateManager
        )
        menuBarController = menuBar

        let onboardingWindow = OnboardingWindowController(
            presentation: permissionPresentation,
            permissionManager: permissionManager,
            accountStateManager: accountStateManager,
            ipcClient: client,
            onStartUsing: { [weak menuBar] in menuBar?.showToday() },
            onStartTour: { [weak menuBar] in menuBar?.beginGuidedTour() }
        )
        onboardingWindowController = onboardingWindow
        onboardingWindow.presentOnLaunch()

        let responseRouter = NotificationResponseRouter(
            openPopover: { [weak menuBar] in menuBar?.showPopover() },
            scrollToDate: displayCoord.historyViewModel.scrollToDateAction
        )
        UNUserNotificationCenter.current().delegate = responseRouter
        notificationResponseRouter = responseRouter

        // Local collection is permission- and service-gated, not account-gated.
        // The Rust service remains responsible for keeping unauthenticated data
        // local and for enabling cloud synchronization only after authentication.
        Task { await relay.start() }
        coordinator.start()
        metricsStore.setAuthenticated(Self.isLoggedIn(accountStateManager.accountState))
        accountMetricsCancellable = accountStateManager.$accountState
            .dropFirst()
            .sink { [metricsStore] state in
                metricsStore.setAuthenticated(Self.isLoggedIn(state))
            }

        // A version mismatch on the very first dial is the upgrade path, not an
        // exotic failure: a helper from the previous install is still holding
        // the socket and still speaking the protocol version it shipped with.
        // Recovery is automatic and silent when this app can positively
        // identify that helper as its own; the alert is the fallback for when
        // it cannot.
        let reaper = OrphanedHelperReaper.live(
            helperExecutablePath: serviceProcessLauncher.bundledServiceURL()?.path
        )
        let launcher = serviceProcessLauncher
        Task.detached {
            await AppDelegate.connectRetryingVersionMismatch(
                client,
                reclaimOrphanedHelper: {
                    guard await reaper.reclaimSocket() else { return false }
                    // The socket is free, and this app's own helper exited on
                    // `duplicate_service_instance` when it found it taken.
                    // Nothing else relaunches it, so the recovery is only half
                    // done until it is asked to start again.
                    await MainActor.run { launcher.restart() }
                    await reaper.waitForRelaunchedHelper()
                    return true
                }
            ) { expected, got in
                await MainActor.run {
                    let alert = NSAlert()
                    alert.alertStyle = .warning
                    alert.messageText = "Velvt can't reach its background service"
                    alert.informativeText = """
                        An older Velvt background service is still holding the \
                        connection on this Mac. It speaks protocol version \(got); \
                        this version of Velvt needs version \(expected).

                        Retry gives it another moment to exit. Restarting the Mac \
                        clears it for good.
                        """
                    alert.addButton(withTitle: "Retry")
                    alert.addButton(withTitle: "Close")
                    return alert.runModal() == .alertFirstButtonReturn
                }
            }
        }
    }

    /// How many times a version mismatch may be answered by reclaiming the
    /// socket before the person is told instead.
    ///
    /// A retry loop that can never succeed is the deadlock this exists to end,
    /// so the automatic path is deliberately finite: one reclaim covers the
    /// single orphan this can actually happen with, the second covers a helper
    /// that was mid-relaunch, and after that the honest answer is the alert.
    nonisolated static let maximumOrphanReclaimAttempts = 2

    /// Dials the IPC socket, reclaiming it from an orphaned helper when that is
    /// what is in the way, and re-dialling for as long as the person asks it to.
    ///
    /// `versionMismatch` is the one `IPCError` the client does not arm a
    /// reconnect for, and `connect()` has a single call site, so without an
    /// explicit re-dial the alert is where the app's IPC life ends until it is
    /// relaunched. The cause is an orphaned helper from an earlier install or a
    /// crashed prior run still holding the socket and answering the handshake
    /// with its own protocol version — which no amount of re-dialling can
    /// change. Retrying only helps once that process is gone, so this asks for
    /// it to go, rather than asking the person to find it in a terminal.
    ///
    /// Transport failures are still left alone: the client arms its own backoff
    /// reconnect for those, which is what carries a freshly relaunched helper
    /// that has not finished binding the socket yet.
    nonisolated static func connectRetryingVersionMismatch(
        _ client: any IPCClientProtocol,
        reclaimOrphanedHelper: @Sendable () async -> Bool = { false },
        presentVersionMismatch: (_ expected: Int, _ got: Int) async -> Bool
    ) async {
        var reclaimAttempts = 0
        while true {
            do {
                try await client.connect()
                return
            } catch let IPCError.versionMismatch(expected, got) {
                if reclaimAttempts < maximumOrphanReclaimAttempts,
                    await reclaimOrphanedHelper()
                {
                    reclaimAttempts += 1
                    continue
                }
                guard await presentVersionMismatch(expected, got) else { return }
            } catch {
                // The IPC client owns retry behavior for transport failures.
                return
            }
        }
    }

    private static func isLoggedIn(_ state: AccountState) -> Bool {
        if case .loggedIn = state {
            return true
        }
        return false
    }

    public func applicationWillTerminate(_ notification: Notification) {
        permissionManager.stopMonitoring()
        permissionCoordinator?.stop()
        accountMetricsCancellable?.cancel()
        accountStateManager.stopListening()
        menuBarController?.remove()
        onboardingWindowController?.close()
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

// MARK: - Orphaned helper recovery

/// One running process, reduced to the four facts that decide whether this app
/// is responsible for it.
///
/// Swift gathers facts; the rule below is the only thing that judges them, and
/// it judges nothing else. None of this leaves the device: it is read from the
/// local process table to answer one local question.
struct RunningProcessFacts: Equatable, Sendable {
    let processIdentifier: pid_t
    let parentProcessIdentifier: pid_t
    /// Fully resolved executable path, as `proc_pidpath` reports it.
    let executablePath: String
    let userIdentifier: uid_t
}

/// Decides which processes this app may terminate. Pure, and deliberately
/// narrow.
///
/// Terminating a process is a real action taken on someone's machine, so the
/// identification is positive on every axis and never a name match: a file
/// called `velvt-service` in a Downloads folder, a copy of the helper from a
/// *different* install, a helper belonging to another user account, and the
/// helper this very app launched are all excluded by construction. What is left
/// is a process running the exact executable inside this app's own bundle, as
/// this user, that this app did not start — an orphan of an earlier run, which
/// is the one thing holding the socket shut.
///
/// The deliberate consequence: an orphan launched from a bundle at a *different*
/// path — the app was dragged to a new location between runs — is not matched,
/// and the person gets the alert instead. Declining to act on a process this app
/// cannot positively claim is the correct trade, not a gap to widen.
enum OrphanedHelperRule {
    static func terminableOrphans(
        in processes: [RunningProcessFacts],
        helperExecutablePath: String,
        currentUser: uid_t,
        ownProcessIdentifier: pid_t
    ) -> [pid_t] {
        guard !helperExecutablePath.isEmpty else { return [] }
        return
            processes
            .filter { process in
                process.executablePath == helperExecutablePath
                    && process.userIdentifier == currentUser
                    && process.processIdentifier > 1
                    && process.processIdentifier != ownProcessIdentifier
                    // The helper this run started is a direct child of this
                    // process. It is never the orphan, and killing it would
                    // turn a recoverable state into a broken one.
                    && process.parentProcessIdentifier != ownProcessIdentifier
            }
            .map(\.processIdentifier)
    }
}

/// Frees the IPC socket by asking an orphaned Velvt helper to quit.
///
/// Every side effect is injected so the rule and the sequence can be tested
/// without signalling anything real.
struct OrphanedHelperReaper: Sendable {
    /// The local process table, as facts.
    var runningProcesses: @Sendable () -> [RunningProcessFacts]
    /// The resolved path of the helper inside this app's own bundle, or nil
    /// when this build has no bundled helper (a `swift run` development run),
    /// in which case there is nothing this app is responsible for.
    var helperExecutablePath: @Sendable () -> String?
    var currentUser: @Sendable () -> uid_t
    var ownProcessIdentifier: @Sendable () -> pid_t
    /// `SIGTERM`, so the helper closes its database and socket on the way out.
    /// Returns whether the signal was delivered.
    var requestTermination: @Sendable (pid_t) -> Bool
    var isRunning: @Sendable (pid_t) -> Bool
    /// One polling interval while waiting for a signalled helper to exit.
    var waitOneInterval: @Sendable () async -> Void
    /// Long enough for a freshly relaunched helper to bind the socket. Only a
    /// courtesy: a dial that still lands early fails as a transport error, for
    /// which the IPC client arms its own backoff reconnect.
    var waitForRelaunch: @Sendable () async -> Void

    /// Returns whether the socket can now be expected to be free, meaning at
    /// least one orphan was found, signalled, and observed to exit.
    func reclaimSocket(pollAttempts: Int = 20) async -> Bool {
        guard let helperPath = helperExecutablePath() else { return false }
        let orphans = OrphanedHelperRule.terminableOrphans(
            in: runningProcesses(),
            helperExecutablePath: helperPath,
            currentUser: currentUser(),
            ownProcessIdentifier: ownProcessIdentifier()
        )
        guard !orphans.isEmpty else {
            OrphanedHelperLog.shared.info("No orphaned velvt-service helper owned by this app")
            return false
        }
        let signalled = orphans.filter { requestTermination($0) }
        guard !signalled.isEmpty else {
            OrphanedHelperLog.shared.error("Could not signal orphaned velvt-service helper")
            return false
        }
        OrphanedHelperLog.shared.info(
            "Asked \(signalled.count, privacy: .public) orphaned velvt-service helper(s) to quit"
        )
        for _ in 0..<pollAttempts {
            if signalled.allSatisfy({ !isRunning($0) }) {
                return true
            }
            await waitOneInterval()
        }
        let gone = signalled.allSatisfy { !isRunning($0) }
        if !gone {
            OrphanedHelperLog.shared.error("Orphaned velvt-service helper did not exit")
        }
        return gone
    }

    func waitForRelaunchedHelper() async {
        await waitForRelaunch()
    }

    static func live(helperExecutablePath: String?) -> OrphanedHelperReaper {
        // Resolved once, and compared as an exact string afterwards.
        // `proc_pidpath` reports resolved paths, so the app side has to be
        // resolved too or a `/tmp` -> `/private/tmp` style symlink would make
        // the same executable look like a different one.
        let resolvedPath = helperExecutablePath.map {
            URL(fileURLWithPath: $0).resolvingSymlinksInPath().path
        }
        return OrphanedHelperReaper(
            runningProcesses: { Self.localProcessTable() },
            helperExecutablePath: { resolvedPath },
            currentUser: { geteuid() },
            ownProcessIdentifier: { getpid() },
            requestTermination: { kill($0, SIGTERM) == 0 },
            isRunning: { kill($0, 0) == 0 },
            waitOneInterval: { try? await Task.sleep(nanoseconds: 100_000_000) },
            waitForRelaunch: { try? await Task.sleep(nanoseconds: 2_000_000_000) }
        )
    }

    /// Reads pid, parent pid, executable path and owning user for every process
    /// this user can see. Nothing is recorded, uploaded or kept: the list is
    /// filtered by `OrphanedHelperRule` and discarded.
    private static func localProcessTable() -> [RunningProcessFacts] {
        let pathCapacity = 4 * Int(PATH_MAX)
        let byteCount = proc_listpids(UInt32(PROC_ALL_PIDS), 0, nil, 0)
        guard byteCount > 0 else { return [] }
        // Headroom: processes can appear between sizing and reading.
        let capacity = Int(byteCount) / MemoryLayout<pid_t>.size + 64
        var pids = [pid_t](repeating: 0, count: capacity)
        let written = proc_listpids(
            UInt32(PROC_ALL_PIDS),
            0,
            &pids,
            Int32(capacity * MemoryLayout<pid_t>.size)
        )
        guard written > 0 else { return [] }
        var facts: [RunningProcessFacts] = []
        for pid in pids.prefix(Int(written) / MemoryLayout<pid_t>.size) where pid > 0 {
            var pathBuffer = [CChar](repeating: 0, count: pathCapacity)
            guard proc_pidpath(pid, &pathBuffer, UInt32(pathCapacity)) > 0 else { continue }
            var info = proc_bsdinfo()
            let infoSize = Int32(MemoryLayout<proc_bsdinfo>.size)
            guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, infoSize) == infoSize else {
                continue
            }
            facts.append(
                RunningProcessFacts(
                    processIdentifier: pid,
                    parentProcessIdentifier: pid_t(bitPattern: info.pbi_ppid),
                    executablePath: String(cString: pathBuffer),
                    userIdentifier: info.pbi_uid
                )
            )
        }
        return facts
    }
}

enum OrphanedHelperLog {
    static let shared = Logger(subsystem: "com.velvt.mac", category: "OrphanedHelperReaper")
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

/// Menu-bar executable entry point. The first-run intro uses one AppKit window;
/// normal product routes remain hosted by `MenuBarController`'s popover.
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
