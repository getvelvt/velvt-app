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
    private var statusItem: NSStatusItem?
    private var cancellables = Set<AnyCancellable>()

    public init(
        presentation: PermissionPresentationModel,
        displayCoordinator: ConcreteDisplayDataCoordinator
    ) {
        popover = NSPopover()
        super.init()
        popover.behavior = .transient
        popover.contentViewController = NSHostingController(
            rootView: MenuBarPopoverView(
                presentation: presentation,
                coordinator: displayCoordinator,
                onEscape: { [weak self] in self?.closePopover() }
            )
        )
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
        Publishers.CombineLatest4(
            collectionStatus,
            connectionStatus,
            accountStateManager.$accountState,
            accountStateManager.$isDeviceRevoked
        )
        .map { [resolver] collection, connection, account, isRevoked in
            resolver.resolve(
                collectionStatus: collection,
                connectionStatus: connection,
                accountState: account,
                isDeviceRevoked: isRevoked
            )
        }
        .removeDuplicates()
        .receive(on: RunLoop.main)
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

    public func showPopover() {
        guard let button = statusItem?.button, !popover.isShown else { return }
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
    }

    public func closePopover() {
        popover.performClose(nil)
    }

    // MARK: Private

    @objc private func handleStatusItemClick() {
        togglePopover()
    }

    private func applyIcon(for state: MenuBarState) {
        let symbolName = MenuBarIconProvider.symbolName(for: state)
        let description = MenuBarIconProvider.accessibilityDescription(for: state)
        let image = NSImage(systemSymbolName: symbolName, accessibilityDescription: description)
        // Template rendering is required for automatic light/dark adaptation.
        image?.isTemplate = true
        statusItem?.button?.image = image
    }
}
