import AppKit
import ApplicationServices
import Combine
import Darwin
import Foundation

/// Collection is strictly event-driven. Scheduled or repeated activity checks
/// are prohibited in this module.

/// The collection layer's only output type.
public struct RawEvent: Equatable, Sendable {
    public let appName: String
    public let bundleIdentifier: String?
    public let windowTitle: String
    public let focusedDocumentURL: String?
    public let occurredAt: Date
    public let durationSeconds: Int

    public init(
        appName: String,
        bundleIdentifier: String? = nil,
        windowTitle: String,
        focusedDocumentURL: String? = nil,
        occurredAt: Date,
        durationSeconds: Int = 0
    ) {
        self.appName = appName
        self.bundleIdentifier = bundleIdentifier
        self.windowTitle = windowTitle
        self.focusedDocumentURL = focusedDocumentURL
        self.occurredAt = occurredAt
        self.durationSeconds = durationSeconds
    }

    func withDuration(seconds: Int) -> RawEvent {
        RawEvent(
            appName: appName,
            bundleIdentifier: bundleIdentifier,
            windowTitle: windowTitle,
            focusedDocumentURL: focusedDocumentURL,
            occurredAt: occurredAt,
            durationSeconds: seconds
        )
    }
}

public protocol EventSink: AnyObject {
    func receive(_ event: RawEvent)
}

public final class EventSinkFanout: EventSink {
    private let sinks: [any EventSink]

    public init(_ sinks: [any EventSink]) {
        self.sinks = sinks
    }

    public func receive(_ event: RawEvent) {
        for sink in sinks {
            sink.receive(event)
        }
    }
}

public enum CollectionStatus: Equatable, Sendable {
    case idle
    case running
    case permissionRevoked
    case error(String)
}

public protocol CollectionAgentProtocol: AnyObject {
    func start() throws
    func stop()
    var status: AnyPublisher<CollectionStatus, Never> { get }
}

public struct RunningApplication: Equatable, Sendable {
    public let processIdentifier: pid_t
    public let appName: String
    public let bundleIdentifier: String?

    public init(processIdentifier: pid_t, appName: String, bundleIdentifier: String? = nil) {
        self.processIdentifier = processIdentifier
        self.appName = appName
        self.bundleIdentifier = bundleIdentifier
    }
}

public struct FocusedActivity: Equatable, Sendable {
    public let windowTitle: String?
    public let focusedDocumentURL: String?

    public init(windowTitle: String?, focusedDocumentURL: String? = nil) {
        self.windowTitle = windowTitle
        self.focusedDocumentURL = focusedDocumentURL
    }
}

public protocol AccessibilityPermissionChecking: AnyObject {
    func hasPermission() -> Bool
}

public protocol WorkspaceActivationObserving: AnyObject {
    func start(activationHandler: @escaping (RunningApplication) -> Void) -> RunningApplication?
    func stop()
}

public protocol AccessibilityObserving: AnyObject {
    func start(
        observing application: RunningApplication,
        activityHandler: @escaping (FocusedActivity) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> FocusedActivity
    func stop()
}

public enum CollectionError: Error, Equatable {
    case permissionRevoked
    case observerRegistrationFailed(code: Int32)
}

public final class AXCollectionAgent: CollectionAgentProtocol {
    public var status: AnyPublisher<CollectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }

    private weak var eventSink: (any EventSink)?
    private let permissionChecker: any AccessibilityPermissionChecking
    private let workspaceObserver: any WorkspaceActivationObserving
    private let accessibilityObserver: any AccessibilityObserving
    private let now: () -> Date
    private let maximumDwellDuration: TimeInterval
    private let statusSubject = CurrentValueSubject<CollectionStatus, Never>(.idle)
    private let lock = NSLock()
    private var isRunning = false
    private var activeProcessIdentifier: pid_t?
    private var pendingDwellEvent: RawEvent?

    public convenience init(eventSink: any EventSink) {
        self.init(
            eventSink: eventSink,
            permissionChecker: SystemAccessibilityPermissionChecker(),
            workspaceObserver: NSWorkspaceActivationObserver(),
            accessibilityObserver: AXApplicationObserver()
        )
    }

