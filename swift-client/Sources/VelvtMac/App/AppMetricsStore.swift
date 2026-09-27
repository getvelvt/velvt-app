import Combine
import Foundation

public enum AuthenticationStatusIndicatorColor: Equatable {
    case red
    case green
}

public struct AuthenticationStatusPresentation: Equatable {
    public let text: String
    public let indicatorColor: AuthenticationStatusIndicatorColor

    public init(
        accountState: AccountState,
        email: String?,
        requiresReauthentication: Bool = false
    ) {
        if requiresReauthentication {
            text = "Sign in required"
            indicatorColor = .red
            return
        }
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

/// Local, count-only diagnostics surfaced in the menu bar.
///
/// **Threading contract.** Facts arrive from whichever queue observed them — the
/// Accessibility collection queue is the hot one — so the counters themselves live
/// behind `lock` and are never touched by SwiftUI. The `@Published` properties are
/// main-thread mirrors of that state: SwiftUI requires every publish on the main
/// actor, so off-main mutations hand the publish to the main queue instead of
/// writing the mirrors directly. The collection path itself never hops; it takes
/// the lock, persists, and returns.
///
/// Publish hops are coalesced behind `isPublishScheduled`: a burst of Accessibility
/// events enqueues at most one pending main-queue block (plus, at worst, one already
/// executing), and that block republishes the newest snapshot rather than a queued
/// per-event value. Nothing accumulates on the hot path.
public final class AppMetricsStore: ObservableObject, AppMetricsCounting, @unchecked Sendable {
    /// Main-thread mirror of `counters.actionsLogged`. Written only on the main thread.
    @Published public private(set) var actionsLogged: Int
    /// Main-thread mirror of `counters.interventions`. Written only on the main thread.
    @Published public private(set) var interventions: Int
    /// Main-thread mirror of `counters.isAuthenticated`. Written only on the main thread.
    @Published public private(set) var isAuthenticated = false

    private enum Key {
        static let actionsLogged = "velvt.metrics.actions_logged"
        static let interventions = "velvt.metrics.interventions"
    }

    /// Source of truth for the counters, guarded by `lock`.
    private struct Counters {
        var actionsLogged: Int
        var interventions: Int
        var isAuthenticated: Bool
    }

    private let defaults: UserDefaults
    private let lock = NSLock()
    /// Guarded by `lock`.
    private var counters: Counters
    /// Guarded by `lock`. True while a publish hop is already queued on the main
    /// queue, so concurrent increments coalesce into that one hop.
    private var isPublishScheduled = false

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let storedActions = defaults.integer(forKey: Key.actionsLogged)
        let storedInterventions = defaults.integer(forKey: Key.interventions)
        counters = Counters(
            actionsLogged: storedActions,
            interventions: storedInterventions,
            isAuthenticated: false
        )
        actionsLogged = storedActions
        interventions = storedInterventions
    }

    public func incrementActionsLogged() {
        lock.withLock {
            counters.actionsLogged += 1
            defaults.set(counters.actionsLogged, forKey: Key.actionsLogged)
        }
        publishLatest()
    }

    public func incrementInterventions() {
        lock.withLock {
            counters.interventions += 1
            defaults.set(counters.interventions, forKey: Key.interventions)
        }
        publishLatest()
    }

    /// Keeps local diagnostics scoped to the authenticated account session.
    /// Leaving the account clears the counters so they cannot be shown to a
    /// logged-out user or carried into another account on the same Mac.
    public func setAuthenticated(_ authenticated: Bool) {
        lock.withLock {
            if !authenticated {
                defaults.removeObject(forKey: Key.actionsLogged)
                defaults.removeObject(forKey: Key.interventions)
                counters.actionsLogged = 0
                counters.interventions = 0
            }
            counters.isAuthenticated = authenticated
        }
        publishLatest()
    }

    // MARK: - Publishing

    /// Mirrors the latest locked snapshot onto the `@Published` properties.
    ///
    /// On the main thread this is synchronous, so main-thread callers (and views
    /// reading straight after a main-thread update) observe the new value at once.
    /// Off the main thread the publish is handed to the main queue and coalesced.
    private func publishLatest() {
        if Thread.isMainThread {
            applyLatestSnapshotOnMain()
            return
        }
        let alreadyScheduled = lock.withLock { () -> Bool in
            let wasScheduled = isPublishScheduled
            isPublishScheduled = true
            return wasScheduled
        }
        guard !alreadyScheduled else { return }
        DispatchQueue.main.async { [weak self] in
            self?.applyLatestSnapshotOnMain()
        }
    }

    /// Must run on the main thread — these assignments publish into SwiftUI.
    /// Always reads the newest snapshot under the lock so a coalesced hop can
    /// never publish a value older than one already mirrored.
    private func applyLatestSnapshotOnMain() {
        assert(Thread.isMainThread, "AppMetricsStore must publish on the main thread")
        let snapshot = lock.withLock { () -> Counters in
            isPublishScheduled = false
            return counters
        }
        if actionsLogged != snapshot.actionsLogged {
            actionsLogged = snapshot.actionsLogged
        }
        if interventions != snapshot.interventions {
            interventions = snapshot.interventions
        }
        if isAuthenticated != snapshot.isAuthenticated {
            isAuthenticated = snapshot.isAuthenticated
        }
    }
}
