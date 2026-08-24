import Foundation
import os

// MARK: - NotificationDeliverySurface

/// Which offer was being carried to the notification centre.
///
/// Only two surfaces ever reach `UNUserNotificationCenter`: the live drift
/// offer and the daily insight. The soft-restart next action and the
/// initiation invitation are in-app cards only — they are answered inside the
/// popover and never post a notification.
public enum NotificationDeliverySurface: String, Equatable, Sendable {
    case driftOffer = "drift_offer"
    case dailyInsight = "daily_insight"
}

// MARK: - NotificationDeliveryOutcome

/// What happened to one attempt to post a notification.
///
/// This exists because a delivery that never happens is otherwise
/// indistinguishable from one that did: the permission gate used to `return`
/// with no log, no counter and no state, so an installation that had never
/// once posted a notification looked, from inside the app, exactly like a
/// healthy one.
public enum NotificationDeliveryOutcome: Equatable, Sendable {
    /// The request reached `UNUserNotificationCenter.add(_:)` and was accepted.
    case delivered
    /// Deliberately not rung: reduced salience. A dismissal bought quiet, or
    /// Velvt's own quiet hours are in force. The in-app card still renders.
    case suppressedBySalience
    /// Notifications were not authorised at the instant the offer was live.
    /// Not a product decision — the offer stays eligible for redelivery.
    case blockedByPermission(PermissionStatus)
    /// The notification centre rejected the request.
    case rejectedByNotificationCentre
}

// MARK: - NotificationDeliveryReporting

/// Records the fate of a delivery attempt. Implementations must never receive
/// or emit notification content: an outcome and a surface carry no title,
/// body, category, or timing evidence.
public protocol NotificationDeliveryReporting: AnyObject, Sendable {
    func report(_ outcome: NotificationDeliveryOutcome, surface: NotificationDeliverySurface)
}

// MARK: - OSLogNotificationDeliveryReporter

/// Default reporter: one line per attempt on the unified log, under the same
/// subsystem the rest of the app already uses.
///
/// A blocked or rejected delivery logs at `.error`, because a notification
/// channel that silently delivers nothing is a product outage — the offer
/// resolves as `returned` or `expired` and is recorded as delivered, which
/// makes the primary outcome a measurement of a nudge nobody received.
public final class OSLogNotificationDeliveryReporter: NotificationDeliveryReporting, @unchecked Sendable {
    private static let log = Logger(subsystem: "com.velvt.mac", category: "NotificationDelivery")

    public init() {}

    public func report(_ outcome: NotificationDeliveryOutcome, surface: NotificationDeliverySurface) {
        let surfaceName = surface.rawValue
        switch outcome {
        case .delivered:
            Self.log.info("notification_delivered surface=\(surfaceName, privacy: .public)")
        case .suppressedBySalience:
            Self.log.info(
                "notification_suppressed_by_salience surface=\(surfaceName, privacy: .public)")
        case .blockedByPermission(let status):
            Self.log.error(
                """
                error_code=notification_permission_blocked \
                surface=\(surfaceName, privacy: .public) \
                status=\(Self.name(of: status), privacy: .public)
                """)
        case .rejectedByNotificationCentre:
            Self.log.error(
                "error_code=notification_centre_rejected surface=\(surfaceName, privacy: .public)")
        }
    }

    private static func name(of status: PermissionStatus) -> String {
        switch status {
        case .unknown: return "unknown"
        case .granted: return "granted"
        case .denied: return "denied"
        case .restricted: return "restricted"
        }
    }
}

// MARK: - RecordingNotificationDeliveryReporter

/// Test double capturing every reported outcome in order.
public final class RecordingNotificationDeliveryReporter: NotificationDeliveryReporting, @unchecked Sendable {
    public struct Entry: Equatable, Sendable {
        public let outcome: NotificationDeliveryOutcome
        public let surface: NotificationDeliverySurface
    }

    private var storage: [Entry] = []
    private let lock = NSLock()

    public init() {}

    public var entries: [Entry] {
        lock.withLock { storage }
    }

    public func report(_ outcome: NotificationDeliveryOutcome, surface: NotificationDeliverySurface) {
        lock.withLock { storage.append(Entry(outcome: outcome, surface: surface)) }
    }
}