    public init(
        eventSink: any EventSink,
        permissionChecker: any AccessibilityPermissionChecking,
        workspaceObserver: any WorkspaceActivationObserving,
        accessibilityObserver: any AccessibilityObserving,
        now: @escaping () -> Date = Date.init,
        maximumDwellDuration: TimeInterval = 30 * 60
    ) {
        self.eventSink = eventSink
        self.permissionChecker = permissionChecker
        self.workspaceObserver = workspaceObserver
        self.accessibilityObserver = accessibilityObserver
        self.now = now
        self.maximumDwellDuration = maximumDwellDuration
    }

    public func start() throws {
        guard lock.withLock({ !isRunning }) else {
            return
        }
        guard permissionChecker.hasPermission() else {
            statusSubject.send(.permissionRevoked)
            return
        }
        let shouldStart = lock.withLock {
            guard !isRunning else {
                return false
            }
            isRunning = true
            return true
        }
        guard shouldStart else {
            return
        }

        let currentApplication = workspaceObserver.start { [weak self] application in
            self?.applicationDidActivate(application)
        }
        statusSubject.send(.running)
        if let currentApplication {
            do {
                try observe(currentApplication)
            } catch CollectionError.permissionRevoked {
                stopAfterPermissionRevocation()
            } catch let CollectionError.observerRegistrationFailed(code) {
                statusSubject.send(.error("ax_observer_registration_failed:\(code)"))
            } catch {
                statusSubject.send(.error("ax_observer_registration_failed"))
            }
        }
    }

    public func stop() {
        let result = lock.withLock { () -> (shouldStop: Bool, finalEvent: RawEvent?) in
            guard isRunning else {
                return (false, nil)
            }
            isRunning = false
            activeProcessIdentifier = nil
            let finalEvent = pendingDwellEvent.map {
                $0.withDuration(seconds: dwellSeconds(from: $0.occurredAt, through: now()))
            }
            pendingDwellEvent = nil
            return (true, finalEvent)
        }
        guard result.shouldStop else {
            return
        }
        if let finalEvent = result.finalEvent {
            eventSink?.receive(finalEvent)
        }
        accessibilityObserver.stop()
        workspaceObserver.stop()
        statusSubject.send(.idle)
    }

    deinit {
        stop()
    }

    private func applicationDidActivate(_ application: RunningApplication) {
        guard lock.withLock({ isRunning }) else {
            return
        }
        guard permissionChecker.hasPermission() else {
            stopAfterPermissionRevocation()
            return
        }
        guard lock.withLock({ activeProcessIdentifier != application.processIdentifier }) else {
            return
        }
        do {
            try observe(application)
        } catch CollectionError.permissionRevoked {
            stopAfterPermissionRevocation()
        } catch let CollectionError.observerRegistrationFailed(code) {
            statusSubject.send(.error("ax_observer_registration_failed:\(code)"))
        } catch {
            statusSubject.send(.error("ax_observer_registration_failed"))
        }
    }

    private func observe(_ application: RunningApplication) throws {
        accessibilityObserver.stop()
        lock.withLock {
            activeProcessIdentifier = application.processIdentifier
        }
        let initialActivity: FocusedActivity
        do {
            initialActivity = try accessibilityObserver.start(
                observing: application,
                activityHandler: { [weak self] activity in
                    self?.emit(
                        processIdentifier: application.processIdentifier,
                        appName: application.appName,
                        bundleIdentifier: application.bundleIdentifier,
                        activity: activity
                    )
                },
                errorHandler: { [weak self] error in
                    self?.accessibilityObserverFailed(
                        error,
                        processIdentifier: application.processIdentifier
                    )
                }
            )
        } catch {
            lock.withLock {
                if activeProcessIdentifier == application.processIdentifier {
                    activeProcessIdentifier = nil
                }
            }
            throw error
        }
        emit(
            processIdentifier: application.processIdentifier,
            appName: application.appName,
            bundleIdentifier: application.bundleIdentifier,
            activity: initialActivity
        )
    }

