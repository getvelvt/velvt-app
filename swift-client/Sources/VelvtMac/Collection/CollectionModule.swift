import AppKit
import ApplicationServices
import Combine
import Darwin
import Foundation
import os

/// Collection is strictly event-driven. Scheduled or repeated activity checks
/// are prohibited in this module.

/// The collection layer's only output type.
public struct RawEvent: Equatable, Sendable {
    public let appName: String
    public let bundleIdentifier: String?
    /// The `LSApplicationCategoryType` the application declares in its own
    /// `Info.plist`, verbatim. A reported fact: this layer never decides what it
    /// means, and most declared values mean nothing.
    public let declaredAppCategory: String?
    /// The `LSItemContentTypes` the application declares across
    /// `CFBundleDocumentTypes`, flattened, deduplicated and sorted. Empty when
    /// the application declares none or its plist could not be read.
    public let documentTypeIDs: [String]
    public let windowTitle: String
    public let focusedDocumentURL: String?
    public let occurredAt: Date
    public let durationSeconds: Int

    public init(
        appName: String,
        bundleIdentifier: String? = nil,
        declaredAppCategory: String? = nil,
        documentTypeIDs: [String] = [],
        windowTitle: String,
        focusedDocumentURL: String? = nil,
        occurredAt: Date,
        durationSeconds: Int = 0
    ) {
        self.appName = appName
        self.bundleIdentifier = bundleIdentifier
        self.declaredAppCategory = declaredAppCategory
        self.documentTypeIDs = documentTypeIDs
        self.windowTitle = windowTitle
        self.focusedDocumentURL = focusedDocumentURL
        self.occurredAt = occurredAt
        self.durationSeconds = durationSeconds
    }

    func withDuration(seconds: Int) -> RawEvent {
        RawEvent(
            appName: appName,
            bundleIdentifier: bundleIdentifier,
            declaredAppCategory: declaredAppCategory,
            documentTypeIDs: documentTypeIDs,
            windowTitle: windowTitle,
            focusedDocumentURL: focusedDocumentURL,
            occurredAt: occurredAt,
            durationSeconds: seconds
        )
    }

    /// The same activity, re-opened at `instant` with nothing measured yet.
    ///
    /// Used to split one continuous dwell into two abutting spans. The
    /// identity fields are preserved verbatim so the agent's
    /// same-activity comparison still treats a later notification for this
    /// app as a continuation rather than a switch.
    func reanchored(at instant: Date) -> RawEvent {
        RawEvent(
            appName: appName,
            bundleIdentifier: bundleIdentifier,
            declaredAppCategory: declaredAppCategory,
            documentTypeIDs: documentTypeIDs,
            windowTitle: windowTitle,
            focusedDocumentURL: focusedDocumentURL,
            occurredAt: instant,
            durationSeconds: 0
        )
    }
}

public protocol EventSink: AnyObject {
    /// A dwell that has ended, carrying its measured duration.
    func receive(_ event: RawEvent)

    /// A dwell that has just begun: the same activity `receive(_:)` will be
    /// handed when it ends, with nothing measured yet.
    ///
    /// Always called after `receive(_:)` for the dwell it replaces, so a sink
    /// sees one activity close before the next opens. Optional: a sink that
    /// only keeps a ledger of measured time has nothing to do here.
    func activityBegan(_ event: RawEvent)
}

extension EventSink {
    public func activityBegan(_ event: RawEvent) {}
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

    public func activityBegan(_ event: RawEvent) {
        for sink in sinks {
            sink.activityBegan(event)
        }
    }
}

public enum CollectionStatus: Equatable, Sendable {
    case idle
    case running
    /// Still collecting, but the application in front could not be observed
    /// at window level. The code says why, as a fixed token and an `AXError`
    /// number, never an application name or a title.
    ///
    /// Not a stop: the workspace observer is still running, and the next
    /// activation that registers returns the agent to `.running`. The usual
    /// cause is an application with no focused or main window at the moment it
    /// was activated.
    case limited(String)
    case permissionRevoked
    /// Collection stopped on a failure. `AXCollectionAgent` never reports one
    /// any more: every AX failure it sees leaves it collecting, as `.limited`.
    case error(String)

