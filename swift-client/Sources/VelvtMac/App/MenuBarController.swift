import AppKit
import Combine
import SwiftUI

// MARK: - MenuBarState

/// The single derived state driving the menu bar icon.
///
/// Precedence when multiple underlying signals are simultaneously true:
/// `deviceRevoked` (most severe, terminal) > `ipcDisconnected` (most
/// actionable) > `collectionPaused` > `normal`.
public enum MenuBarState: Equatable, Sendable, CaseIterable {
    case normal
    case collectionPaused
    case ipcDisconnected
    case deviceRevoked
}

// MARK: - MenuBarStateResolver

/// Combines `CollectionStatus`, `ConnectionStatus`, and account state into a
/// single `MenuBarState`. Pure — no side effects, no stored state.
public struct MenuBarStateResolver {
    public init() {}

    public func resolve(
        collectionStatus: CollectionStatus,
        connectionStatus: ConnectionStatus,
        accountState: AccountState,
        isDeviceRevoked: Bool
    ) -> MenuBarState {
        if isDeviceRevoked {
            return .deviceRevoked
        }
        if connectionStatus != .connected {
            return .ipcDisconnected
        }
        if collectionStatus == .permissionRevoked {
            return .collectionPaused
        }
        return .normal
    }
}

// MARK: - MenuBarStateStream

/// Derives a stream of resolved `MenuBarState` values from the three
/// independent status sources.
///
/// `CombineLatest4` re-emits on *every* upstream emission, using the latest
/// cached value from the other three publishers. When two of the four
/// sources change as part of the same logical event but arrive as separate
/// synchronous `@Published` writes (or otherwise in quick succession), the
/// first of those two emissions briefly combines a stale value with a fresh
/// one — exactly the kind of compound update `AccountStateManager` performs
/// when handling `device_revoked` (`accountState` and `isDeviceRevoked` are
/// set in two separate statements). Debouncing the *resolved* state — rather
/// than any one input — coalesces that burst into a single, correct,
/// settled emission instead of letting a transient incorrect `MenuBarState`
/// reach the icon. `debounceInterval` defaults to a value imperceptible for
/// a status icon but is injectable so tests can use a much shorter window.
@MainActor
enum MenuBarStateStream {
    static func make(
        resolver: MenuBarStateResolver,
        collectionStatus: some Publisher<CollectionStatus, Never>,
        connectionStatus: some Publisher<ConnectionStatus, Never>,
        accountStateManager: AccountStateManager,
        debounceInterval: RunLoop.SchedulerTimeType.Stride = .milliseconds(50)
    ) -> AnyPublisher<MenuBarState, Never> {
        Publishers.CombineLatest4(
            collectionStatus,
            connectionStatus,
            accountStateManager.$accountState,
            accountStateManager.$isDeviceRevoked
        )
        .map { collection, connection, account, isRevoked in
            resolver.resolve(
                collectionStatus: collection,
                connectionStatus: connection,
                accountState: account,
                isDeviceRevoked: isRevoked
            )
        }
        .debounce(for: debounceInterval, scheduler: RunLoop.main)
        .removeDuplicates()
        .eraseToAnyPublisher()
    }
}

// MARK: - MenuBarIconProvider

/// Maps a `MenuBarState` to an SF Symbol name and accessibility description.
/// Kept free of `NSImage` so the mapping itself is testable without AppKit.
enum MenuBarIconProvider {
    static func symbolName(for state: MenuBarState) -> String {
        switch state {
        case .normal: return "circle.fill"
        case .collectionPaused: return "pause.circle"
        case .ipcDisconnected: return "wifi.slash"
        case .deviceRevoked: return "exclamationmark.triangle.fill"
        }
    }

    static func accessibilityDescription(for state: MenuBarState) -> String {
        switch state {
        case .normal: return "Velvt"
        case .collectionPaused: return "Velvt — collection paused"
        case .ipcDisconnected: return "Velvt — service disconnected"
        case .deviceRevoked: return "Velvt — device revoked"
        }
    }
}

@MainActor
public protocol PopoverPresenting: AnyObject {
    var behavior: NSPopover.Behavior { get set }
    var animates: Bool { get set }
    var contentViewController: NSViewController? { get set }
    var contentSize: NSSize { get set }
    var isShown: Bool { get }

    func show(relativeTo positioningRect: NSRect, of positioningView: NSView, preferredEdge: NSRectEdge)
    func close()
}

extension NSPopover: PopoverPresenting {}

@MainActor
public protocol StatusItemManaging: AnyObject {
    var button: NSButton? { get }

    func install(target: AnyObject, action: Selector)
    func remove()
}

@MainActor
private final class SystemStatusItemManager: StatusItemManaging {
    private var statusItem: NSStatusItem?

    var button: NSButton? { statusItem?.button }

    func install(target: AnyObject, action: Selector) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.target = target
        item.button?.action = action
        statusItem = item
    }

    func remove() {
        if let statusItem {
            NSStatusBar.system.removeStatusItem(statusItem)
        }
        statusItem = nil
    }
}

