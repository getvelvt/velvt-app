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
    private let popover: NSPopover
    private let popoverWillOpen = CurrentValueSubject<Void, Never>(())
    private let activateApp: () -> Void
    private let terminateApp: () -> Void
    private let serviceConnectionStatus: ServiceConnectionStatusModel
    private let collectionActivityStatus: CollectionActivityStatusModel
    private let currentActivity: CurrentActivityModel
    private let serviceAlertModel: ServiceAlertModel
    private let collectionSettings: CollectionSettingsModel
    private let metricsStore: AppMetricsStore
    private var statusItem: NSStatusItem?
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
        simulateNotification: (() -> Void)? = nil,
        activateApp: @escaping @MainActor () -> Void = {
            NSApp.unhide(nil)
            NSApp.activate(ignoringOtherApps: true)
        },
        terminateApp: @escaping @MainActor () -> Void = { NSApp.terminate(nil) }
    ) {
        let serviceConnectionStatus = ServiceConnectionStatusModel(connectionStatus: connectionStatus)
        let collectionActivityStatus = CollectionActivityStatusModel(collectionStatus: collectionStatus)
        let serviceAlertModel = serviceAlertModel ?? ServiceAlertModel(messages: Empty<ServerMessage, Never>())
        self.serviceConnectionStatus = serviceConnectionStatus
        self.collectionActivityStatus = collectionActivityStatus
        self.currentActivity = currentActivity
        self.serviceAlertModel = serviceAlertModel
        self.collectionSettings = collectionSettings
        self.metricsStore = metricsStore
        popover = NSPopover()
        self.activateApp = activateApp
        self.terminateApp = terminateApp
        super.init()
        popover.behavior = .transient
        // Disabling the fade keeps show/close synchronous (isShown flips
        // immediately), which both reads more like a snappy status-item
        // utility and avoids a test-only race on the animation completion.
        popover.animates = false
        popover.contentViewController = NSHostingController(
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
                metricsStore: metricsStore,
                popoverWillOpen: popoverWillOpen.eraseToAnyPublisher(),
                onEscape: { [weak self] in self?.closePopover() },
                onTerminate: { [weak self] in self?.terminateApp() }
            )
        )
        popover.contentSize = MenuBarPopoverLayout.preferredContentSize
    }

    // MARK: Lifecycle

    /// Creates the `NSStatusItem` and sets the initial (`.normal`) icon.
    /// Hides the Dock icon so the app behaves as a menu-bar-only accessory;
    /// this does not prevent the app from showing windows (e.g. onboarding)
    /// or from terminating normally.
    public func install() {
        NSApp.setActivationPolicy(.accessory)
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.target = self
        item.button?.action = #selector(handleStatusItemClick)
        statusItem = item
        applyIcon(for: .normal)
    }

    /// Removes the status item. Safe to call multiple times.
    public func remove() {
        if let statusItem {
            NSStatusBar.system.removeStatusItem(statusItem)
        }
        statusItem = nil
    }

    // MARK: State observation

    /// Subscribes to the three independent status sources and derives a
    /// single `MenuBarState` via `MenuBarStateResolver` on every change.
    public func observe(
        collectionStatus: some Publisher<CollectionStatus, Never>,
        connectionStatus: some Publisher<ConnectionStatus, Never>,
        accountStateManager: AccountStateManager
    ) {
        MenuBarStateStream.make(
            resolver: resolver,
            collectionStatus: collectionStatus,
            connectionStatus: connectionStatus,
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
        guard let button = statusItem?.button, !popover.isShown else { return }
        popoverWillOpen.send()
        popover.contentSize = MenuBarPopoverLayout.contentSize(
            for: button.window?.screen?.visibleFrame ?? NSScreen.main?.visibleFrame
        )
        activateApp()
        let positioningRect = button.bounds.offsetBy(dx: -16, dy: 0)
        popover.show(relativeTo: positioningRect, of: button, preferredEdge: .minY)
    }

    public func closePopover() {
        popover.close()
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

    private func applyIcon(for state: MenuBarState) {
        let description = MenuBarIconProvider.accessibilityDescription(for: state)
        statusItem?.button?.image = NSImage(named: "VelvtMenuBarIcon")
        statusItem?.button?.image?.isTemplate = true
        statusItem?.button?.toolTip = description
    }
}