    private func emit(
        processIdentifier: pid_t,
        appName: String,
        bundleIdentifier: String?,
        activity: FocusedActivity
    ) {
        guard permissionChecker.hasPermission() else {
            stopAfterPermissionRevocation()
            return
        }
        let nextEvent = RawEvent(
            appName: appName,
            bundleIdentifier: bundleIdentifier,
            windowTitle: activity.windowTitle ?? "",
            focusedDocumentURL: activity.focusedDocumentURL,
            occurredAt: now()
        )
        let completedEvent = lock.withLock { () -> RawEvent? in
            guard isRunning && activeProcessIdentifier == processIdentifier else {
                return nil
            }
            guard let previousEvent = pendingDwellEvent else {
                pendingDwellEvent = nextEvent
                return nil
            }
            guard previousEvent.appName != nextEvent.appName
                || previousEvent.bundleIdentifier != nextEvent.bundleIdentifier
                || previousEvent.windowTitle != nextEvent.windowTitle
                || previousEvent.focusedDocumentURL != nextEvent.focusedDocumentURL
            else {
                return nil
            }
            pendingDwellEvent = nextEvent
            return previousEvent.withDuration(seconds: dwellSeconds(
                from: previousEvent.occurredAt,
                through: nextEvent.occurredAt
            ))
        }
        if let completedEvent {
            eventSink?.receive(completedEvent)
        }
    }

    private func accessibilityObserverFailed(_ error: CollectionError, processIdentifier: pid_t) {
        if error == .permissionRevoked {
            stopAfterPermissionRevocation()
            return
        }
        let result = lock.withLock { () -> (shouldHandle: Bool, finalEvent: RawEvent?) in
            guard isRunning && activeProcessIdentifier == processIdentifier else {
                return (false, nil)
            }
            activeProcessIdentifier = nil
            let finalEvent = pendingDwellEvent.map {
                $0.withDuration(seconds: dwellSeconds(from: $0.occurredAt, through: now()))
            }
            pendingDwellEvent = nil
            return (true, finalEvent)
        }
        guard result.shouldHandle else {
            return
        }
        accessibilityObserver.stop()
        if let finalEvent = result.finalEvent {
            eventSink?.receive(finalEvent)
        }
        if case let .observerRegistrationFailed(code) = error {
            statusSubject.send(.error("ax_observer_failed:\(code)"))
        } else {
            statusSubject.send(.error("ax_observer_failed"))
        }
    }

    private func stopAfterPermissionRevocation() {
        let result = lock.withLock { () -> (shouldStop: Bool, finalEvent: RawEvent?) in
            guard isRunning else {
                return (false, nil)
            }
            isRunning = false
            activeProcessIdentifier = nil
            let finalEvent = pendingDwellEvent.map {
                $0.withDuration(seconds: dwellSeconds(from: $0.occurredAt, through: now()))
            }
            pendingDwellEvent = nil
            return (true, finalEvent)
        }
        guard result.shouldStop else {
            return
        }
        if let finalEvent = result.finalEvent {
            eventSink?.receive(finalEvent)
        }
        accessibilityObserver.stop()
        workspaceObserver.stop()
        statusSubject.send(.permissionRevoked)
    }

    private func dwellSeconds(from start: Date, through end: Date) -> Int {
        let elapsed = max(0, end.timeIntervalSince(start))
        return Int(min(elapsed, maximumDwellDuration).rounded(.down))
    }
}

public final class FakeCollectionAgent: CollectionAgentProtocol {
    public var status: AnyPublisher<CollectionStatus, Never> {
        statusSubject.eraseToAnyPublisher()
    }

    private weak var eventSink: (any EventSink)?
    private let statusSubject = CurrentValueSubject<CollectionStatus, Never>(.idle)
    private var isRunning = false

    public init(eventSink: any EventSink) {
        self.eventSink = eventSink
    }

    public func start() throws {
        guard !isRunning else {
            return
        }
        isRunning = true
        statusSubject.send(.running)
    }