    /// Whether the agent is still observing application activations.
    public var isCollecting: Bool {
        switch self {
        case .running, .limited: return true
        case .idle, .permissionRevoked, .error: return false
        }
    }
}

/// One unified-log line per collection status transition, at `.default`, so
/// it is kept on disk and `log show` finds it after the fact.
///
/// The status never used to be logged at all, so "Collection paused" on a Mac
/// that was plainly collecting could not be traced to the failure behind it.
/// Every token is fixed or an `AXError` number, so the line is public.
public enum CollectionStatusLog {
    private static let log = Logger(subsystem: "com.velvt.mac", category: "Collection")

    public static func report(from previous: CollectionStatus, to status: CollectionStatus) {
        let line = line(from: previous, to: status)
        log.log(level: line.level, "\(line.message, privacy: .public)")
    }

    static func line(
        from previous: CollectionStatus,
        to status: CollectionStatus
    ) -> (level: OSLogType, message: String) {
        var message = "collection_status_changed from=\(token(previous)) to=\(token(status))"
        if let reason = reason(status) {
            message += " reason=\(reason)"
        }
        return (.default, message)
    }

    private static func token(_ status: CollectionStatus) -> String {
        switch status {
        case .idle: return "idle"
        case .running: return "running"
        case .limited: return "limited"
        case .permissionRevoked: return "permission_revoked"
        case .error: return "error"
        }
    }

    private static func reason(_ status: CollectionStatus) -> String? {
        switch status {
        case .limited(let code), .error(let code): return code
        case .idle, .running, .permissionRevoked: return nil
        }
    }
}

public protocol CollectionAgentProtocol: AnyObject {
    func start() throws
    func stop()
    /// Whether the agent is observing right now.
    ///
    /// `PermissionCollectionCoordinator` used to keep its own copy of this and
    /// consult that instead. An agent stops itself when the AX observer reports
    /// the permission was revoked, so the copy went stale exactly when it
    /// mattered: the coordinator went on believing collection was running and
    /// refused to start it again, and nothing on any surface said so.
    var isRunning: Bool { get }
    var status: AnyPublisher<CollectionStatus, Never> { get }
}

public struct RunningApplication: Equatable, Sendable {
    public let processIdentifier: pid_t
    public let appName: String
    public let bundleIdentifier: String?
    /// The application bundle on disk, when macOS reports one. Carried so the
    /// declared metadata can be read from the application's own `Info.plist`;
    /// absent for a process that is not a bundled application.
    public let bundleURL: URL?

