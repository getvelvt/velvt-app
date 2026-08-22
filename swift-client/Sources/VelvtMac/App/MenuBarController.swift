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

/// The surface the menu bar content is presented in.
///
/// Named for the `NSPopover` it started as, and still shaped like one so that
/// `NSPopover` keeps conforming — tests inject a trivial fake through it. The
/// shipping implementation is `MenuBarPanelPresenter`, because **an
/// `NSPopover` cannot be resized by the user**. That is not a styling
/// limitation, it is the whole class: `NSPopover`'s only non-accessibility
/// properties are `animates, appearance, behavior, contentSize,
/// contentViewController, delegate, detached, hasFullSizeContent,
/// positioningRect, positioningView, positioningWindow, shown` — no
/// `styleMask`, no `minSize`/`maxSize`, no `isResizable`, no
/// `frameAutosaveName`. A shown popover lives in an `_NSPopoverWindow` whose
/// `styleMask.rawValue` is `0` (borderless) and whose `isResizable` is
/// `false`, so there is no edge for the cursor to grab. `contentSize` is
/// settable only in code.
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

// MARK: - MenuBarWindowSizeStore

/// Persists the size the user dragged the window to.
///
/// Only the size. The origin is recomputed from the status item on every open
/// (`MenuBarPopoverLayout.windowFrame`), which is what keeps a restored window
/// from landing on a display that is no longer attached. `NSWindow`'s
/// `setFrameAutosaveName` would have persisted both for free, and that is
/// exactly why it is not used here.
public final class MenuBarWindowSizeStore {
    private static let widthKey = "velvt.menubar.window_content_width"
    private static let heightKey = "velvt.menubar.window_content_height"

    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// The stored size, or nil on a first launch — and also nil for a stored
    /// size that is not a usable window, so a corrupt or zeroed default falls
    /// back to the preferred size rather than opening something invisible.
    public var contentSize: CGSize? {
        guard defaults.object(forKey: Self.widthKey) != nil,
            defaults.object(forKey: Self.heightKey) != nil
        else { return nil }
        let width = CGFloat(defaults.double(forKey: Self.widthKey))
        let height = CGFloat(defaults.double(forKey: Self.heightKey))
        guard width.isFinite, height.isFinite, width > 0, height > 0 else { return nil }
        return CGSize(width: width, height: height)
    }

    public func store(_ size: CGSize) {
        guard size.width.isFinite, size.height.isFinite, size.width > 0, size.height > 0 else {
            return
        }
        defaults.set(Double(size.width), forKey: Self.widthKey)
        defaults.set(Double(size.height), forKey: Self.heightKey)
    }

    public func clear() {
        defaults.removeObject(forKey: Self.widthKey)
        defaults.removeObject(forKey: Self.heightKey)
    }
}

// MARK: - MenuBarPanelPresenter

/// The resizable replacement for the popover.
///
/// An `NSPanel` rather than an `NSWindow` so it can be `.nonactivatingPanel`:
/// measured, that mask still reports `canBecomeKey == true` (the Settings text
/// fields and the Escape handler need key) while `canBecomeMain` stays
/// `false`, so the panel never behaves like a document window.
///
/// The style mask is `.titled` **without** `.fullSizeContentView`. Measured on
/// this machine, `.fullSizeContentView` reports
/// `contentView.safeAreaInsets.top == 28` and lays the content out under the
/// title bar; without it the insets are zero on every edge and
/// `contentLayoutRect` equals the content view's bounds, so nothing can be
/// clipped by the chrome. The title bar is made transparent and its title and
/// standard buttons hidden, so what is left is a plain drag strip in the
/// window's own colour rather than a document title bar.
///
/// Not in Cmd-Tab and not in the Dock: that comes from the application's
/// `.accessory` activation policy, which the executable entry point owns and
/// this type deliberately does not touch. `.ignoresCycle` additionally keeps
/// the panel out of Cmd-` window cycling.
@MainActor
public final class MenuBarPanelPresenter: NSObject, PopoverPresenting, NSWindowDelegate {
    /// Accepted and ignored. Kept so `NSPopover` and this type stay
    /// interchangeable through `PopoverPresenting`; dismissal is implemented
    /// directly below rather than by an `NSPopover.Behavior`.
    public var behavior: NSPopover.Behavior = .transient

