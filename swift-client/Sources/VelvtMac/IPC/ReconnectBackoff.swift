import Foundation

/// Computes capped exponential reconnect delays with injectable jitter.
public struct ReconnectBackoff: Sendable {
    private let maximumDelay: TimeInterval
    private let jitter: @Sendable () -> Double

    public init(maximumDelay: TimeInterval = 60, jitter: @escaping @Sendable () -> Double = { Double.random(in: 0.8 ... 1.2) }) {
        self.maximumDelay = maximumDelay
        self.jitter = jitter
    }

    public func delay(forAttempt attempt: Int) -> TimeInterval {
        let exponent = max(0, min(attempt - 1, 62))
        return min(pow(2, Double(exponent)), maximumDelay) * jitter()
    }
}