// MARK: - MenuBarController

/// Owns the `NSStatusItem` and its `NSPopover`. This is the only type in the
/// app that creates or touches an `NSStatusItem` directly.
///
/// All AppKit interaction here is `@MainActor`; `NSStatusItem` is created in
/// `install()`, which must be called from the main thread (every
/// `NSApplicationDelegate` lifecycle callback already runs there).
@MainActor
public final class MenuBarController: NSObject {
    private let resolver = MenuBarStateResolver()
    private let popover: any PopoverPresenting
    private let popoverWillOpen = CurrentValueSubject<Void, Never>(())
    private let activateApp: () -> Void
    private let terminateApp: () -> Void
    private let serviceConnectionStatus: ServiceConnectionStatusModel
    private let collectionActivityStatus: CollectionActivityStatusModel
    private let currentActivity: CurrentActivityModel
    private let serviceAlertModel: ServiceAlertModel
    private let collectionSettings: CollectionSettingsModel
    private let metricsStore: AppMetricsStore
    private let guidedTour = GuidedTourModel()
    private let statusItemManager: any StatusItemManaging
    private var cancellables = Set<AnyCancellable>()

    /// Whether the popover is currently shown. Exposed for tests; production
    /// callers should use `togglePopover()`/`showPopover()`/`closePopover()`.
    public var isPopoverShown: Bool { popover.isShown }

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)? = nil,
        displayCoordinator: ConcreteDisplayDataCoordinator,
        accountStateManager: AccountStateManager? = nil,
        ipcClient: (any IPCClientProtocol)? = nil,
        menuStatusViewModel: MenuStatusViewModel? = nil,
        metricsStore: AppMetricsStore = AppMetricsStore(defaults: UserDefaults(suiteName: "MenuBarController.preview") ?? .standard),
        currentActivity: CurrentActivityModel = CurrentActivityModel(),
        serviceAlertModel: ServiceAlertModel? = nil,
        collectionSettings: CollectionSettingsModel = CollectionSettingsModel(),
        workBlockCoordinator: WorkBlockCoordinator? = nil,
        localDashboardCoordinator: LocalDashboardCoordinator? = nil,
        collectionStatus: AnyPublisher<CollectionStatus, Never> = Just(.idle).eraseToAnyPublisher(),
        connectionStatus: AnyPublisher<ConnectionStatus, Never> = Just(.disconnected).eraseToAnyPublisher(),
        simulateNotification: (() async -> DebugInsightSimulationResult)? = nil,
        restartLocalService: (() -> Void)? = nil,
        replayOnboarding: (() -> Void)? = nil,
        startGuidedTour: (() -> Void)? = nil,
        updateController: AppUpdateController? = nil,
        popover: (any PopoverPresenting)? = nil,
        statusItemManager: (any StatusItemManaging)? = nil,
        activateApp: @escaping @MainActor () -> Void = {
            NSApp.unhide(nil)
            NSApp.activate(ignoringOtherApps: true)
        },
        terminateApp: @escaping @MainActor () -> Void = { NSApp.terminate(nil) }
    ) {
        let serviceConnectionStatus = ServiceConnectionStatusModel(connectionStatus: connectionStatus)
        let updateController = updateController ?? .disabled()
        let collectionActivityStatus = CollectionActivityStatusModel(collectionStatus: collectionStatus)
        let serviceAlertModel = serviceAlertModel ?? ServiceAlertModel(messages: Empty<ServerMessage, Never>())
        self.serviceConnectionStatus = serviceConnectionStatus
        self.collectionActivityStatus = collectionActivityStatus
        self.currentActivity = currentActivity
        self.serviceAlertModel = serviceAlertModel
        self.collectionSettings = collectionSettings
        self.metricsStore = metricsStore
        self.popover = popover ?? NSPopover()
        self.statusItemManager = statusItemManager ?? SystemStatusItemManager()
        self.activateApp = activateApp
        self.terminateApp = terminateApp
        super.init()
        self.popover.behavior = .transient
        // Disabling the fade keeps show/close synchronous (isShown flips
        // immediately), which both reads more like a snappy status-item
        // utility and avoids a test-only race on the animation completion.
        self.popover.animates = false
        let hostingController = NSHostingController(
            rootView: MenuBarPopoverView(
                presentation: presentation,
                permissionManager: permissionManager,
                coordinator: displayCoordinator,
                serviceConnectionStatus: serviceConnectionStatus,
                collectionActivityStatus: collectionActivityStatus,
                currentActivity: currentActivity,
                serviceAlertModel: serviceAlertModel,
                collectionSettings: collectionSettings,
                workBlockCoordinator: workBlockCoordinator,
                localDashboardCoordinator: localDashboardCoordinator,
                accountStateManager: accountStateManager,
                ipcClient: ipcClient,
                menuStatusViewModel: menuStatusViewModel,
                simulateNotification: simulateNotification,
                restartLocalService: restartLocalService,
                replayOnboarding: replayOnboarding,
                startGuidedTour: startGuidedTour,
                updateController: updateController,
                guidedTour: guidedTour,
                metricsStore: metricsStore,
                popoverWillOpen: popoverWillOpen.eraseToAnyPublisher(),
                onEscape: { [weak self] in self?.closePopover() },
                onTerminate: { [weak self] in self?.terminateApp() }
            )
        )
        // The popover owns its explicit, screen-clamped size. Without this,
        // NSHostingController can replace it with SwiftUI's flexible fitting
        // size after presentation and expand the popover to the screen height.
        hostingController.sizingOptions = []
        self.popover.contentViewController = hostingController
        self.popover.contentSize = MenuBarPopoverLayout.preferredContentSize
        guidedTour.$isPresented
            .removeDuplicates()
            .dropFirst()
            .sink { [weak self] isPresented in
                self?.updatePopoverSize(includesWalkthrough: isPresented)
            }
            .store(in: &cancellables)
    }

    // MARK: Lifecycle

    /// Creates the `NSStatusItem` and sets the initial (`.normal`) icon.
    /// The executable entry point owns the application's activation policy;
    /// keeping that process-wide lifecycle decision out of this controller
    /// also lets the complete XCTest bundle finish normally.
    public func install() {
        statusItemManager.install(target: self, action: #selector(handleStatusItemClick))
        applyIcon(for: .normal)
    }

    /// Removes the status item. Safe to call multiple times.
    public func remove() {
        statusItemManager.remove()
    }

    // MARK: State observation

    /// Subscribes to the three independent status sources and derives a
    /// single `MenuBarState` via `MenuBarStateResolver` on every change.
    public func observe(
        collectionStatus: some Publisher<CollectionStatus, Never>,
        connectionStatus _: some Publisher<ConnectionStatus, Never>,
        accountStateManager: AccountStateManager
    ) {
        let stableConnectionStatus = serviceConnectionStatus.$phase
            .map { phase -> ConnectionStatus in
                switch phase {
                case .connected, .waking:
                    return .connected
                case .starting:
                    return .connecting
                case .unavailable:
                    return .disconnected
                }
            }
            .eraseToAnyPublisher()
        MenuBarStateStream.make(
            resolver: resolver,
            collectionStatus: collectionStatus,
            connectionStatus: stableConnectionStatus,
            accountStateManager: accountStateManager
        )
        .sink { [weak self] state in
            self?.applyIcon(for: state)
        }
        .store(in: &cancellables)
    }

    // MARK: Popover

    public func togglePopover() {
        if popover.isShown {
            closePopover()
        } else {
            showPopover()
        }
    }

    /// Shows the popover, activating the app first so it opens correctly
    /// even if the app is currently hidden (e.g. via Cmd+H or a notification
    /// tap arriving while backgrounded).
    public func showPopover() {
        guard let button = statusItemManager.button, !popover.isShown else { return }
        popoverWillOpen.send()
        popover.contentSize = MenuBarPopoverLayout.contentSize(
            for: button.window?.screen?.visibleFrame ?? NSScreen.main?.visibleFrame,
            includesWalkthrough: guidedTour.isPresented
        )
        activateApp()
        let positioningRect = button.bounds.offsetBy(dx: -16, dy: 0)
        popover.show(relativeTo: positioningRect, of: button, preferredEdge: .minY)
    }

    public func closePopover() {
        popover.close()
    }

    public func showToday() {
        guidedTour.dismiss()
        showPopover()
    }

    public func beginGuidedTour() {
        showPopover()
        guidedTour.start()
    }

    // MARK: Private

    @objc private func handleStatusItemClick() {
        togglePopover()
    }

    /// The four SF Symbols used here (`circle.fill`, `pause.circle`,
    /// `wifi.slash`, `exclamationmark.triangle.fill`) each report a
    /// different natural glyph width. Left at their default size, the
    /// status item visibly shifts left/right in the menu bar every time the
    /// icon switches between them. Rendering every symbol at the same fixed
    /// point size pins them to a consistent canvas so the icon's apparent
    /// position never moves, only its glyph.
    private static let iconConfiguration = NSImage.SymbolConfiguration(pointSize: 14, weight: .regular)

    private func updatePopoverSize(includesWalkthrough: Bool) {
        let visibleFrame = statusItemManager.button?.window?.screen?.visibleFrame
            ?? NSScreen.main?.visibleFrame
        popover.contentSize = MenuBarPopoverLayout.contentSize(
            for: visibleFrame,
            includesWalkthrough: includesWalkthrough
        )
    }

    private func applyIcon(for state: MenuBarState) {
        let description = MenuBarIconProvider.accessibilityDescription(for: state)
        statusItemManager.button?.image = NSImage(named: "VelvtMenuBarIcon")
        statusItemManager.button?.image?.isTemplate = true
        statusItemManager.button?.toolTip = description
    }
}