    /// Show and close stay synchronous, for the same reason the popover
    /// disabled its fade: `isShown` has to flip immediately.
    public var animates = false {
        didSet { panel.animationBehavior = animates ? .utilityWindow : .none }
    }

    /// Called with the content size after the user finishes dragging an edge.
    public var onUserResize: ((CGSize) -> Void)?

    /// Called when the window closed itself because focus left it, as opposed
    /// to the owner calling `close()`. The owner needs to know the difference:
    /// see `MenuBarController.togglePopover()`.
    public var onDismissedByFocusLoss: (() -> Void)?

    /// The content size the window should open at, and the bounds a drag may
    /// not go outside. Recomputed by the owner on every open.
    public var minimumContentSize = MenuBarPopoverLayout.minimumContentSize {
        didSet { panel.contentMinSize = minimumContentSize }
    }

    public var maximumContentSize = CGSize(
        width: CGFloat.greatestFiniteMagnitude,
        height: CGFloat.greatestFiniteMagnitude
    ) {
        didSet { panel.contentMaxSize = maximumContentSize }
    }

    public var contentViewController: NSViewController? {
        get { panel.contentViewController }
        set { panel.contentViewController = newValue }
    }

    /// Clamped into `[minimumContentSize, maximumContentSize]` on the way in.
    /// `NSWindow.contentMinSize` is enforced for user drags but *not* for
    /// `setContentSize(_:)` — measured: a panel with a 420x320 minimum accepts
    /// a programmatic 100x100 — so the clamp has to happen here.
    public var contentSize: NSSize {
        get { panel.contentRect(forFrameRect: panel.frame).size }
        set {
            let clamped = NSSize(
                width: min(max(newValue.width, min(minimumContentSize.width, maximumContentSize.width)), maximumContentSize.width),
                height: min(max(newValue.height, min(minimumContentSize.height, maximumContentSize.height)), maximumContentSize.height)
            )
            guard clamped.width > 0, clamped.height > 0 else { return }
            lastAppliedContentSize = clamped
            panel.setContentSize(clamped)
        }
    }

    public var isShown: Bool { panel.isVisible }

    /// Exposed so the owner can position the window and so tests can assert on
    /// the real thing rather than on a description of it.
    public let panel: NSPanel

    /// The last size this type put on the window itself.
    ///
    /// `windowDidResize` cannot distinguish a drag from a programmatic
    /// `setFrame`, and a flag set around the call is only correct if AppKit
    /// posts the notification synchronously — which is not guaranteed.
    /// Comparing sizes is order-independent: a resize that lands on the size
    /// we just asked for is ours, and storing it would at best be a no-op and
    /// at worst re-persist a screen-clamped size as the user's preference.
    private var lastAppliedContentSize: NSSize?

    public override init() {
        panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: MenuBarPopoverLayout.preferredContentSize),
            styleMask: [.nonactivatingPanel, .titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        super.init()
        panel.delegate = self
        panel.title = "Velvt"
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.isMovableByWindowBackground = false
        panel.isReleasedWhenClosed = false
        panel.isRestorable = false
        panel.becomesKeyOnlyIfNeeded = false
        // `hidesOnDeactivate` is the documented lever and it does apply to a
        // `.nonactivatingPanel` — but it only *orders out*, and AppKit orders
        // those windows back in when the app is activated again. The status
        // item click activates the app, so the panel would reappear underneath
        // the toggle and the click would read as a no-op. Closing explicitly
        // on resign-key is deterministic and matches `.transient` popover
        // dismissal, which is what the surface used to be.
        panel.hidesOnDeactivate = false
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        panel.animationBehavior = .none
        panel.contentMinSize = minimumContentSize
        panel.contentMaxSize = maximumContentSize
        // The content forces `.preferredColorScheme(.dark)`. Without pinning
        // the window to match, a title bar in Light mode would sit as a pale
        // strip above dark content. The colour is `Color.velvtSurface`, so the
        // drag strip reads as part of the surface rather than as chrome.
        panel.appearance = NSAppearance(named: .darkAqua)
        panel.backgroundColor = NSColor(
            srgbRed: 0.09, green: 0.08, blue: 0.10, alpha: 1
        )
    }

    // MARK: PopoverPresenting

