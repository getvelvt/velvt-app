import Foundation

/// Delivery module - owns receipt, local caching, and notification scheduling
/// for ready-to-display payloads. Does NOT generate insight text, perform
/// analytics, abstract events, or call cloud APIs.

/// Receives ready-to-display payloads from IPC.
public protocol InsightReceiving: AnyObject {
    func receive(insight: InsightPayload) async throws
    func receive(history: HistoryPayload) async throws
}

/// Caches at most seven days of ready-to-display payloads.
public protocol InsightCaching: AnyObject {
    func store(insight: InsightPayload) async throws
    func store(history: HistoryPayload) async throws
    func recentHistory() async throws -> HistoryPayload?
}

/// Schedules notifications using text already supplied by Rust.
public protocol NotificationScheduling: AnyObject {
    func requestAuthorization() async throws -> Bool
    func schedule(insight: InsightPayload) async throws
}

/// Safe delivery errors that never include insight text.
public enum DeliveryError: Error, Equatable {
    case cacheUnavailable
    case notificationAuthorizationDenied
    case notificationSchedulingFailed(code: Int)
}

