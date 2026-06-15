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
    public let windowTitle: String
    public let occurredAt: Date

    public init(appName: String, windowTitle: String, occurredAt: Date) {
        self.appName = appName
        self.windowTitle = windowTitle
        self.occurredAt = occurredAt
    }
}

public protocol EventSink: AnyObject {
    func receive(_ event: RawEvent)
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

    public init(processIdentifier: pid_t, appName: String) {
        self.processIdentifier = processIdentifier
        self.appName = appName
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
        titleHandler: @escaping (String?) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> String?
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
    private let statusSubject = CurrentValueSubject<CollectionStatus, Never>(.idle)
    private let lock = NSLock()
    private var isRunning = false
    private var activeProcessIdentifier: pid_t?

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
        now: @escaping () -> Date = Date.init
    ) {
        self.eventSink = eventSink
        self.permissionChecker = permissionChecker
        self.workspaceObserver = workspaceObserver
        self.accessibilityObserver = accessibilityObserver
        self.now = now
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
            } catch {
                stop()
                throw error
            }
        }
    }

    public func stop() {
        let shouldStop = lock.withLock {
            guard isRunning else {
                return false
            }
            isRunning = false
            activeProcessIdentifier = nil
            return true
        }
        guard shouldStop else {
            return
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
        let initialTitle: String?
        do {
            initialTitle = try accessibilityObserver.start(
                observing: application,
                titleHandler: { [weak self] title in
                    self?.emit(
                        processIdentifier: application.processIdentifier,
                        appName: application.appName,
                        windowTitle: title ?? ""
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
            windowTitle: initialTitle ?? ""
        )
    }

    private func emit(processIdentifier: pid_t, appName: String, windowTitle: String) {
        guard lock.withLock({ isRunning && activeProcessIdentifier == processIdentifier }) else {
            return
        }
        guard permissionChecker.hasPermission() else {
            stopAfterPermissionRevocation()
            return
        }
        eventSink?.receive(RawEvent(appName: appName, windowTitle: windowTitle, occurredAt: now()))
    }

    private func accessibilityObserverFailed(_ error: CollectionError, processIdentifier: pid_t) {
        guard lock.withLock({ isRunning && activeProcessIdentifier == processIdentifier }) else {
            return
        }
        if error == .permissionRevoked {
            stopAfterPermissionRevocation()
            return
        }
        accessibilityObserver.stop()
        lock.withLock {
            activeProcessIdentifier = nil
        }
        if case let .observerRegistrationFailed(code) = error {
            statusSubject.send(.error("ax_observer_failed:\(code)"))
        } else {
            statusSubject.send(.error("ax_observer_failed"))
        }
    }

    private func stopAfterPermissionRevocation() {
        let shouldStop = lock.withLock {
            guard isRunning else {
                return false
            }
            isRunning = false
            activeProcessIdentifier = nil
            return true
        }
        guard shouldStop else {
            return
        }
        accessibilityObserver.stop()
        workspaceObserver.stop()
        statusSubject.send(.permissionRevoked)
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

public final class SystemAccessibilityPermissionChecker: AccessibilityPermissionChecking {
    public init() {}

    public func hasPermission() -> Bool {
        AXIsProcessTrusted()
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
        return RunningApplication(processIdentifier: application.processIdentifier, appName: appName)
    }
}

public final class AXApplicationObserver: AccessibilityObserving {
    private let lock = NSLock()
    private let callbackQueue: DispatchQueue
    private var observer: AXObserver?
    private var runLoop: CFRunLoop?
    private var runLoopSource: CFRunLoopSource?
    private var titleHandler: ((String?) -> Void)?
    private var errorHandler: ((CollectionError) -> Void)?

    public init(callbackQueue: DispatchQueue = DispatchQueue(label: "com.velvt.collection.events")) {
        self.callbackQueue = callbackQueue
    }

    public func start(
        observing application: RunningApplication,
        titleHandler: @escaping (String?) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> String? {
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
        guard let mainWindow = copyElement(attribute: kAXMainWindowAttribute, from: applicationElement) else {
            throw CollectionError.observerRegistrationFailed(code: AXError.noValue.rawValue)
        }
        for notification in [kAXFocusedWindowChangedNotification, kAXTitleChangedNotification] {
            let registration = AXObserverAddNotification(
                createdObserver,
                mainWindow,
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
            self.titleHandler = titleHandler
            self.errorHandler = errorHandler
        }
        thread.name = "com.velvt.collection.ax-run-loop"
        thread.start()
        started.wait()
        return copyTitle(from: mainWindow)
    }

    public func stop() {
        let resources = lock.withLock { () -> (CFRunLoop?, CFRunLoopSource?) in
            let resources = (runLoop, runLoopSource)
            observer = nil
            runLoop = nil
            runLoopSource = nil
            titleHandler = nil
            errorHandler = nil
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

    private static let callback: AXObserverCallback = { _, element, _, context in
        guard let context else {
            return
        }
        // The context is safe because AXApplicationObserver owns the AXObserver
        // and removes its run-loop source before the controller can deallocate.
        let controller = Unmanaged<AXApplicationObserver>.fromOpaque(context).takeUnretainedValue()
        controller.handle(element)
    }

    private func handle(_ element: AXUIElement) {
        // AX callbacks run on a private CFRunLoop. Delivery crosses explicitly
        // onto a serial dispatch queue; no AXUIElement leaves the callback.
        switch copyTitleResult(from: element) {
        case let .success(title):
            let handler = lock.withLock { titleHandler }
            callbackQueue.async {
                handler?(title)
            }
        case let .failure(error):
            let handler = lock.withLock { errorHandler }
            callbackQueue.async {
                handler?(error)
            }
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