    /// `positioningRect` and `preferredEdge` are accepted for protocol parity
    /// and are not used: a window under a status item is positioned from the
    /// status item's frame in screen coordinates, which `positioningView`
    /// carries and a rect in the view's own space does not.
    public func show(
        relativeTo positioningRect: NSRect,
        of positioningView: NSView,
        preferredEdge: NSRectEdge
    ) {
        _ = positioningRect
        _ = preferredEdge
        position(under: positioningView)
        panel.makeKeyAndOrderFront(nil)
    }

    public func close() {
        panel.orderOut(nil)
    }

    /// Recomputes the frame from wherever the status item is *now*. Called on
    /// every open, so a menu bar rearrangement, a display change or a notch
    /// moves the window with the item instead of stranding it.
    public func position(under positioningView: NSView) {
        let statusItemFrame = screenFrame(of: positioningView)
        let visibleFrame = positioningView.window?.screen?.visibleFrame
            ?? NSScreen.main?.visibleFrame
        maximumContentSize = MenuBarPopoverLayout.maximumContentSize(for: visibleFrame)
        let frame = MenuBarPopoverLayout.windowFrame(
            forContentSize: contentSize,
            statusItemFrame: statusItemFrame,
            visibleFrame: visibleFrame
        )
        lastAppliedContentSize = panel.contentRect(forFrameRect: frame).size
        panel.setFrame(frame, display: false)
    }

    private func screenFrame(of view: NSView) -> CGRect? {
        guard let window = view.window else { return nil }
        return window.convertToScreen(view.convert(view.bounds, to: nil))
    }

    // MARK: NSWindowDelegate

    public func windowDidResize(_ notification: Notification) {
        let size = contentSize
        guard size != lastAppliedContentSize else { return }
        lastAppliedContentSize = size
        onUserResize?(size)
    }

    /// Click-away dismissal, the `.transient` popover behaviour this surface
    /// replaced.
    ///
    /// Deferred by one turn because AppKit installs the *new* key window after
    /// posting this, and the guard below needs to see it: the focus-session
    /// composer is an `NSPopover` anchored inside this panel, the sign-in flow
    /// and the two destructive confirmations are sheets on it, and all of them
    /// take key. Measured, a popover shown from a view in this panel becomes a
    /// child window of it (`window.parent === panel`, and it appears in
    /// `panel.childWindows`), so the check is ordinary public API rather than
    /// a class-name test. Settings navigation is *not* in that list any more —
    /// it renders in this window and takes no key from it.
    public func windowDidResignKey(_ notification: Notification) {
        DispatchQueue.main.async { [weak self] in
            guard let self, self.panel.isVisible else { return }
            guard Self.shouldDismiss(panel: self.panel, keyWindow: NSApp.keyWindow) else { return }
            self.close()
            self.onDismissedByFocusLoss?()
        }
    }

