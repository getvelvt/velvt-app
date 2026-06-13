import Darwin
import Foundation

/// Collection module - owns event-driven NSWorkspace and Accessibility event
/// observation. Does NOT poll, persist events, abstract data, call cloud APIs,
/// or render UI.

/// Local-only raw event captured from macOS.
public struct CapturedRawEvent: Equatable, Sendable {
    /// Stable event identifier.
    public let eventID: UUID
    /// UTC event timestamp.
    public let occurredAt: Date
    /// Raw application name; local-only.
    public let appName: String
    /// Raw window title; local-only.
    public let windowTitle: String
    /// Optional raw bundle identifier; local-only.
    public let bundleID: String?

    public init(
        eventID: UUID,
        occurredAt: Date,
        appName: String,
        windowTitle: String,
        bundleID: String?
    ) {
        self.eventID = eventID
        self.occurredAt = occurredAt
        self.appName = appName
        self.windowTitle = windowTitle
        self.bundleID = bundleID
    }
}

/// Current event-collection availability.
public enum CollectionStatus: Equatable, Sendable {
    case stopped
    case running
    case accessibilityPermissionRequired
}

/// Receives captured events from event-driven macOS observers.
public protocol EventCollecting: AnyObject {
    var status: CollectionStatus { get }
    func start(eventHandler: @escaping (CapturedRawEvent) -> Void) throws
    func stop()
}

/// Owns the per-process Accessibility observer lifecycle.
public protocol AccessibilityObserving: AnyObject {
    func observe(processIdentifier: pid_t, eventHandler: @escaping (CapturedRawEvent) -> Void) throws
    func tearDown()
}

/// Observes foreground application activation notifications.
public protocol WorkspaceActivationObserving: AnyObject {
    func start(activationHandler: @escaping (pid_t) -> Void)
    func stop()
}

/// Safe collection errors that never include raw event content.
public enum CollectionError: Error, Equatable {
    case accessibilityPermissionDenied
    case observerRegistrationFailed(code: Int)
}
