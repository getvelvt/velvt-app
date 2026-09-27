import SwiftUI

/// The window header's second line: where this Mac's uploads stand.
///
/// Said calmly, because it is almost never urgent. Collection, the dashboard,
/// Patterns and work blocks all run on this Mac whether or not the server can
/// be reached, so a state that stops uploads says that first. The size of the
/// upload queue stays out of the header: "Working offline · 14632 queued"
/// read as an alarm about something the person can do nothing about. The
/// count, and what actually happens to events that never get through, is in
/// `detail`, which the header shows only when asked (a click or a hover).
/// App Info keeps the exact counts, retry times and error codes.
///
/// Every state is one the service reports. `MenuStatus.cloudReady` is the
/// service's `GET /v1/ready` probe and says nothing about the account, so the
/// account decides first; `uploadStatus` is `upload_status_for` in
/// `rust-service/src/ipc/router.rs`. While signed out the service queues
/// nothing new (`upload_eligible`), but batches queued before sign-out stay
/// and keep being retried.
public struct CloudSyncStatusPresentation: Equatable {
    public enum State: Equatable, CaseIterable {
        /// No account model: a preview or a test without a service behind it.
        case unavailable
        case signingIn
        case signingOut
        case deletingAccount
        /// Never signed in, or signed out. Nothing new is queued.
        case signedOut
        /// The server ended the session (`needs_reauth`).
        case signInAgain
        /// Signed in; the first `menu_status` has not arrived.
        case checking
        /// Signed in; the server's readiness probe did not answer.
        case offline
        /// Signed in and the server answers, but it refused this Mac's
        /// credentials on the last upload (`auth_required`).
        case uploadsPaused
        /// Signed in and the server answers, but the last upload failed or was
        /// rate limited, and the queue is waiting out its backoff.
        case backingOff
        /// Signed in, the server answers, and batches are waiting their turn.
        case syncing
        /// Signed in, the server answers, and nothing is waiting.
        case synced
    }

    public let state: State
    public let headline: String
    /// What the header adds on request. `nil` when the headline says it all.
    public let detail: String?

    /// What happens to an event that never uploads, in one sentence.
    ///
    /// `UPLOAD_BATCH_ATTEMPT_CEILING` (288, `rust-service/src/persistence/models.rs`)
    /// abandons a batch on its 288th failed attempt, and attempts run at the
    /// fifteen-minute backoff cap while the server stays away: about three days.
    /// An abandoned batch is never sent. The local views read the service's own
    /// event store, not the upload queue, so dropping a batch removes nothing
    /// the person can see on this Mac. Change this sentence with the ceiling.
    public static let dropRule =
        "Events that still can't upload after about three days of retries are dropped; "
        + "your history on this Mac is not affected."

    public init(
        accountState: AccountState?,
        requiresReauthentication: Bool,
        status: MenuStatus?,
        now: Date = Date(),
        locale: Locale = .current,
        timeZone: TimeZone = .current
    ) {
        let state = Self.state(
            accountState: accountState,
            requiresReauthentication: requiresReauthentication,
            status: status
        )
        self.state = state
        headline = Self.headline(for: state)

        let waiting = Self.waitingSentences(queuedEventCount: status?.queuedEventCount ?? 0, locale: locale)
        let lead: String?
        switch state {
        case .unavailable, .signingIn, .signingOut, .deletingAccount, .checking, .syncing, .synced:
            lead = nil
        case .signedOut:
            lead = "No activity is uploaded while you're signed out."
        case .signInAgain:
            lead = "Sign in again to resume uploads."
        case .offline:
            lead = "Uploads resume when Velvt can reach its server."
        case .uploadsPaused:
            // `BatchUploadError::AuthenticationRequired` reschedules the
            // batch fifteen minutes out (`upload/coordinator.rs`).
            lead = "The server didn't accept this Mac's sign-in on the last try. Velvt tries again every 15 minutes."
        case .backingOff:
            if let retryAt = status?.nextUploadAttemptAt, retryAt > now {
                let time = retryAt.formatted(
                    Date.FormatStyle(date: .omitted, time: .shortened, locale: locale, timeZone: timeZone)
                )
                lead = "Velvt tries again at \(time)."
            } else {
                lead = "Velvt tries again shortly."
            }
        }
        detail = lead.map { ([$0] + waiting).joined(separator: " ") }
    }