    /// Pure so the rule can be tested against real windows without driving a
    /// real activation cycle.
    ///
    /// Key moving to a window this panel owns — the focus-session popover, a
    /// sign-in sheet, a confirmation sheet — is not the user clicking away,
    /// and dismissing on it would close the surface the moment anyone opened
    /// anything in it.
    static func shouldDismiss(panel: NSWindow, keyWindow: NSWindow?) -> Bool {
        guard let keyWindow else { return true }
        if keyWindow === panel { return false }
        if keyWindow.parent === panel { return false }
        if keyWindow.sheetParent === panel { return false }
        if panel.childWindows?.contains(where: { $0 === keyWindow }) == true { return false }
        if panel.sheets.contains(where: { $0 === keyWindow }) { return false }
        return true
    }
}

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
    private let windowSizeStore: MenuBarWindowSizeStore
    private let now: () -> Date
    private var cancellables = Set<AnyCancellable>()

    /// When the window last closed itself because focus left it.
    ///
    /// Clicking the status item while the window is open does two things in
    /// one event: it takes key away from the window, and it fires the toggle.
    /// The order those land in is AppKit's to decide. If the dismissal wins,
    /// the toggle sees a closed window and reopens it, and the click reads as
    /// doing nothing at all — the single most-used interaction in the app,
    /// broken intermittently. A toggle arriving in the shadow of a focus-loss
    /// dismissal is the *same* gesture, so it closes and stops rather than
    /// reopening.
    private var lastFocusLossDismissal: Date?

    /// Long enough to cover the two halves of one click landing in either
    /// order, short enough that a deliberate reopen a moment later still
    /// opens. Injectable so the rule is tested rather than waited on.
    private let focusLossToggleGrace: TimeInterval

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
        windowSizeStore: MenuBarWindowSizeStore = MenuBarWindowSizeStore(),
        focusLossToggleGrace: TimeInterval = 0.3,
        now: @escaping () -> Date = Date.init,
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
        // The default surface is a resizable panel, not a popover. See
        // `PopoverPresenting` for why an `NSPopover` cannot be the answer to
        // "let me drag this bigger".
        self.popover = popover ?? MenuBarPanelPresenter()
        self.statusItemManager = statusItemManager ?? SystemStatusItemManager()
        self.windowSizeStore = windowSizeStore
        self.focusLossToggleGrace = focusLossToggleGrace
        self.now = now
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
        self.popover.contentSize = MenuBarPopoverLayout.resolvedContentSize(
            stored: windowSizeStore.contentSize,
            visibleFrame: nil
        )
        if let panel = self.popover as? MenuBarPanelPresenter {
            panel.minimumContentSize = MenuBarPopoverLayout.minimumContentSize
            panel.onDismissedByFocusLoss = { [weak self] in
                guard let self else { return }
                self.lastFocusLossDismissal = self.now()
            }
            panel.onUserResize = { [weak self] size in
                guard let self else { return }
                self.windowSizeStore.store(
                    MenuBarPopoverLayout.storableContentSize(
                        size,
                        includesWalkthrough: self.guidedTour.isPresented
                    )
                )
            }
        }
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
            return
        }
        if let dismissal = lastFocusLossDismissal,
            now().timeIntervalSince(dismissal) < focusLossToggleGrace
        {
            // The window closed a moment ago because this very click took
            // focus off it. Consume the toggle instead of reopening.
            lastFocusLossDismissal = nil
            return
        }
        showPopover()
    }

    /// Shows the popover, activating the app first so it opens correctly
    /// even if the app is currently hidden (e.g. via Cmd+H or a notification
    /// tap arriving while backgrounded).
    public func showPopover() {
        guard let button = statusItemManager.button, !popover.isShown else { return }
        lastFocusLossDismissal = nil
        popoverWillOpen.send()
        let visibleFrame = button.window?.screen?.visibleFrame ?? NSScreen.main?.visibleFrame
        if let panel = popover as? MenuBarPanelPresenter {
            panel.maximumContentSize = MenuBarPopoverLayout.maximumContentSize(for: visibleFrame)
        }
        // The size the user dragged the window to outranks the preferred size,
        // but never the screen it is opening on.
        popover.contentSize = MenuBarPopoverLayout.resolvedContentSize(
            stored: windowSizeStore.contentSize,
            visibleFrame: visibleFrame,
            includesWalkthrough: guidedTour.isPresented
        )
        activateApp()
        let positioningRect = button.bounds.offsetBy(dx: -16, dy: 0)
        popover.show(relativeTo: positioningRect, of: button, preferredEdge: .minY)
    }

    public func closePopover() {
        popover.close()
    }

    /// Drives the focus-loss dismissal path without an activation cycle, so
    /// the toggle ordering rule can be tested rather than reasoned about.
    /// Production goes through `MenuBarPanelPresenter.onDismissedByFocusLoss`.
    func simulateFocusLossDismissalForTesting() {
        popover.close()
        lastFocusLossDismissal = now()
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
        popover.contentSize = MenuBarPopoverLayout.resolvedContentSize(
            stored: windowSizeStore.contentSize,
            visibleFrame: visibleFrame,
            includesWalkthrough: includesWalkthrough
        )
        // Growing by the tour bar moves the bottom edge down, which can push
        // the window off the bottom of the screen. Re-anchoring puts it back
        // under the status item at the new height.
        if let panel = popover as? MenuBarPanelPresenter,
            let button = statusItemManager.button,
            panel.isShown
        {
            panel.position(under: button)
        }
    }

    private func applyIcon(for state: MenuBarState) {
        let description = MenuBarIconProvider.accessibilityDescription(for: state)
        statusItemManager.button?.image = NSImage(named: "VelvtMenuBarIcon")
        statusItemManager.button?.image?.isTemplate = true
        statusItemManager.button?.toolTip = description
    }
}
