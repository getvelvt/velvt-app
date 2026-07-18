import Combine
import Foundation

public enum AuthenticationStatusIndicatorColor: Equatable {
    case red
    case green
}

public struct AuthenticationStatusPresentation: Equatable {
    public let text: String
    public let indicatorColor: AuthenticationStatusIndicatorColor

    public init(accountState: AccountState, email: String?) {
        switch accountState {
        case .loggedIn:
            text = email?.isEmpty == false ? email! : "Authenticated"
            indicatorColor = .green
        case .loggedOut, .loggingIn, .loggingOut, .pendingErasure:
            text = "Not Authenticated"
            indicatorColor = .red
        }
    }
}

public protocol AppMetricsCounting: AnyObject, Sendable {
    var actionsLogged: Int { get }
    var interventions: Int { get }
    func incrementActionsLogged()
    func incrementInterventions()
}

public final class AppMetricsStore: ObservableObject, AppMetricsCounting, @unchecked Sendable {
    @Published public private(set) var actionsLogged: Int
    @Published public private(set) var interventions: Int
    @Published public private(set) var isAuthenticated = false

    private enum Key {
        static let actionsLogged = "velvt.metrics.actions_logged"
        static let interventions = "velvt.metrics.interventions"
    }

    private let defaults: UserDefaults
    private let lock = NSLock()

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        actionsLogged = defaults.integer(forKey: Key.actionsLogged)
        interventions = defaults.integer(forKey: Key.interventions)
    }

    public func incrementActionsLogged() {
        increment(\.actionsLogged, key: Key.actionsLogged)
    }

    public func incrementInterventions() {
        increment(\.interventions, key: Key.interventions)
    }

    /// Keeps local diagnostics scoped to the authenticated account session.
    /// Leaving the account clears the counters so they cannot be shown to a
    /// logged-out user or carried into another account on the same Mac.
    public func setAuthenticated(_ authenticated: Bool) {
        lock.withLock {
            if !authenticated {
                defaults.removeObject(forKey: Key.actionsLogged)
                defaults.removeObject(forKey: Key.interventions)
                actionsLogged = 0
                interventions = 0
            }
            isAuthenticated = authenticated
        }
    }

    private func increment(_ keyPath: ReferenceWritableKeyPath<AppMetricsStore, Int>, key: String) {
        lock.withLock {
            let value = self[keyPath: keyPath] + 1
            defaults.set(value, forKey: key)
            self[keyPath: keyPath] = value
        }
    }
}