    public func stop() {
        guard isRunning else {
            return
        }
        isRunning = false
        statusSubject.send(.idle)
    }

    public func injectEvent(_ event: RawEvent) {
        guard isRunning else {
            return
        }
        eventSink?.receive(event)
    }
}

public final class NSWorkspaceActivationObserver: WorkspaceActivationObserving {
    private var subscription: NSObjectProtocol?

    public init() {}

    public func start(activationHandler: @escaping (RunningApplication) -> Void) -> RunningApplication? {
        guard subscription == nil else {
            return snapshot(NSWorkspace.shared.frontmostApplication)
        }
        subscription = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification,
            object: nil,
            queue: nil
        ) { [weak self] notification in
            guard
                let self,
                let application = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
                let snapshot = self.snapshot(application)
            else {
                return
            }
            activationHandler(snapshot)
        }
        return snapshot(NSWorkspace.shared.frontmostApplication)
    }

    public func stop() {
        guard let subscription else {
            return
        }
        NSWorkspace.shared.notificationCenter.removeObserver(subscription)
        self.subscription = nil
    }

    deinit {
        stop()
    }

    private func snapshot(_ application: NSRunningApplication?) -> RunningApplication? {
        guard let application, let appName = application.localizedName else {
            return nil
        }
        return RunningApplication(
            processIdentifier: application.processIdentifier,
            appName: appName,
            bundleIdentifier: application.bundleIdentifier
        )
    }
}

public final class AXApplicationObserver: AccessibilityObserving {
    private let lock = NSLock()
    private let callbackQueue: DispatchQueue
    private var observer: AXObserver?
    private var runLoop: CFRunLoop?
    private var runLoopSource: CFRunLoopSource?
    private var activityHandler: ((FocusedActivity) -> Void)?
    private var errorHandler: ((CollectionError) -> Void)?
    private var applicationElement: AXUIElement?
    private var focusedWindow: AXUIElement?
    private var observesBrowserDocument = false

    public init(callbackQueue: DispatchQueue = DispatchQueue(label: "com.velvt.collection.events")) {
        self.callbackQueue = callbackQueue
    }

