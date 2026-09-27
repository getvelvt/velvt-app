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
    /// The service withdrew the offer — the person came back, answered it, or
    /// the block ended — while the attempt was still waiting on the permission
    /// check, so nothing was posted. Not a fault: a banner for an offer that
    /// is no longer true would be worse than none. Until this was reported,
    /// it was the one way an attempt could end without a trace.
    case withdrawnBeforeDelivery
    /// Notifications were not authorised at the instant the offer was live.
    /// Not a product decision — the offer stays eligible for redelivery.
    case blockedByPermission(PermissionStatus)
    /// The notification centre rejected the request.
    case rejectedByNotificationCentre
    /// Velvt was the active app when the notification centre asked how to
    /// show a delivered notification, and the drift card was already in front
    /// of the person: listed in Notification Center, with no banner and no
    /// sound on top of the card that says the same thing.
    case listedBehindVisibleCard
    /// Velvt was the active app, and nothing of Velvt's on screen was showing
    /// the notification's content: shown as a banner with sound, exactly as it
    /// would be with Velvt in the background.
    case bannerWhileActive
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
/// subsystem the rest of the app already uses, category `NotificationDelivery`.
///
/// Every line is at a level the unified log persists. Successes and quiet
/// outcomes are `.notice` (`OSLogType.default`), not `.info`: an `.info` line
/// lives only in memory, so `log show` after the fact found nothing and a
/// delivered notification could not be told from one that was never
/// attempted. A blocked or rejected
/// delivery is `.error`, because a notification channel that silently
/// delivers nothing is a product outage — the offer resolves as `returned` or
/// `expired` and is recorded as delivered, which makes the primary outcome a
/// measurement of a nudge nobody received.
///
/// Lines carry an outcome, a surface and at most a permission status, all
/// fixed tokens, so they are logged public: there is no title, body, category
/// or timing evidence in them to redact.
public final class OSLogNotificationDeliveryReporter: NotificationDeliveryReporting, @unchecked Sendable {
    private static let log = Logger(subsystem: "com.velvt.mac", category: "NotificationDelivery")

    public init() {}

    public func report(_ outcome: NotificationDeliveryOutcome, surface: NotificationDeliverySurface) {
        let line = Self.line(for: outcome, surface: surface)
        Self.log.log(level: line.level, "\(line.message, privacy: .public)")
    }

    /// The level and text of the one line `outcome` is logged as.
    static func line(
        for outcome: NotificationDeliveryOutcome,
        surface: NotificationDeliverySurface
    ) -> (level: OSLogType, message: String) {
        let surfaceName = "surface=\(surface.rawValue)"
        switch outcome {
        case .delivered:
            return (.default, "notification_delivered \(surfaceName)")
        case .suppressedBySalience:
            return (.default, "notification_suppressed_by_salience \(surfaceName)")
        case .withdrawnBeforeDelivery:
            return (.default, "notification_withdrawn_before_delivery \(surfaceName)")
        case .blockedByPermission(let status):
            return (
                .error,
                "error_code=notification_permission_blocked \(surfaceName) status=\(name(of: status))"
            )
        case .rejectedByNotificationCentre:
            return (.error, "error_code=notification_centre_rejected \(surfaceName)")
        case .listedBehindVisibleCard:
            return (
                .default,
                "notification_presented_while_active \(surfaceName) presentation=list_behind_visible_card"
            )
        case .bannerWhileActive:
            return (.default, "notification_presented_while_active \(surfaceName) presentation=banner")
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