    public init(
        processIdentifier: pid_t,
        appName: String,
        bundleIdentifier: String? = nil,
        bundleURL: URL? = nil
    ) {
        self.processIdentifier = processIdentifier
        self.appName = appName
        self.bundleIdentifier = bundleIdentifier
        self.bundleURL = bundleURL
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

/// What registering for an application found.
public enum AccessibilityRegistration: Equatable, Sendable {
    /// Registered at window level. The focused window's activity right now.
    case window(FocusedActivity)
    /// Registered for the application, which has no focused or main window
    /// yet (`kAXErrorNoValue`): one still launching, one whose windows are all
    /// closed, a panel not yet key. The activity handler reports the first
    /// window that gains focus.
    case awaitingWindow
}

public protocol AccessibilityPermissionChecking: AnyObject {
    func hasPermission() -> Bool
}

public protocol WorkspaceActivationObserving: AnyObject {
    func start(activationHandler: @escaping (RunningApplication) -> Void) -> RunningApplication?
    func stop()
}

public protocol AccessibilityObserving: AnyObject {
    /// Throws when the application cannot be observed at all.
    func start(
        observing application: RunningApplication,
        activityHandler: @escaping (FocusedActivity) -> Void,
        errorHandler: @escaping (CollectionError) -> Void
    ) throws -> AccessibilityRegistration
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
    private let metadataProvider: any DeclaredAppMetadataReading
    private let now: () -> Date
    private let maximumDwellDuration: TimeInterval
    private let statusSubject = CurrentValueSubject<CollectionStatus, Never>(.idle)
    private let reportStatusTransition: (CollectionStatus, CollectionStatus) -> Void
    private let lock = NSLock()
    private var isRunningLocked = false
    /// The application the AX observer is registered for, if any.
    private var activeProcessIdentifier: pid_t?
    /// The application the workspace last reported in front, whether or not
    /// the AX observer could be registered for it. The open dwell is its own.
    private var frontmostApplication: RunningApplication?
    /// Whether the registration for `activeProcessIdentifier` reaches a
    /// window. False while it waits for one, and while nothing is registered.
    private var observesWindow = false
    private var pendingDwellEvent: RawEvent?
    /// Guards `publishedStatus` and every send, and nothing else. Activations
    /// report on the main thread and AX observer failures on the callback
    /// queue, so without it two reports could reach subscribers in the
    /// opposite order to the one they were decided in.
    private let statusLock = NSLock()
    private var publishedStatus: CollectionStatus = .idle

    public var isRunning: Bool { lock.withLock { isRunningLocked } }

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
        metadataProvider: any DeclaredAppMetadataReading = BundleInfoPlistMetadataProvider(),
        now: @escaping () -> Date = Date.init,
        maximumDwellDuration: TimeInterval = 30 * 60,
        reportStatusTransition: @escaping (_ from: CollectionStatus, _ to: CollectionStatus) -> Void =
            CollectionStatusLog.report
    ) {
        self.eventSink = eventSink
        self.permissionChecker = permissionChecker
        self.workspaceObserver = workspaceObserver
        self.accessibilityObserver = accessibilityObserver
        self.metadataProvider = metadataProvider
        self.now = now
        self.maximumDwellDuration = maximumDwellDuration
        self.reportStatusTransition = reportStatusTransition
    }

    public func start() throws {
        guard lock.withLock({ !isRunningLocked }) else {
            return
        }
        guard permissionChecker.hasPermission() else {
            publish(.permissionRevoked)
            throw CollectionError.permissionRevoked
        }
        let shouldStart = lock.withLock {
            guard !isRunningLocked else {
                return false
            }
            isRunningLocked = true
            return true
        }
        guard shouldStart else {
            return
        }

        let currentApplication = workspaceObserver.start { [weak self] application in
            self?.applicationDidActivate(application)
        }
        publish(.running)
        if let currentApplication {
            observeReportingFailure(currentApplication, activatedAt: now())
        }
    }

    public func stop() {
        let result = lock.withLock { () -> (shouldStop: Bool, finalEvent: RawEvent?) in
            guard isRunningLocked else {
                return (false, nil)
            }
            isRunningLocked = false
            clearApplicationLocked()
            let finalEvent = takePendingDwellLocked(at: now(), reanchor: false)
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
        publish(.idle)
    }

    /// Emits the dwell that is still in progress, carrying only the duration
    /// measured up to `instant`, and re-opens the same activity at `instant`
    /// so the remainder is still measured and emitted when the user actually
    /// switches away.
    ///
    /// Splitting one dwell this way conserves time exactly. The flushed span
    /// `[start, instant]` and the later span `[instant, switch]` abut and do
    /// not overlap, so no second of activity is counted twice and none is
    /// dropped. A flush with nothing measured yet emits nothing, so repeated
    /// calls at the same instant are idempotent.
    ///
    /// This is not a scheduled activity check and does not make the module
    /// non-event-driven: it queries neither the Accessibility API nor the
    /// workspace, and it reports only an observation the agent has already
    /// made. The caller supplies `instant`; the agent never wakes itself.
    ///
    /// **Not yet wired to work-block end, deliberately.** Two facts block it,
    /// both verified against the service on 2026-08-21:
    ///
    /// 1. The only block-end signal Swift receives is the `work_block_state`
    ///    snapshot the deadline scheduler pushes *after* it has already run
    ///    `finish` (`work_block/mod.rs:1289`). By then the block reads
    ///    `completed`, so a flush sent on that signal is discarded by the
    ///    phase guard at `work_block/mod.rs:519` and changes nothing.
    /// 2. Firing earlier — off the snapshot's `ends_at` — would land the event
    ///    while the block is still active and would fix the ledger, but the
    ///    service also runs `evaluate_drift` on every observation
    ///    (`work_block/mod.rs:563`) and can push an OS notification from it
    ///    (`ipc/router.rs:1507`). The gates read the flushed event's
    ///    `occurred_at`, which is the dwell's start, so a long terminal dwell
    ///    can clear them and interrupt the user seconds before their block
    ///    ends. Suppressing that needs a field the v28 protocol does not have.
    ///
    /// - Returns: `true` when an event was emitted.
    @discardableResult
    public func flushPendingDwell(at instant: Date) -> Bool {
        let completedEvent = lock.withLock { () -> RawEvent? in
            guard isRunningLocked, let pending = pendingDwellEvent else {
                return nil
            }
            guard dwellSeconds(from: pending.occurredAt, through: instant) > 0 else {
                return nil
            }
            return takePendingDwellLocked(at: instant, reanchor: true)
        }
        guard let completedEvent else {
            return false
        }
        eventSink?.receive(completedEvent)
        return true
    }

    /// Closes the in-progress dwell at `instant` and returns the event to
    /// deliver, or `nil` when none is open.
    ///
    /// When `reanchor` is true the same activity is re-opened at `instant`,
    /// so collection continues and the remaining time is measured against
    /// the new anchor. When it is false the dwell is discarded, which is
    /// what the teardown paths want: collection is ending, so there is no
    /// remainder to measure.
    ///
    /// The caller must already hold `lock`. `instant` is an autoclosure so the
    /// teardown paths, which pass `now()`, still read the clock only when a
    /// dwell is actually open.
    private func takePendingDwellLocked(
        at instant: @autoclosure () -> Date,
        reanchor: Bool
    ) -> RawEvent? {
        guard let pending = pendingDwellEvent else {
            return nil
        }
        let closedAt = instant()
        pendingDwellEvent = reanchor ? pending.reanchored(at: closedAt) : nil
        return pending.withDuration(
            seconds: dwellSeconds(from: pending.occurredAt, through: closedAt)
        )
    }

    deinit {
        stop()
    }

    private func applicationDidActivate(_ application: RunningApplication) {
        guard lock.withLock({ isRunningLocked }) else {
            return
        }
        guard permissionChecker.hasPermission() else {
            stopAfterPermissionRevocation()
            return
        }
        // Only an application already observed at window level has nothing
        // left to register. Another activation of one that is not, including
        // the one already in front, is a retry.
        guard
            lock.withLock({
                !(activeProcessIdentifier == application.processIdentifier && observesWindow)
            })
        else {
            return
        }
        observeReportingFailure(application, activatedAt: now())
    }

    /// Registers for `application` and reports how that went.
    ///
    /// Whatever the registration finds, the application activated at
    /// `activatedAt` has its own dwell from that instant: at window level when
    /// the observer reached a window, at application level when it did not.
    ///
    /// A registration that fails for one application leaves the workspace
    /// observer running, so it is `.limited`, not a stop, and the next one that
    /// reaches a window is `.running` again. Only `start()` used to report
    /// `.running`, so a single application that could not be observed left the
    /// status on an error for the rest of the session.
    private func observeReportingFailure(_ application: RunningApplication, activatedAt: Date) {
        do {
            switch try observe(application, activatedAt: activatedAt) {
            case .window:
                publish(.running, observing: application.processIdentifier)
            case .awaitingWindow:
                publish(
                    .limited("ax_observer_registration_failed:\(AXError.noValue.rawValue)"),
                    observing: application.processIdentifier
                )
            }
        } catch CollectionError.permissionRevoked {
            stopAfterPermissionRevocation()
        } catch CollectionError.observerRegistrationFailed(let code) {
            beginApplicationDwell(application, at: activatedAt)
            publish(.limited("ax_observer_registration_failed:\(code)"))
        } catch {
            beginApplicationDwell(application, at: activatedAt)
            publish(.limited("ax_observer_registration_failed"))
        }
    }

    /// Publishes `status` and logs the transition, once, when it differs from
    /// the status last published.
    ///
    /// A collecting status is dropped once the agent has stopped. An
    /// activation on the main thread can finish registering after the callback
    /// queue has already stopped the agent for a revoked permission, and its
    /// `.running` must not paper over that stop.
    ///
    /// `processIdentifier` names the application a registration's report is
    /// about. The report is dropped once that registration no longer says what
    /// it did: the application is no longer the one observed, or its window
    /// was reached (for `.limited`) or lost (for `.running`) since. Its observer
    /// can fail, or reach its first window, on the callback queue before the
    /// activation that registered it reports, and that is the newer fact.
    /// Checked under `statusLock`, so in either order the newer report is the
    /// one left standing.
    ///
    /// Never called with `lock` held: it takes it.
    private func publish(_ status: CollectionStatus, observing processIdentifier: pid_t? = nil) {
        statusLock.withLock {
            guard !status.isCollecting || isStillCurrent(status, observing: processIdentifier) else {
                return
            }
            let previous = publishedStatus
            guard status != previous else {
                return
            }
            publishedStatus = status
            statusSubject.send(status)
            reportStatusTransition(previous, status)
        }
    }

    private func isStillCurrent(_ status: CollectionStatus, observing processIdentifier: pid_t?) -> Bool {
        lock.withLock {
            guard isRunningLocked else {
                return false
            }
            guard let processIdentifier else {
                return true
            }
            return activeProcessIdentifier == processIdentifier && observesWindow == (status == .running)
        }
    }

    private func observe(
        _ application: RunningApplication,
        activatedAt: Date
    ) throws -> AccessibilityRegistration {
        accessibilityObserver.stop()
        lock.withLock {
            activeProcessIdentifier = application.processIdentifier
            frontmostApplication = application
            observesWindow = false
        }
        let registration: AccessibilityRegistration
        do {
            registration = try accessibilityObserver.start(
                observing: application,
                activityHandler: { [weak self] activity in
                    self?.emit(application: application, activity: activity)
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
        switch registration {
        case .window(let activity):
            emit(application: application, activity: activity, at: activatedAt)
        case .awaitingWindow:
            beginApplicationDwell(application, at: activatedAt)
        }
        return registration
    }

    /// Reports window-level activity for `application`: at `instant` for the
    /// window a registration found, at `now()` for a later notification.
    private func emit(application: RunningApplication, activity: FocusedActivity, at instant: Date? = nil) {
        guard permissionChecker.hasPermission() else {
            stopAfterPermissionRevocation()
            return
        }
        // Read before the lock is taken, like `now()`: the provider caches per
        // bundle identifier, so this is a dictionary lookup on every event after
        // an application's first, and no file is touched while the lock is held.
        let declared = metadataProvider.metadata(for: application)
        let nextEvent = RawEvent(
            appName: application.appName,
            bundleIdentifier: application.bundleIdentifier,
            declaredAppCategory: declared.declaredAppCategory,
            documentTypeIDs: declared.documentTypeIDs,
            windowTitle: activity.windowTitle ?? "",
            focusedDocumentURL: activity.focusedDocumentURL,
            occurredAt: instant ?? now()
        )
        let outcome = lock.withLock { () -> (report: DwellReport, reachedWindow: Bool)? in
            guard isRunningLocked && activeProcessIdentifier == application.processIdentifier else {
                return nil
            }
            let reachedWindow = !observesWindow
            observesWindow = true
            return (replacePendingDwellLocked(with: nextEvent), reachedWindow)
        }
        guard let outcome else {
            return
        }
        deliver(outcome.report)
        // The first window of an application that had none when it was
        // activated: the observer registered for it has now reached one.
        if outcome.reachedWindow {
            publish(.running, observing: application.processIdentifier)
        }
    }

    /// Opens a dwell for `application` from what the workspace reports about
    /// it: its name, its bundle identifier and what it declares about itself.
    /// No window title, because none could be read, and none is made up.
    ///
    /// For an application in front that cannot be observed at window level.
    /// Nothing used to be reported for one: the dwell before it stayed open
    /// and absorbed its time at the next switch, so the time went to the wrong
    /// application, and a departure to it never reached the drift gate. The
    /// workspace always knows which application is in front; only the window
    /// needs the AX observer.
    ///
    /// A no-op once the application is observed at window level, so a window
    /// the observer reached first is never replaced by less.
    private func beginApplicationDwell(_ application: RunningApplication, at instant: Date) {
        let declared = metadataProvider.metadata(for: application)
        let event = RawEvent(
            appName: application.appName,
            bundleIdentifier: application.bundleIdentifier,
            declaredAppCategory: declared.declaredAppCategory,
            documentTypeIDs: declared.documentTypeIDs,
            windowTitle: "",
            focusedDocumentURL: nil,
            occurredAt: instant
        )
        let report = lock.withLock { () -> DwellReport in
            guard
                isRunningLocked,
                frontmostApplication?.processIdentifier == application.processIdentifier,
                !observesWindow
            else {
                return DwellReport()
            }
            return replacePendingDwellLocked(with: event)
        }
        deliver(report)
    }

    /// The dwell a change of activity closed, and the one it began.
    private struct DwellReport {
        var closed: RawEvent?
        var began: RawEvent?
    }

    /// Makes `next` the open dwell unless it is the activity already open.
    ///
    /// The caller must already hold `lock`.
    private func replacePendingDwellLocked(with next: RawEvent) -> DwellReport {
        guard let previous = pendingDwellEvent else {
            pendingDwellEvent = next
            return DwellReport(closed: nil, began: next)
        }
        // Declared metadata is deliberately absent from this comparison. It
        // is a property of the application, not of the activity, so it
        // cannot distinguish two dwells the four identity fields agree on —
        // and making it part of activity identity would let a first-read
        // failure that later succeeds register as a switch the user never
        // made.
        guard
            previous.appName != next.appName
                || previous.bundleIdentifier != next.bundleIdentifier
                || previous.windowTitle != next.windowTitle
                || previous.focusedDocumentURL != next.focusedDocumentURL
        else {
            return DwellReport()
        }
        // Never before the dwell it closes, so spans abut and never overlap: a
        // registration's first window can be reported after an AX callback for
        // the same application already opened a later one.
        let next = next.occurredAt < previous.occurredAt ? next.reanchored(at: previous.occurredAt) : next
        pendingDwellEvent = next
        return DwellReport(
            closed: previous.withDuration(
                seconds: dwellSeconds(from: previous.occurredAt, through: next.occurredAt)),
            began: next
        )
    }

    private func deliver(_ report: DwellReport) {
        if let closed = report.closed {
            eventSink?.receive(closed)
        }
        // After the dwell it replaces, never before. The service reads the
        // two in order: the closed report lands on the row its own in-progress
        // report opened, and only then does the new activity open the next.
        // Reported at the start because that is when a departure is still
        // true; reported only when it ended, a departure reached the drift
        // gate at the moment the person came back.
        if let began = report.began {
            eventSink?.activityBegan(began)
        }
    }

    private func accessibilityObserverFailed(_ error: CollectionError, processIdentifier: pid_t) {
        if error == .permissionRevoked {
            stopAfterPermissionRevocation()
            return
        }
        let application = lock.withLock { () -> RunningApplication? in
            guard isRunningLocked && activeProcessIdentifier == processIdentifier else {
                return nil
            }
            activeProcessIdentifier = nil
            observesWindow = false
            return frontmostApplication
        }
        guard let application else {
            return
        }
        accessibilityObserver.stop()
        // The observer for this application is gone, but the application is
        // still in front and the workspace observer still running. Its window
        // can no longer be read, so the rest of its time is an application-level
        // dwell until the next activation, which registers afresh because
        // `activeProcessIdentifier` no longer matches anything.
        beginApplicationDwell(application, at: now())
        if case .observerRegistrationFailed(let code) = error {
            publish(.limited("ax_observer_failed:\(code)"))
        } else {
            publish(.limited("ax_observer_failed"))
        }
    }

    private func stopAfterPermissionRevocation() {
        let result = lock.withLock { () -> (shouldStop: Bool, finalEvent: RawEvent?) in
            guard isRunningLocked else {
                return (false, nil)
            }
            isRunningLocked = false
            clearApplicationLocked()
            let finalEvent = takePendingDwellLocked(at: now(), reanchor: false)
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
        publish(.permissionRevoked)
    }

    /// The caller must already hold `lock`.
    private func clearApplicationLocked() {
        activeProcessIdentifier = nil
        frontmostApplication = nil
        observesWindow = false
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
    public private(set) var isRunning = false

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
            bundleIdentifier: application.bundleIdentifier,
            bundleURL: application.bundleURL
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
    ) throws -> AccessibilityRegistration {
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
        // An application can be in front with no focused or main window. It is
        // registered all the same: the focused-window notification on the
        // application element is what reports its first window, without
        // polling. This used to throw `kAXErrorNoValue` instead, and nothing
        // was observed until the next activation.
        let initialWindow =
            copyElement(attribute: kAXFocusedWindowAttribute, from: applicationElement)
            ?? copyElement(attribute: kAXMainWindowAttribute, from: applicationElement)
        try addNotification(kAXFocusedWindowChangedNotification, to: applicationElement, of: createdObserver)
        if let initialWindow {
            try addNotification(kAXTitleChangedNotification, to: initialWindow, of: createdObserver)
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
        guard let initialWindow else {
            return .awaitingWindow
        }
        return .window(snapshot(applicationElement: applicationElement, window: initialWindow))
    }

    private func addNotification(_ notification: String, to element: AXUIElement, of observer: AXObserver) throws {
        let registration = AXObserverAddNotification(
            observer,
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

    private static let callback: AXObserverCallback = { _, _, notification, context in
        guard let context else {
            return
        }
        // The context is safe because AXApplicationObserver owns the AXObserver
        // and removes its run-loop source before the controller can deallocate.
        let controller = Unmanaged<AXApplicationObserver>.fromOpaque(context).takeUnretainedValue()
        controller.handle(notification: notification as String)
    }

    private func handle(notification: String) {
        // AX callbacks run on a private CFRunLoop. Delivery crosses explicitly
        // onto a serial dispatch queue; no AXUIElement leaves the callback.
        do {
            guard let activity = try refreshSnapshot(notification: notification) else {
                return
            }
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

    /// The focused window's activity, or `nil` while the application has no
    /// window to describe.
    private func refreshSnapshot(notification: String) throws -> FocusedActivity? {
        let resources = lock.withLock { (observer, applicationElement, focusedWindow) }
        guard let applicationElement = resources.1 else {
            throw CollectionError.observerRegistrationFailed(code: AXError.invalidUIElement.rawValue)
        }
        var window = resources.2
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
        // Until a window has gained focus there is nothing to describe. The
        // element a notification names may then be the application itself or
        // a control inside it, and its title is not a window title.
        guard let window else {
            return nil
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
        for element in [window, copyElement(attribute: kAXFocusedUIElementAttribute, from: applicationElement)]
            .compactMap({ $0 })
        {
            var candidate: AXUIElement? = element
            for _ in 0..<5 {
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
        "com.kagi.kagimacOS",
    ]

    static func isSupportedBrowser(bundleIdentifier: String?) -> Bool {
        guard let bundleIdentifier else { return false }
        if browserBundleIdentifiers.contains(bundleIdentifier) { return true }
        return [
            "com.google.Chrome.",
            "com.microsoft.edgemac.",
            "com.brave.Browser.",
            "org.mozilla.firefox.",
        ].contains { bundleIdentifier.hasPrefix($0) }
    }

    private static let optionalWindowNotifications = [
        kAXValueChangedNotification,
        kAXSelectedChildrenChangedNotification,
        kAXSelectedRowsChangedNotification,
    ]

    private func addOptionalBrowserNotifications(
        observer: AXObserver,
        applicationElement: AXUIElement,
        window: AXUIElement?,
        bundleIdentifier: String?
    ) {
        guard Self.isSupportedBrowser(bundleIdentifier: bundleIdentifier) else { return }
        _ = AXObserverAddNotification(
            observer,
            applicationElement,
            kAXFocusedUIElementChangedNotification as CFString,
            Unmanaged.passUnretained(self).toOpaque()
        )
        // Without a window yet, the focused-window change adds these to the
        // first one that gains focus.
        guard let window else { return }
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
        guard case .success(let title) = copyTitleResult(from: element) else {
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