    public func start(
        observing application: RunningApplication,
        activityHandler: @escaping (FocusedActivity) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> FocusedActivity {
        stop()
        var createdObserver: AXObserver?
        let result = AXObserverCreate(application.processIdentifier, Self.callback, &createdObserver)
        guard result != .apiDisabled else {
            throw CollectionError.permissionRevoked
        }
        guard result == .success, let createdObserver else {
            throw CollectionError.observerRegistrationFailed(code: result.rawValue)
        }

        let applicationElement = AXUIElementCreateApplication(application.processIdentifier)
        guard let initialWindow = copyElement(attribute: kAXFocusedWindowAttribute, from: applicationElement)
            ?? copyElement(attribute: kAXMainWindowAttribute, from: applicationElement)
        else {
            throw CollectionError.observerRegistrationFailed(code: AXError.noValue.rawValue)
        }
        for (element, notification) in [
            (applicationElement, kAXFocusedWindowChangedNotification),
            (initialWindow, kAXTitleChangedNotification)
        ] {
            let registration = AXObserverAddNotification(
                createdObserver,
                element,
                notification as CFString,
                Unmanaged.passUnretained(self).toOpaque()
            )
            guard registration != .apiDisabled else {
                throw CollectionError.permissionRevoked
            }
            guard registration == .success else {
                throw CollectionError.observerRegistrationFailed(code: registration.rawValue)
            }
        }
        addOptionalBrowserNotifications(
            observer: createdObserver,
            applicationElement: applicationElement,
            window: initialWindow,
            bundleIdentifier: application.bundleIdentifier
        )

        let source = AXObserverGetRunLoopSource(createdObserver)
        let started = DispatchSemaphore(value: 0)
        let thread = Thread { [weak self] in
            guard let self else {
                started.signal()
                return
            }
            let currentRunLoop = CFRunLoopGetCurrent()
            self.lock.withLock {
                self.runLoop = currentRunLoop
            }
            CFRunLoopAddSource(currentRunLoop, source, .defaultMode)
            started.signal()
            CFRunLoopRun()
        }
        lock.withLock {
            observer = createdObserver
            runLoopSource = source
            self.activityHandler = activityHandler
            self.errorHandler = errorHandler
            self.applicationElement = applicationElement
            focusedWindow = initialWindow
            observesBrowserDocument = Self.isSupportedBrowser(
                bundleIdentifier: application.bundleIdentifier)
        }
        thread.name = "com.velvt.collection.ax-run-loop"
        thread.start()
        started.wait()
        return snapshot(applicationElement: applicationElement, window: initialWindow)
    }

    public func stop() {
        let resources = lock.withLock { () -> (CFRunLoop?, CFRunLoopSource?) in
            let resources = (runLoop, runLoopSource)
            observer = nil
            runLoop = nil
            runLoopSource = nil
            activityHandler = nil
            errorHandler = nil
            applicationElement = nil
            focusedWindow = nil
            observesBrowserDocument = false
            return resources
        }
        guard let runLoop = resources.0, let source = resources.1 else {
            return
        }
        CFRunLoopRemoveSource(runLoop, source, .defaultMode)
        CFRunLoopStop(runLoop)
    }

    deinit {
        stop()
    }

    private static let callback: AXObserverCallback = { _, element, notification, context in
        guard let context else {
            return
        }
        // The context is safe because AXApplicationObserver owns the AXObserver
        // and removes its run-loop source before the controller can deallocate.
        let controller = Unmanaged<AXApplicationObserver>.fromOpaque(context).takeUnretainedValue()
        controller.handle(element, notification: notification as String)
    }

    private func handle(_ element: AXUIElement, notification: String) {
        // AX callbacks run on a private CFRunLoop. Delivery crosses explicitly
        // onto a serial dispatch queue; no AXUIElement leaves the callback.
        do {
            let activity = try refreshSnapshot(notification: notification, notifiedElement: element)
            let handler = lock.withLock { activityHandler }
            callbackQueue.async { handler?(activity) }
        } catch let error as CollectionError {
            let handler = lock.withLock { errorHandler }
            callbackQueue.async { handler?(error) }
        } catch {
            let handler = lock.withLock { errorHandler }
            callbackQueue.async { handler?(.observerRegistrationFailed(code: AXError.failure.rawValue)) }
        }
    }

    private func refreshSnapshot(notification: String, notifiedElement: AXUIElement) throws -> FocusedActivity {
        let resources = lock.withLock { (observer, applicationElement, focusedWindow) }
        guard let applicationElement = resources.1 else {
            throw CollectionError.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue)
        }
        var window = resources.2 ?? notifiedElement
        if notification == kAXFocusedWindowChangedNotification,
            let nextWindow = copyElement(attribute: kAXFocusedWindowAttribute, from: applicationElement)
        {
            window = nextWindow
            if let observer = resources.0 {
                if let previousWindow = resources.2 {
                    AXObserverRemoveNotification(observer, previousWindow, kAXTitleChangedNotification as CFString)
                    for optionalNotification in Self.optionalWindowNotifications {
                        AXObserverRemoveNotification(observer, previousWindow, optionalNotification as CFString)
                    }
                }
                let registration = AXObserverAddNotification(
                    observer,
                    nextWindow,
                    kAXTitleChangedNotification as CFString,
                    Unmanaged.passUnretained(self).toOpaque()
                )
                guard registration != .apiDisabled else { throw CollectionError.permissionRevoked }
                guard registration == .success || registration == .notificationAlreadyRegistered else {
                    throw CollectionError.observerRegistrationFailed(code: registration.rawValue)
                }
                if lock.withLock({ observesBrowserDocument }) {
                    for optionalNotification in Self.optionalWindowNotifications {
                        _ = AXObserverAddNotification(
                            observer,
                            nextWindow,
                            optionalNotification as CFString,
                            Unmanaged.passUnretained(self).toOpaque()
                        )
                    }
                }
            }
            lock.withLock { focusedWindow = nextWindow }
        }
        return snapshot(applicationElement: applicationElement, window: window)
    }