    static func state(
        accountState: AccountState?,
        requiresReauthentication: Bool,
        status: MenuStatus?
    ) -> State {
        guard let accountState else { return .unavailable }
        if requiresReauthentication { return .signInAgain }
        switch accountState {
        case .loggingIn: return .signingIn
        case .loggingOut: return .signingOut
        case .pendingErasure: return .deletingAccount
        case .loggedOut: return .signedOut
        case .loggedIn: break
        }
        guard let status else { return .checking }
        if !status.cloudReady { return .offline }
        // `last_upload_error_code`, which picks `auth_required` and
        // `rate_limited`, can come from a batch that has already been
        // abandoned. A paused queue with nothing in it is not paused.
        let hasWaitingBatches = status.pendingUploadBatchCount + status.failedUploadBatchCount > 0
        switch status.uploadStatus {
        case "auth_required":
            return hasWaitingBatches ? .uploadsPaused : .synced
        case "retrying", "rate_limited":
            return hasWaitingBatches ? .backingOff : .synced
        case "pending":
            return .syncing
        case "ready":
            return .synced
        default:
            // `privacy_rejected` is about one batch the server refused and
            // says nothing about the rest of the queue; the counts do.
            if status.failedUploadBatchCount > 0 { return .backingOff }
            if status.pendingUploadBatchCount > 0 { return .syncing }
            return .synced
        }
    }

    static func headline(for state: State) -> String {
        switch state {
        case .unavailable: "Sync status unavailable"
        case .signingIn: "Signing in…"
        case .signingOut: "Signing out…"
        case .deletingAccount: "Account deletion in progress"
        case .signedOut: "Local only · sign in to sync"
        case .signInAgain: "Signed out · everything local still works"
        case .checking: "Checking sync…"
        case .offline: "Offline · everything local still works"
        case .uploadsPaused: "Uploads paused · everything local still works"
        case .backingOff: "Uploads paused briefly · everything local still works"
        case .syncing: "Syncing…"
        case .synced: "Synced"
        }
    }

    private static func waitingSentences(queuedEventCount count: Int, locale: Locale) -> [String] {
        guard count > 0 else { return [] }
        let waiting =
            count == 1
            ? "1 event is waiting."
            : "\(count.formatted(.number.locale(locale))) events are waiting."
        return [waiting, dropRule]
    }
}

/// Draws a `CloudSyncStatusPresentation`: the headline always, the detail
/// only when the person clicks the headline (or hovers it, as a tooltip).
struct CloudSyncStatusLine: View {
    let presentation: CloudSyncStatusPresentation
    @Binding var isExpanded: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .trailing, spacing: 3) {
            if let detail = presentation.detail {
                Button(action: toggle) {
                    HStack(alignment: .firstTextBaseline, spacing: 4) {
                        headline
                        Image(systemName: "chevron.down")
                            .font(.system(size: 7, weight: .semibold))
                            .foregroundStyle(VelvtInk.tertiaryOnInk)
                            .rotationEffect(.degrees(isExpanded ? 180 : 0))
                            .accessibilityHidden(true)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(detail)
                .accessibilityLabel(presentation.headline)
                .accessibilityHint(isExpanded ? "Hides upload details" : "Shows upload details")
                if isExpanded {
                    Text(detail)
                        .font(VelvtType.caption(10))
                        .foregroundStyle(VelvtInk.tertiaryOnInk)
                        .multilineTextAlignment(.trailing)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: 280, alignment: .trailing)
                        .transition(.opacity)
                }
            } else {
                headline
            }
        }
        .preference(key: CloudSyncHeadlinePreferenceKey.self, value: presentation.headline)
    }

    private var headline: some View {
        Text(presentation.headline)
            .font(VelvtType.caption(10))
            .foregroundStyle(VelvtInk.secondaryOnInk)
            .multilineTextAlignment(.trailing)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func toggle() {
        let animation: Animation? =
            MenuBarMotionPolicy.shouldAnimate(reduceMotion: reduceMotion) ? .easeInOut(duration: 0.18) : nil
        withAnimation(animation) { isExpanded.toggle() }
    }
}

/// The headline the sync line last drew.
///
/// A test seam, and only that: SwiftUI builds no accessibility tree inside a
/// test process, so this is how a test reads what the header says after a
/// status arrives. Nothing in the app reads it.
struct CloudSyncHeadlinePreferenceKey: PreferenceKey {
    static let defaultValue: String? = nil

    static func reduce(value: inout String?, nextValue: () -> String?) {
        value = nextValue() ?? value
    }
}

/// The header's sync line, subscribed to the two models it reads.
///
/// `MenuBarPopoverView` holds `AccountStateManager` and `MenuStatusViewModel`
/// as plain optional `let`s, because an `@ObservedObject` cannot be optional,
/// so a line computed in its body redrew only when some unrelated observed
/// object happened to publish: the header could say "Checking…" or show a
/// sign-in state long after the service had answered. Taking both models
/// non-optionally here restores the subscription, as `CorrectionWorkbenchView`
/// does for the workbench.
struct ObservedCloudSyncStatusLine: View {
    @ObservedObject var accountStateManager: AccountStateManager
    @ObservedObject var menuStatus: MenuStatusViewModel
    @Binding var isExpanded: Bool

    var body: some View {
        CloudSyncStatusLine(
            presentation: CloudSyncStatusPresentation(
                accountState: accountStateManager.accountState,
                requiresReauthentication: accountStateManager.requiresReauthentication,
                status: menuStatus.status
            ),
            isExpanded: $isExpanded
        )
    }
}