    private func snapshot(applicationElement: AXUIElement, window: AXUIElement) -> FocusedActivity {
        FocusedActivity(
            windowTitle: copyTitle(from: window),
            focusedDocumentURL: lock.withLock { observesBrowserDocument }
                ? copyFocusedDocumentURL(applicationElement: applicationElement, window: window)
                : nil
        )
    }

    private func copyFocusedDocumentURL(applicationElement: AXUIElement, window: AXUIElement) -> String? {
        for element in [window, copyElement(attribute: kAXFocusedUIElementAttribute, from: applicationElement)].compactMap({ $0 }) {
            var candidate: AXUIElement? = element
            for _ in 0 ..< 5 {
                guard let current = candidate else { break }
                for attribute in [kAXDocumentAttribute, kAXURLAttribute] {
                    if let value = copyString(attribute: attribute, from: current), !value.isEmpty {
                        return value
                    }
                }
                candidate = copyElement(attribute: kAXParentAttribute, from: current)
            }
        }
        return nil
    }

    private func copyString(attribute: String, from element: AXUIElement) -> String? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else {
            return nil
        }
        if let value = value as? String { return value }
        if let value = value as? URL { return value.absoluteString }
        return nil
    }

    private static let browserBundleIdentifiers: Set<String> = [
        "com.apple.Safari",
        "com.google.Chrome",
        "org.chromium.Chromium",
        "com.microsoft.edgemac",
        "com.brave.Browser",
        "company.thebrowser.Browser",
        "company.thebrowser.dia",
        "org.mozilla.firefox",
        "com.operasoftware.Opera",
        "com.operasoftware.OperaGX",
        "com.vivaldi.Vivaldi",
        "com.kagi.kagimacOS"
    ]

    static func isSupportedBrowser(bundleIdentifier: String?) -> Bool {
        guard let bundleIdentifier else { return false }
        if browserBundleIdentifiers.contains(bundleIdentifier) { return true }
        return [
            "com.google.Chrome.",
            "com.microsoft.edgemac.",
            "com.brave.Browser.",
            "org.mozilla.firefox."
        ].contains { bundleIdentifier.hasPrefix($0) }
    }

    private static let optionalWindowNotifications = [
        kAXValueChangedNotification,
        kAXSelectedChildrenChangedNotification,
        kAXSelectedRowsChangedNotification
    ]

    private func addOptionalBrowserNotifications(
        observer: AXObserver,
        applicationElement: AXUIElement,
        window: AXUIElement,
        bundleIdentifier: String?
    ) {
        guard Self.isSupportedBrowser(bundleIdentifier: bundleIdentifier) else { return }
        _ = AXObserverAddNotification(
            observer,
            applicationElement,
            kAXFocusedUIElementChangedNotification as CFString,
            Unmanaged.passUnretained(self).toOpaque()
        )
        for notification in Self.optionalWindowNotifications {
            _ = AXObserverAddNotification(
                observer,
                window,
                notification as CFString,
                Unmanaged.passUnretained(self).toOpaque()
            )
        }
    }

    private func copyElement(attribute: String, from element: AXUIElement) -> AXUIElement? {
        var value: CFTypeRef?
        guard
            AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
            let value,
            CFGetTypeID(value) == AXUIElementGetTypeID()
        else {
            return nil
        }
        return unsafeBitCast(value, to: AXUIElement.self)
    }

    private func copyTitle(from element: AXUIElement) -> String? {
        guard case let .success(title) = copyTitleResult(from: element) else {
            return nil
        }
        return title
    }

    private func copyTitleResult(from element: AXUIElement) -> Result<String?, CollectionError> {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, kAXTitleAttribute as CFString, &value)
        switch result {
        case .success:
            return .success(value as? String)
        case .noValue, .attributeUnsupported:
            return .success(nil)
        case .apiDisabled:
            return .failure(.permissionRevoked)
        default:
            return .failure(.observerRegistrationFailed(code: result.rawValue))
        }
    }
}
