import AppKit
import Combine
import SwiftUI

public enum MenuBarAccountAction: Equatable {
    case authenticate(AuthViewModel.AuthMode)
    case logOut
    case deleteAccount
}

private struct HistoryWorkspaceView: View {
    @ObservedObject var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject var localDashboardCoordinator: LocalDashboardCoordinator

    var body: some View {
        YourWeekContentView(
            snapshot: localDashboardCoordinator.snapshot,
            historyAvailability: coordinator.historyAvailability,
            historyNotReadyReason: coordinator.historyNotReadyReason,
            historyViewModel: coordinator.historyViewModel
        )
        .onAppear { localDashboardCoordinator.refresh() }
    }
}

struct YourWeekContentView: View {
    /// The seven days, read from local evidence on this Mac.
    ///
    /// This chart was removed once, on the reasoning that "a list of patterns
    /// is a dashboard, and a dashboard is a tracker." That objection was aimed
    /// at the wrong thing. What made the old version a tracker was that it
    /// reported cloud daily summaries — a scoreboard arriving from elsewhere.
    /// `dailyActivity` is different in kind: Rust builds it from this
    /// machine's own observations, it never leaves the device, and it is the
    /// evidence behind the single pattern claim below rather than a substitute
    /// for one. Showing the week the claim was drawn from is what separates an
    /// observation from an assertion.
    ///
    /// It was also, in practice, being thrown away: this property arrived on
    /// every snapshot and was never read, while the tab rendered a cloud
    /// summary that has returned `no_data` for every day it has ever been
    /// asked about.
    let snapshot: LocalDashboardSnapshot?
    let historyAvailability: DeliveryAvailability
    var historyNotReadyReason: String? = nil
    @ObservedObject var historyViewModel: HistoryViewModel

    var body: some View {
        // No scroll view of its own: the workspace detail pane scrolls every
        // tab now, and nesting two scroll views made the inner one swallow
        // the wheel events that should have moved the outer one.
        VStack(alignment: .leading, spacing: 12) {
            LocalWeekActivityView(days: snapshot?.dailyActivity ?? [])
            WeekOverWeekCoachingView(
                availability: historyAvailability,
                notReadyReason: historyNotReadyReason,
                viewModel: historyViewModel
            )
        }
        .padding(12)
    }
}

/// Seven local days, drawn from `LocalDashboardSnapshot.dailyActivity`.
///
/// Rust builds exactly `DAILY_ACTIVITY_DAYS` rows per request
/// (`dashboard.rs:387`) and the shaper pins the count, so the row count is the
/// service's to decide and this view renders whatever it is handed rather than
/// padding or truncating to a number of its own.
struct LocalWeekActivityView: View {
    let days: [LocalDailyActivityDay]

    /// Time per category across the whole window. Segments arrive bucketed per
    /// app — one row per `(stable_id, category)` — so several can share a
    /// category, and a bar coloured per segment would give one category two
    /// colours in a single day.
    private var secondsByCategory: [String: Int] {
        days.reduce(into: [String: Int]()) { totals, day in
            for segment in day.segments where segment.durationSeconds > 0 {
                totals[segment.category, default: 0] += segment.durationSeconds
            }
        }
    }

    private var palette: [String: Color] {
        ActivityPalette.assign(forSecondsByCategory: secondsByCategory)
    }

    private var hasAnyActivity: Bool {
        days.contains { $0.activeSeconds > 0 }
    }

    /// One slice per category for a single day, widest first.
    ///
    /// Segments arrive bucketed per app — one per `(stable_id, category)` — so
    /// several can carry the same category. Drawing them unmerged puts two
    /// slices of one colour side by side, which reads as one slice whose width
    /// disagrees with the hover text under the pointer.
    static func slices(for day: LocalDailyActivityDay) -> [(category: String, seconds: Int)] {
        let totals = day.segments.reduce(into: [String: Int]()) { totals, segment in
            guard segment.durationSeconds > 0 else { return }
            totals[segment.category, default: 0] += segment.durationSeconds
        }
        return ActivityPalette.rank(totals).map { (category: $0.key, seconds: $0.value) }
    }

    /// Spoken as one row: the same three facts the sighted row carries, in the
    /// same order.
    static func accessibilityLabel(for day: LocalDailyActivityDay) -> String {
        let date = DaySummaryViewModel.formatDate(day.date)
        guard day.activeSeconds > 0 else { return "\(date), no observed activity" }
        let time = DaySummaryViewModel.formatActiveTime(day.activeSeconds)
        guard let top = slices(for: day).first else { return "\(date), \(time) observed" }
        return "\(date), \(time) observed, mostly \(localCategoryLabel(top.category))"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Daily Activity")
                        .font(.headline)
                        .foregroundStyle(Color.velvtText)
                    // Names the source, because the honest thing about this
                    // chart is where it comes from.
                    Text("Observed on this Mac")
                        .font(.caption2)
                        .foregroundStyle(Color.velvtMuted)
                        .lineLimit(1)
                }
                Spacer()
                Text("\(days.count) days")
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
            }

            if days.isEmpty {
                Text("Waiting for the first local observation.")
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                    .padding(.top, 2)
            } else {
                VStack(spacing: 3) {
                    ForEach(days) { day in
                        LocalDayActivityRow(day: day, palette: palette)
                    }
                }
                if hasAnyActivity {
                    LocalActivityLegend(
                        entries: ActivityPalette.rank(secondsByCategory).compactMap { entry in
                            palette[entry.key].map {
                                (category: entry.key, color: $0, seconds: entry.value)
                            }
                        }
                    )
                    .padding(.top, 2)
                }
            }
        }
        .padding(10)
        .background(Color.velvtPanel)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Daily activity observed on this Mac")
    }
}

private struct LocalDayActivityRow: View {
    let day: LocalDailyActivityDay
    let palette: [String: Color]

    private var isEmpty: Bool { day.activeSeconds == 0 }

    var body: some View {
        HStack(spacing: 8) {
            Text(DaySummaryViewModel.formatDate(day.date))
                .font(.caption.bold())
                .foregroundStyle(isEmpty ? Color.velvtMuted.opacity(0.5) : Color.velvtText)
                .lineLimit(1)
                .frame(width: 62, alignment: .leading)

            LocalSplitActivityBar(day: day, palette: palette)
                .frame(height: 9)
                .frame(maxWidth: .infinity)

            Text(isEmpty ? "—" : DaySummaryViewModel.formatActiveTime(day.activeSeconds))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(Color.velvtMuted)
                .frame(width: 48, alignment: .trailing)
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(LocalWeekActivityView.accessibilityLabel(for: day))
    }
}

/// Per-slice detail is a native `.help` tooltip rather than a pointer-entered
/// callback that rewrites a label elsewhere. This file is asserted against the
/// literal name of that callback, because a row acting on the pointer merely
/// crossing it is the defect the guard exists for. A tooltip says the same
/// sentence without reopening that door.
private struct LocalSplitActivityBar: View {
    let day: LocalDailyActivityDay
    let palette: [String: Color]

    private var slices: [(category: String, seconds: Int)] {
        LocalWeekActivityView.slices(for: day)
    }

    private var total: Int {
        max(slices.reduce(0) { $0 + $1.seconds }, 1)
    }

    var body: some View {
        GeometryReader { proxy in
            HStack(spacing: 2) {
                if slices.isEmpty {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(Color.white.opacity(day.activeSeconds == 0 ? 0.06 : 0.12))
                        .help(emptyHelpText)
                } else {
                    ForEach(slices, id: \.category) { slice in
                        let text = helpText(for: slice)
                        RoundedRectangle(cornerRadius: 3)
                            .fill(palette[slice.category] ?? ActivityPalette.unmatched)
                            .frame(
                                width: max(
                                    5,
                                    proxy.size.width * CGFloat(slice.seconds) / CGFloat(total)))
                            // `.help` is delivered through the accessibility
                            // tree, so hiding the slice from that tree silences
                            // the tooltip along with it. The row above already
                            // sets an explicit combined label, so nothing here
                            // is announced twice.
                            .help(text)
                    }
                }
            }
        }
    }

    private var emptyHelpText: String {
        day.activeSeconds == 0
            ? "Nothing observed on this day."
            : "Observed activity on this day is not classified yet."
    }

    private func helpText(for slice: (category: String, seconds: Int)) -> String {
        let percent = Int((Double(slice.seconds) / Double(total) * 100).rounded())
        return
            "\(localCategoryLabel(slice.category)): \(DaySummaryViewModel.formatActiveTime(slice.seconds)), \(percent)% of observed time."
    }
}

/// Names the colours and states the totals behind them.
///
/// Carrying the time here rather than only in a tooltip is deliberate: a number
/// a person has to discover by hovering a nine-point bar is a number most people
/// never see, and the totals are the part of this chart that is actually a
/// claim. The tooltip still gives the per-day split.
private struct LocalActivityLegend: View {
    let entries: [(category: String, color: Color, seconds: Int)]

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 10) { chips }
            VStack(alignment: .leading, spacing: 3) { chips }
        }
    }

    @ViewBuilder
    private var chips: some View {
        ForEach(entries.prefix(5), id: \.category) { entry in
            HStack(spacing: 4) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(entry.color)
                    .frame(width: 7, height: 7)
                Text(localCategoryLabel(entry.category))
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
                    .lineLimit(1)
                Text(DaySummaryViewModel.formatActiveTime(entry.seconds))
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(Color.velvtText)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(
                "\(localCategoryLabel(entry.category)), \(DaySummaryViewModel.formatActiveTime(entry.seconds)) across the window"
            )
        }
    }
}

/// `FOCUS_WORK` is a wire value, not a word. This is the only transform
/// applied to it — no renaming, no grouping, no editorialising.
func localCategoryLabel(_ category: String) -> String {
    category.replacingOccurrences(of: "_", with: " ").lowercased().capitalized
}

struct WeekOverWeekCoachingView: View {
    let availability: DeliveryAvailability
    /// Why history is unavailable, when the service said why. Rust
    /// distinguishes an unreachable backend from an empty week; without this
    /// the tab answered both with advice to keep working, which tells someone
    /// whose network failed that the fault is their work habits.
    var notReadyReason: String? = nil
    @ObservedObject var viewModel: HistoryViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Label(
                    viewModel.progressiveInsight?.tier.label ?? "Progressive insights",
                    systemImage: "chart.line.uptrend.xyaxis"
                )
                    .font(.caption.bold())
                    .foregroundStyle(Color.velvtPink)
                Spacer()
                if let insight = viewModel.progressiveInsight {
                    Text(insight.confidenceSummary)
                        .font(.caption2)
                        .foregroundStyle(Color.velvtMuted)
                }
            }

            if let insight = viewModel.progressiveInsight {
                coachingLine("Observation", insight.observation)
                coachingLine("Comparison", insight.comparison)
                coachingLine("Try next", insight.suggestedAction)
                Text(insight.evidenceSummary)
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted)
                    .fixedSize(horizontal: false, vertical: true)
            } else if availability == .notGenerated {
                coachingPlaceholder(
                    notReadyReason == "backend_unavailable"
                        ? "Daily summaries could not be reached just now. Local collection is unaffected and this will catch up on its own."
                        : "No observed day is ready yet. Keep Velvt running during a normal work block."
                )
            } else if availability == .loading || viewModel.isLoading {
                coachingPlaceholder(
                    "Loading privacy-safe daily coverage."
                )
            } else {
                coachingPlaceholder(
                    "No qualifying activity is available yet."
                )
            }
        }
        .padding(10)
        .background(Color.velvtPanel)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(viewModel.progressiveInsight?.tier.label ?? "Progressive insights")
    }

    private func coachingLine(_ label: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label)
                .font(.caption2.bold())
                .foregroundStyle(Color.velvtText)
            Text(text)
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func coachingPlaceholder(_ text: String) -> some View {
        Text(text)
            .font(.caption2)
            .foregroundStyle(Color.velvtMuted)
            .fixedSize(horizontal: false, vertical: true)
    }
}

public enum MenuBarAccountActionResolver {
    public static func actions(for accountState: AccountState) -> [MenuBarAccountAction] {
        switch accountState {
        case .loggedOut: return [.authenticate(.logIn), .authenticate(.signUp)]
        case .loggedIn: return [.logOut, .deleteAccount]
        case .loggingIn, .loggingOut, .pendingErasure: return []
        }
    }
}

@MainActor
public final class ServiceConnectionStatusModel: ObservableObject {
    @Published public private(set) var status: ConnectionStatus = .disconnected
    @Published public private(set) var phase: LocalServiceConnectionPhase = .starting
    private var cancellable: AnyCancellable?
    private var graceTimer: AnyCancellable?
    private var lifecycleCancellables = Set<AnyCancellable>()
    private let scheduler: any ConnectionGraceScheduling
    private let graceInterval: TimeInterval
    private var hasConfirmedHandshake = false
    private var isWaking = false

    public init(
        connectionStatus: AnyPublisher<ConnectionStatus, Never>,
        scheduler: (any ConnectionGraceScheduling)? = nil,
        graceInterval: TimeInterval = 4,
        workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter
    ) {
        self.scheduler = scheduler ?? DispatchConnectionGraceScheduler()
        self.graceInterval = graceInterval
        cancellable = connectionStatus.receive(on: RunLoop.main).sink { [weak self] status in
            self?.handle(status)
        }
        workspaceNotifications.publisher(for: NSWorkspace.willSleepNotification)
            .sink { [weak self] _ in self?.prepareForSleep() }
            .store(in: &lifecycleCancellables)
        workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
            .sink { [weak self] _ in self?.handleWake() }
            .store(in: &lifecycleCancellables)
        scheduleTimeout()
    }

    private func handle(_ status: ConnectionStatus) {
        self.status = status
        if status == .connected {
            graceTimer?.cancel()
            hasConfirmedHandshake = true
            isWaking = false
            phase = .connected
            return
        }

        if isWaking {
            phase = .waking
        } else if !hasConfirmedHandshake {
            phase = .starting
        }
        scheduleTimeout()
    }

    private func prepareForSleep() {
        graceTimer?.cancel()
        isWaking = true
    }

    private func handleWake() {
        isWaking = true
        phase = .waking
        scheduleTimeout()
    }

    private func scheduleTimeout() {
        graceTimer?.cancel()
        graceTimer = scheduler.schedule(after: graceInterval) { [weak self] in
            guard let self, self.status != .connected else { return }
            self.isWaking = false
            self.phase = .unavailable
        }
    }
}

public enum LocalServiceConnectionPhase: Equatable, Sendable {
    case starting
    case waking
    case connected
    case unavailable
}

@MainActor
public protocol ConnectionGraceScheduling: AnyObject {
  func schedule(after interval: TimeInterval, action: @escaping @MainActor () -> Void)
    -> AnyCancellable
}

@MainActor
public final class DispatchConnectionGraceScheduler: ConnectionGraceScheduling {
    public init() {}

    public func schedule(
        after interval: TimeInterval,
        action: @escaping @MainActor () -> Void
    ) -> AnyCancellable {
        let workItem = DispatchWorkItem { Task { @MainActor in action() } }
        DispatchQueue.main.asyncAfter(deadline: .now() + interval, execute: workItem)
        return AnyCancellable { workItem.cancel() }
    }
}

@MainActor
public final class CollectionActivityStatusModel: ObservableObject {
    @Published public private(set) var status: CollectionStatus = .idle
    private var cancellable: AnyCancellable?

    public init(collectionStatus: AnyPublisher<CollectionStatus, Never>) {
    cancellable = collectionStatus.receive(on: RunLoop.main).sink { [weak self] in self?.status = $0
    }
    }
}

public final class CollectionSettingsModel: ObservableObject {
    @Published public var offlineEventCollectionEnabled: Bool {
        didSet {
            defaults.set(offlineEventCollectionEnabled, forKey: Self.offlineEventCollectionKey)
        }
    }

    private static let offlineEventCollectionKey = "velvt.collection.offline_events_enabled"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if defaults.object(forKey: Self.offlineEventCollectionKey) == nil {
            offlineEventCollectionEnabled = true
        } else {
            offlineEventCollectionEnabled = defaults.bool(forKey: Self.offlineEventCollectionKey)
        }
    }
}

public struct ServiceAlert: Equatable, Sendable {
    public enum Severity: Equatable, Sendable {
        case warning
        case error
    }

    public let severity: Severity
    public let title: String
    public let message: String

    public init(severity: Severity, title: String, message: String) {
        self.severity = severity
        self.title = title
        self.message = message
    }
}

@MainActor
public final class ServiceAlertModel: ObservableObject {
    @Published public private(set) var alert: ServiceAlert?
    private var cancellable: AnyCancellable?

    public init(messages: some Publisher<ServerMessage, Never>) {
    cancellable =
      messages
            .receive(on: RunLoop.main)
            .compactMap(Self.alert(for:))
            .sink { [weak self] in self?.alert = $0 }
    }

    public func dismiss() {
        alert = nil
    }

    private static func alert(for message: ServerMessage) -> ServiceAlert? {
        switch message {
        case .malformedMessage:
            return ServiceAlert(
                severity: .warning,
                title: "Message rejected",
                message: "The local service rejected an invalid message."
            )
        case .privacyViolationAlert(let alert):
            return ServiceAlert(
                severity: .error,
                title: "Privacy guard blocked data",
                message: alert.message
            )
        case .shuttingDown:
            return ServiceAlert(
                severity: .warning,
                title: "Service restarting",
                message: "Velvt is reconnecting to the local service."
            )
        case .errorResponse(let error):
            return ServiceAlert(
                severity: .error,
                title: "Service error",
                message: error.message
            )
        case .serviceStatus(let status):
            return alert(forServiceState: status)
        default:
            return nil
        }
    }

    /// The service reporting on itself.
    ///
    /// Rust sends this on every connection and on every health transition, and
    /// the client decoded it and dropped it — so an app that had stopped
    /// uploading, or had degraded to coarser classification, looked exactly
    /// like one working perfectly. Two states are deliberately silent: `ready`
    /// has nothing to say, and a refresh already in flight resolves itself in
    /// seconds, so surfacing it would be a banner that exists to flicker.
    ///
    /// Every message here names what still works. In each of these states local
    /// collection and every local surface are unaffected, and saying so is the
    /// difference between a status and a scare.
    static func alert(forServiceState status: ServiceStatus) -> ServiceAlert? {
        switch status.state {
        case .ready:
            return nil
        case .degraded where status.reason == "auth_refresh_in_flight":
            return nil
        case .degraded:
            return ServiceAlert(
                severity: .warning,
                title: "Reduced classification",
                message:
                    "Velvt is labelling activity with its basic rules for now. Collection and your local history are unaffected."
            )
        case .authRequired:
            return ServiceAlert(
                severity: .warning,
                title: "Signed out",
                message:
                    "Cloud sync is paused until you sign in. Collection and your local history continue on this Mac."
            )
        case .uploadPaused:
            return ServiceAlert(
                severity: .warning,
                title: "Uploads paused",
                message:
                    "This device is no longer authorised to sync. Sign in again to resume. Collection and your local history continue on this Mac."
            )
        }
    }
}

public struct CurrentActivity: Equatable, Sendable {
    public let appName: String
    public let windowTitle: String

    public init(appName: String, windowTitle: String) {
        self.appName = appName
        self.windowTitle = windowTitle
    }
}

public final class CurrentActivityModel: ObservableObject, EventSink {
    @Published public private(set) var activity: CurrentActivity?
    @Published public private(set) var collectedEventCount = 0

    public init() {}

    public func receive(_ event: RawEvent) {
        let activity = CurrentActivity(appName: event.appName, windowTitle: event.windowTitle)
        DispatchQueue.main.async { [weak self] in
            self?.activity = activity
            self?.collectedEventCount += 1
        }
    }
}

enum QueuedEventPresentation {
    static func activity(_ event: QueuedEventSummary) -> String {
        if let localLabel = event.localLabel?.nilIfBlank {
            return localLabel
        }
        guard event.label != "unlogged" else {
            return "Unclassified activity"
        }
        let component = event.label.split(separator: ":", maxSplits: 1).last.map(String.init)
            ?? event.label
        return component
            .replacingOccurrences(of: "_", with: " ")
            .lowercased()
            .capitalized
    }

    static func category(_ event: QueuedEventSummary) -> String {
        category(event.category)
    }

    static func activity(_ correction: ClassificationCorrectionSummary) -> String {
        if let localLabel = correction.localLabel?.nilIfBlank {
            return localLabel
        }
        let component = correction.label.split(separator: ":", maxSplits: 1).last.map(String.init)
            ?? correction.label
        return component
            .replacingOccurrences(of: "_", with: " ")
            .lowercased()
            .capitalized
    }

    static func category(_ correction: ClassificationCorrectionSummary) -> String {
        category(correction.category)
    }

    static func category(_ value: String) -> String {
        value
            .replacingOccurrences(of: "_", with: " ")
            .lowercased()
            .capitalized
    }
}

private struct QueuedEventCorrectionRow: View {
    let event: QueuedEventSummary
    let onSave: (String, String?) -> Void
    let onUndo: () -> Void
    @State private var activityName: String
    @State private var category: String

    init(
        event: QueuedEventSummary,
        onSave: @escaping (String, String?) -> Void,
        onUndo: @escaping () -> Void
    ) {
        self.event = event
        self.onSave = onSave
        self.onUndo = onUndo
        _activityName = State(initialValue: QueuedEventPresentation.activity(event))
        _category = State(initialValue: event.category)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("Activity: \(QueuedEventPresentation.activity(event))")
                .font(.subheadline)
                .lineLimit(1)
                .truncationMode(.tail)
            Text(
                "Category: \(QueuedEventPresentation.category(event)) · Queued \(event.occurredAt.formatted(date: .omitted, time: .shortened))"
            )
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(1)
            TextField("Local activity name", text: $activityName)
                .textFieldStyle(.roundedBorder)
                .font(.caption)
                .onChange(of: activityName) { value in
                    if value.count > 48 {
                        activityName = String(value.prefix(48))
                    }
                }
                .accessibilityHint(
                    "This name stays on this Mac and is never included in cloud activity data."
                )
            HStack(spacing: 6) {
                Picker("Category", selection: $category) {
                    ForEach(Self.categories, id: \.self) { value in
                        Text(QueuedEventPresentation.category(value)).tag(value)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .controlSize(.small)
                Button("Save") {
                    onSave(category, normalizedName)
                }
                .controlSize(.small)
                if event.classificationSource == .userRule {
                    Button("Undo", action: onUndo)
                        .buttonStyle(.plain)
                        .font(.caption)
                }
            }
            // A correction now generalizes to the application, so the next
            // window of the same app is already classified. Said plainly here
            // because a label silently changing across windows the user never
            // touched reads as a malfunction, not as learning.
            Text(Self.scopeExplanation)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 7)
    }

    private var normalizedName: String? {
        let trimmed = activityName.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// States the rule rather than the per-event outcome.
    ///
    /// Whether a specific event can generalize is `app_scope_eligible`, which
    /// lives in Rust and is not on the wire, so the client cannot say which
    /// case a given row is. The rule itself is deterministic and true in every
    /// case, which is enough for the user to predict what saving will do — and
    /// the second sentence is the escape hatch when the guess is wrong.
    fileprivate static let scopeExplanation =
        "Applies to every window of this app. Browser windows apply to that site only. "
        + "Correcting an individual window later overrides it just there."

    fileprivate static let categories = [
        "FOCUS_WORK",
        "PASSIVE_CONSUMPTION",
        "SOCIAL_FEED",
        "COMMUNICATION",
        "TASK_MANAGEMENT",
        "REFERENCE",
        "SYSTEM",
        "UNLOGGED",
    ]
}

private struct ClassificationCorrectionHistoryRow: View {
    let correction: ClassificationCorrectionSummary
    let onSave: (String, String?) -> Void
    let onUndo: () -> Void
    @State private var isEditing = false
    @State private var activityName: String
    @State private var category: String

    init(
        correction: ClassificationCorrectionSummary,
        onSave: @escaping (String, String?) -> Void,
        onUndo: @escaping () -> Void
    ) {
        self.correction = correction
        self.onSave = onSave
        self.onUndo = onUndo
        _activityName = State(initialValue: QueuedEventPresentation.activity(correction))
        _category = State(initialValue: correction.category)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .top, spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(QueuedEventPresentation.activity(correction))
                        .font(.caption.bold())
                        .lineLimit(1)
                    Text(
                        "\(QueuedEventPresentation.category(correction.category)) · Saved \(correction.updatedAt.formatted(date: .abbreviated, time: .omitted)) · Local only"
                    )
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                }
                Spacer(minLength: 4)
                Button(isEditing ? "Cancel" : "Edit") { isEditing.toggle() }
                    .buttonStyle(.plain)
                    .font(.caption2)
                Button("Undo", action: onUndo)
                    .buttonStyle(.plain)
                    .font(.caption2)
            }
            if isEditing {
                TextField("Local activity name", text: $activityName)
                    .textFieldStyle(.roundedBorder)
                    .font(.caption)
                    .onChange(of: activityName) { value in
                        if value.count > 48 { activityName = String(value.prefix(48)) }
                    }
                    .accessibilityHint("This name stays on this Mac")
                HStack(spacing: 6) {
                    Picker("Category", selection: $category) {
                        ForEach(QueuedEventCorrectionRow.categories, id: \.self) { value in
                            Text(QueuedEventPresentation.category(value)).tag(value)
                        }
                    }
                    .pickerStyle(.menu)
                    .controlSize(.small)
                    Button("Save changes") {
                        onSave(category, normalizedName)
                        isEditing = false
                    }
                    .controlSize(.small)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(normalizedName == nil)
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 5)
    }

    private var normalizedName: String? {
        let value = activityName.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }
}

struct CorrectionHistoryBrowser: View {
    @ObservedObject var model: MenuStatusViewModel
    @State private var query = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                TextField("Search saved corrections", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .font(.caption)
                    .onSubmit { search() }
                    .accessibilityLabel("Search local correction history")
                Button("Search", action: search)
                    .controlSize(.small)
            }
            .padding(.horizontal, 16)

            if let page = model.correctionHistoryPage {
                if page.items.isEmpty {
                    Text(query.isEmpty ? "No saved corrections yet" : "No matching corrections")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(page.items) { correction in
                                ClassificationCorrectionHistoryRow(
                                    correction: correction,
                                    onSave: { category, name in
                                        model.updateCorrection(
                                            correction,
                                            category: category,
                                            localActivityName: name
                                        )
                                    },
                                    onUndo: {
                                        model.undoCorrection(stableID: correction.stableID)
                                    }
                                )
                            }
                        }
                    }
                    .frame(minHeight: 80, maxHeight: 140)
                }
                HStack {
                    Button("Previous") { model.previousCorrectionHistoryPage() }
                        .disabled(page.offset == 0)
                    Spacer()
                    Text(pageDescription(page))
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("Next") { model.nextCorrectionHistoryPage() }
                        .disabled(!page.hasMore)
                }
                .controlSize(.small)
                .padding(.horizontal, 16)
            } else {
                ProgressView("Loading saved corrections…")
                    .controlSize(.small)
                    .font(.caption2)
                    .padding(.horizontal, 16)
            }
        }
        .onAppear { model.refreshCorrectionHistory(query: query, offset: 0) }
    }

    private func search() {
        model.refreshCorrectionHistory(query: query, offset: 0)
    }

    private func pageDescription(_ page: CorrectionHistoryPage) -> String {
        guard page.totalCount > 0 else { return "0 results" }
        let first = page.offset + 1
        let last = min(page.totalCount, page.offset + page.items.count)
        return "\(first)–\(last) of \(page.totalCount)"
    }
}

public struct PopoverConnectionPresentation {
    public let label: String
    public let color: Color

    public init(label: String, color: Color) {
        self.label = label
        self.color = color
    }

    public init(status: ConnectionStatus) {
        switch status {
        case .connected:
            label = "Local service connected"
            color = .green
        case .disconnected:
            label = "Disconnected"
            color = .red
        case .connecting, .handshaking, .reconnecting:
            label = "Connecting"
            color = .yellow
        }
    }

    public init(phase: LocalServiceConnectionPhase) {
        switch phase {
        case .starting:
            label = "Starting local service…"
            color = .yellow
        case .waking:
            label = "Waking local service…"
            color = .yellow
        case .connected:
            label = "Local service connected"
            color = .green
        case .unavailable:
            label = "Local service unavailable"
            color = .red
        }
    }
}

public enum MenuBarPopoverLayout {
    /// 600pt is measured, not chosen. It is the narrowest popover width at
    /// which the Now tab stops rewrapping: the tab's content measures 325pt
    /// tall at 660, 620 and 600, then 354pt at 560 and 383pt at 500. Every
    /// 40pt of extra narrowing buys roughly 30pt of extra height on the
    /// tallest tab, so width below 600 is paid for in the scarce dimension.
    /// At 600 the popover is 47% of a 1280pt laptop screen instead of 52%.
    ///
    /// 480pt of height clears the tallest realistic Now tab — an active work
    /// block plus the early signal measures 358pt against a 390pt content
    /// budget — without needing the whole pane to scroll in the common case.
    public static let preferredContentSize = CGSize(width: 600, height: 480)

    /// The guided-tour bar measures exactly 86pt at 560, 600 and 660pt wide,
    /// plus its 1pt divider. Growing the popover by that amount and no more
    /// keeps every row of the main content where it was when the tour opens
    /// and closes. The previous fixed 600pt walkthrough height added 150pt,
    /// so opening the tour pushed the content pane 63pt taller and closing it
    /// pulled it back — a visible jump on both edges of the transition.
    public static let guidedTourBarHeight: CGFloat = 87

    public static var walkthroughContentSize: CGSize {
        CGSize(
            width: preferredContentSize.width,
            height: preferredContentSize.height + guidedTourBarHeight
        )
    }

    /// The floor the screen clamp may not go under, and the floor a manual
    /// resize may not drag under. The clamp used to be
    /// `max(1, visibleFrame - inset)`, which on a small enough visible frame
    /// hands the window a 1pt dimension. A 1pt window is not a degraded
    /// interface, it is an invisible one, and the user has no way back out of
    /// it. Below this size the window fills the visible frame instead.
    ///
    /// 500x320 is measured, not chosen. Rendering the surface on a width
    /// ladder (`MenuBarWindowSnapshotTests`) puts the floor at 470pt: the
    /// bottom bar reads "Start a focus sess…" at 460, "Start a focus sessi…"
    /// at 465 and the whole label at 470. 500 keeps 30pt of slack for the
    /// wider account labels ("Reauthenticate" instead of "Sign In"). On the
    /// height ladder 320 is the shortest window where the Now tab's primary
    /// action is on screen without scrolling; 280 cuts "Start a work block"
    /// in half and 240 hides it behind the bottom bar.
    public static let minimumContentSize = CGSize(width: 500, height: 320)

    public static let screenInset: CGFloat = 24

    /// The workspace rail down the left of the surface — Now / Patterns /
    /// Settings. Named because the Settings pane has to subtract it to know
    /// how much width it is actually being given.
    public static let navigationRailWidth: CGFloat = 132

    /// Measured on the panel style mask this app uses — `.titled` *without*
    /// `.fullSizeContentView`, so the content view sits below the title bar
    /// rather than under it. A window frame is this much taller than its
    /// content. Deliberately not `.fullSizeContentView`: that mask reports
    /// `contentView.safeAreaInsets.top == 28` and draws the content under the
    /// title bar, which is exactly the "wordmark cut off at the top edge"
    /// failure. With this mask the measured insets are zero on every edge and
    /// a top clip is structurally impossible.
    public static let titleBarHeight: CGFloat = 28

    /// The gap between the bottom of the status item and the top of the
    /// window, matching the standing distance `NSPopover` leaves for its arrow.
    public static let statusItemGap: CGFloat = 6

    /// The ceiling a manual resize may not drag past: the screen the window is
    /// on, less the standing inset, and never below the floor.
    public static func maximumContentSize(for visibleFrame: CGRect?) -> CGSize {
        guard let visibleFrame else {
            return CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        }
        return CGSize(
            width: max(minimumContentSize.width, visibleFrame.width - screenInset),
            height: max(
                minimumContentSize.height,
                visibleFrame.height - screenInset - titleBarHeight
            )
        )
    }

    /// The size to open at.
    ///
    /// `stored` is the size the user last dragged the window to, or nil on a
    /// first launch. A size the user chose outranks the preferred size, but
    /// not the screen: a window restored from a 6K display onto a laptop is
    /// clamped down rather than opened wider than the screen it is on. The
    /// walkthrough adds its bar to whatever survives that, so opening the tour
    /// grows the window by the bar and never moves the rows above it.
    public static func resolvedContentSize(
        stored: CGSize?,
        visibleFrame: CGRect?,
        includesWalkthrough: Bool = false
    ) -> CGSize {
        guard let stored, stored.width > 0, stored.height > 0 else {
            return contentSize(for: visibleFrame, includesWalkthrough: includesWalkthrough)
        }
        let maximum = maximumContentSize(for: visibleFrame)
        let base = CGSize(
            width: min(max(stored.width, min(minimumContentSize.width, maximum.width)), maximum.width),
            height: min(
                max(stored.height, min(minimumContentSize.height, maximum.height)),
                maximum.height
            )
        )
        guard includesWalkthrough else { return base }
        return CGSize(
            width: base.width,
            height: min(base.height + guidedTourBarHeight, maximum.height)
        )
    }

    /// The size to persist after a manual resize. The walkthrough bar is the
    /// window's, not the user's, so it is taken back off before storing —
    /// otherwise resizing with the tour open would make the tour's extra 87pt
    /// permanent and the window would grow by a bar on every launch.
    public static func storableContentSize(
        _ contentSize: CGSize,
        includesWalkthrough: Bool
    ) -> CGSize {
        guard includesWalkthrough else { return contentSize }
        return CGSize(
            width: contentSize.width,
            height: max(minimumContentSize.height, contentSize.height - guidedTourBarHeight)
        )
    }

    /// Where the window goes: under the status item, horizontally centred on
    /// it, and always inside the visible frame of the screen the status item
    /// is on.
    ///
    /// The origin is recomputed from the status item on every open rather than
    /// restored from disk. That is what makes the window survive the status
    /// item moving — a menu bar rearrangement, a display change, a notch, a
    /// second monitor being unplugged — without a restored frame ever being
    /// able to land somewhere the user cannot see. Only the *size* is
    /// persisted; the position is always derived.
    public static func windowFrame(
        forContentSize contentSize: CGSize,
        statusItemFrame: CGRect?,
        visibleFrame: CGRect?
    ) -> CGRect {
        let size = CGSize(width: contentSize.width, height: contentSize.height + titleBarHeight)
        guard let visibleFrame else {
            let origin = statusItemFrame.map {
                CGPoint(x: $0.midX - size.width / 2, y: $0.minY - statusItemGap - size.height)
            } ?? .zero
            return CGRect(origin: origin, size: size)
        }

        var x: CGFloat
        var y: CGFloat
        if let statusItemFrame {
            x = statusItemFrame.midX - size.width / 2
            y = statusItemFrame.minY - statusItemGap - size.height
        } else {
            x = visibleFrame.midX - size.width / 2
            y = visibleFrame.maxY - statusItemGap - size.height
        }

        // Clamp into the visible frame. `min` is applied before `max` so that a
        // window wider or taller than the screen still has its top-left corner
        // on screen rather than its bottom-right.
        x = max(visibleFrame.minX, min(x, visibleFrame.maxX - size.width))
        y = max(visibleFrame.minY, min(y, visibleFrame.maxY - size.height))
        return CGRect(x: x, y: y, width: size.width, height: size.height)
    }

    public static func contentSize(
        for visibleFrame: CGRect?,
        includesWalkthrough: Bool = false
    ) -> CGSize {
        let preferredSize = includesWalkthrough
            ? walkthroughContentSize
            : preferredContentSize
        guard let visibleFrame else { return preferredSize }
        return CGSize(
            width: clamp(
                preferredSize.width,
                available: visibleFrame.width,
                minimum: minimumContentSize.width
            ),
            height: clamp(
                preferredSize.height,
                available: visibleFrame.height,
                minimum: minimumContentSize.height
            )
        )
    }

    /// Shrinks to the screen, but never below a size a person can read and
    /// never past the screen itself. The result is always greater than zero
    /// and never exceeds `available`.
    private static func clamp(
        _ preferred: CGFloat,
        available: CGFloat,
        minimum: CGFloat
    ) -> CGFloat {
        let usableMinimum = min(preferred, minimum)
        return max(min(usableMinimum, available), min(preferred, available - screenInset))
    }
}

public enum MenuBarMotionPolicy {
    public static func shouldAnimate(reduceMotion: Bool) -> Bool {
        !reduceMotion
    }
}

/// A Settings destination.
///
/// Still named for the submenus it used to be, because the set of
/// destinations is exactly what it was: this stopped being a submenu when it
/// stopped opening a window, not when it changed contents.
enum SettingsSubmenu: CaseIterable, Hashable, Identifiable {
    case appInfo
    /// The correction workbench. Named for what a person does here, not for
    /// the upload queue it also happens to list.
    case teachApps
    case collectionSettings
    case onboarding
    #if DEBUG
        case debug
    #endif

    var title: String {
        switch self {
        case .appInfo: return "App Info"
        case .teachApps: return "Teach Velvt Your Apps"
        case .collectionSettings: return "Collection Settings"
        case .onboarding: return "Onboarding & Tour"
        #if DEBUG
            case .debug: return "Debug/Testing"
        #endif
        }
    }

    var id: Self { self }
}

/// How the Settings tab arranges its destination list against the detail that
/// list selects.
enum SettingsPaneMode: Equatable {
    /// List on the left, detail beside it — what a resizable window buys, and
    /// the shape the Now / Patterns / Settings rail already uses one level up.
    case sideBySide(listWidth: CGFloat)

    /// One column: the list, or the selected destination with a way back.
    /// Below the threshold, two columns would hand the detail *less* width
    /// than the child popover this pane replaced, which would make the fix a
    /// regression for the correction workbench.
    case stacked
}

/// The Settings pane's layout rule, pulled out of the view so the widths can
/// be asserted rather than eyeballed.
enum SettingsPaneLayout {
    /// Fits "Teach Velvt Your Apps" — the longest destination title — on one
    /// line at `.caption`, with the room a sidebar row insets away.
    static let listWidth: CGFloat = 164

    /// The width every destination was already laid out for: the width of the
    /// `NSPopover` this pane replaced. The detail is never given less.
    static let minimumDetailWidth: CGFloat = 300

    /// And never lets a destination stretch past this.
    ///
    /// Measured on the wide shots: at 603pt of detail the Collection Settings
    /// toggles drift to the middle of the pane, because a `Toggle` at its
    /// intrinsic width centres itself in a `VStack` and 300pt of popover used
    /// to hide that; the onboarding sentence runs to a 685pt line. Both are
    /// artifacts of handing content laid out for 300–380pt whatever a dragged
    /// window happens to be. Capping the content column and pinning it left
    /// keeps every destination the shape it was designed as, and lets the
    /// extra width the user asked for go to the destinations that use it.
    static let maximumDetailContentWidth: CGFloat = 420

    /// The width the Settings tab has to work with inside a window of
    /// `contentWidth` — the workspace rail and its hairline come off first.
    static func paneWidth(forContentWidth contentWidth: CGFloat) -> CGFloat {
        max(0, contentWidth - MenuBarPopoverLayout.navigationRailWidth - 1)
    }

    static func mode(forPaneWidth paneWidth: CGFloat) -> SettingsPaneMode {
        paneWidth >= listWidth + minimumDetailWidth
            ? .sideBySide(listWidth: listWidth)
            : .stacked
    }

    /// What the selected destination actually gets to draw in.
    static func detailWidth(forPaneWidth paneWidth: CGFloat) -> CGFloat {
        switch mode(forPaneWidth: paneWidth) {
        case .sideBySide(let listWidth): return paneWidth - listWidth
        case .stacked: return paneWidth
        }
    }
}

public enum MenuBarWorkspaceTab: CaseIterable, Equatable, Hashable {
    case workBlock
    case history
    case settings

    /// "Today" and "Your Week" are reporting periods — the vocabulary of a
    /// report you read, not of a thing that is watching with you right now.
    /// "Now" is where the product actually lives, and "Patterns" is a claim
    /// about the person rather than a date range.
    public var title: String {
        switch self {
        case .workBlock: return "Now"
        case .history: return "Patterns"
        case .settings: return "Settings"
        }
    }

    fileprivate var systemImage: String {
        switch self {
        case .workBlock: return "timer"
        case .history: return "calendar"
        case .settings: return "gearshape"
        }
    }

    var keyboardShortcut: KeyEquivalent {
        switch self {
        case .workBlock: return "1"
        case .history: return "2"
        case .settings: return "3"
        }
    }
}

public struct MenuBarPopoverNavigator {
    public private(set) var selectedWorkspaceTab: MenuBarWorkspaceTab = .workBlock

    public init() {}

    public mutating func selectWorkspaceTab(_ tab: MenuBarWorkspaceTab) {
        selectedWorkspaceTab = tab
    }

    public mutating func showSettings() {
        selectedWorkspaceTab = .settings
    }

    public mutating func resetForPopoverOpening() {
        selectedWorkspaceTab = .workBlock
    }
}

/// What Escape does, given what is open.
enum MenuBarEscapeAction: Equatable {
    case dismissGuidedTour
    case clearSettingsSelection
    case closeSurface
}

/// Escape used to mean one thing — close the surface — because everything it
/// could have backed out of first was a separate window that took the key
/// press itself. The Settings detail is inside this window now, so Escape has
/// to back out of it before it closes anything, and the order is worth
/// asserting rather than reading.
enum MenuBarEscapeResolver {
    static func action(
        guidedTourIsPresented: Bool,
        selectedWorkspaceTab: MenuBarWorkspaceTab,
        selectedSettingsDestination: SettingsSubmenu?
    ) -> MenuBarEscapeAction {
        if guidedTourIsPresented { return .dismissGuidedTour }
        if selectedWorkspaceTab == .settings, selectedSettingsDestination != nil {
            return .clearSettingsSelection
        }
        return .closeSurface
    }
}

public struct MenuBarPopoverView: View {
    @ObservedObject private var presentation: PermissionPresentationModel
    private let permissionManager: (any PermissionManagerProtocol)?
    @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject private var serviceConnectionStatus: ServiceConnectionStatusModel
    @ObservedObject private var collectionActivityStatus: CollectionActivityStatusModel
    @ObservedObject private var currentActivity: CurrentActivityModel
    @ObservedObject private var serviceAlertModel: ServiceAlertModel
    @ObservedObject private var collectionSettings: CollectionSettingsModel
    @ObservedObject private var workBlockCoordinator: WorkBlockCoordinator
    @ObservedObject private var localDashboardCoordinator: LocalDashboardCoordinator
    private let accountStateManager: AccountStateManager?
    private let ipcClient: (any IPCClientProtocol)?
    private let menuStatusViewModel: MenuStatusViewModel?
    private let simulateNotification: (() async -> DebugInsightSimulationResult)?
    private let restartLocalService: (() -> Void)?
    private let replayOnboarding: (() -> Void)?
    private let startGuidedTour: (() -> Void)?
    @ObservedObject private var updateController: AppUpdateController
    @ObservedObject private var guidedTour: GuidedTourModel
    @ObservedObject private var metricsStore: AppMetricsStore
    private let popoverWillOpen: AnyPublisher<Void, Never>
    private let onEscape: () -> Void
    private let onTerminate: () -> Void
    @State private var navigator = MenuBarPopoverNavigator()
    @State private var selectedSettingsDestination: SettingsSubmenu?
    @State private var confirmsWorkBlockClear = false
    @State private var diagnosticsCopied = false
    @State private var debugInsightStatus: String?
    @State private var showsFocusSession = false
  @State private var showsSystemState = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        presentation: PermissionPresentationModel,
        permissionManager: (any PermissionManagerProtocol)? = nil,
        coordinator: ConcreteDisplayDataCoordinator,
        serviceConnectionStatus: ServiceConnectionStatusModel,
        collectionActivityStatus: CollectionActivityStatusModel,
        currentActivity: CurrentActivityModel,
        serviceAlertModel: ServiceAlertModel,
        collectionSettings: CollectionSettingsModel = CollectionSettingsModel(),
        workBlockCoordinator: WorkBlockCoordinator? = nil,
        localDashboardCoordinator: LocalDashboardCoordinator? = nil,
        accountStateManager: AccountStateManager? = nil,
        ipcClient: (any IPCClientProtocol)? = nil,
        menuStatusViewModel: MenuStatusViewModel? = nil,
        simulateNotification: (() async -> DebugInsightSimulationResult)? = nil,
        restartLocalService: (() -> Void)? = nil,
        replayOnboarding: (() -> Void)? = nil,
        startGuidedTour: (() -> Void)? = nil,
        updateController: AppUpdateController,
        guidedTour: GuidedTourModel = GuidedTourModel(),
    metricsStore: AppMetricsStore = AppMetricsStore(
      defaults: UserDefaults(suiteName: "MenuBarPopoverView.preview") ?? .standard),
        popoverWillOpen: AnyPublisher<Void, Never> = Empty().eraseToAnyPublisher(),
        onEscape: @escaping () -> Void,
        onTerminate: @escaping () -> Void = {}
    ) {
        self.presentation = presentation
        self.permissionManager = permissionManager
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.collectionActivityStatus = collectionActivityStatus
        self.currentActivity = currentActivity
        self.serviceAlertModel = serviceAlertModel
        self.collectionSettings = collectionSettings
    self.workBlockCoordinator =
      workBlockCoordinator ?? WorkBlockCoordinator(ipcClient: UnavailableWorkBlockIPCClient())
    self.localDashboardCoordinator =
      localDashboardCoordinator
      ?? LocalDashboardCoordinator(ipcClient: UnavailableLocalDashboardIPCClient())
        self.accountStateManager = accountStateManager
        self.ipcClient = ipcClient
        self.menuStatusViewModel = menuStatusViewModel
        self.simulateNotification = simulateNotification
        self.restartLocalService = restartLocalService
        self.replayOnboarding = replayOnboarding
        self.startGuidedTour = startGuidedTour
        self.updateController = updateController
        self.guidedTour = guidedTour
        self.metricsStore = metricsStore
        self.popoverWillOpen = popoverWillOpen
        self.onEscape = onEscape
        self.onTerminate = onTerminate
    }

    /// Test seam. Returns a copy of this view whose workspace already starts
    /// on Settings with `destination` selected, so the snapshot tests can
    /// photograph each destination without an app, a click, or a reach into
    /// SwiftUI's private state. Assigning a `State` before the view is first
    /// rendered is the supported way to give one an initial value. The
    /// shipping app never calls this — every real path starts with no
    /// destination selected.
    func openedOnSettings(_ destination: SettingsSubmenu?) -> MenuBarPopoverView {
        var copy = self
        var seeded = MenuBarPopoverNavigator()
        seeded.showSettings()
        copy._navigator = State(initialValue: seeded)
        copy._selectedSettingsDestination = State(initialValue: destination)
        return copy
    }

    public var body: some View {
        VStack(spacing: 0) {
            mainContent
            if guidedTour.isPresented {
                Divider().opacity(0.2)
                GuidedTourBar(model: guidedTour)
                    .fixedSize(horizontal: false, vertical: true)
                    .layoutPriority(2)
                    .transition(.opacity)
            }
        }
        .animation(
            MenuBarMotionPolicy.shouldAnimate(reduceMotion: reduceMotion)
                ? .easeInOut(duration: 0.18)
                : nil,
            value: guidedTour.isPresented
        )
        .frame(
            idealWidth: MenuBarPopoverLayout.preferredContentSize.width,
            maxWidth: .infinity,
            idealHeight: MenuBarPopoverLayout.preferredContentSize.height,
            maxHeight: .infinity,
            alignment: .top
        )
        .preferredColorScheme(.dark)
        .tint(Color.velvtPink)
        .onExitCommand {
            switch MenuBarEscapeResolver.action(
                guidedTourIsPresented: guidedTour.isPresented,
                selectedWorkspaceTab: navigator.selectedWorkspaceTab,
                selectedSettingsDestination: selectedSettingsDestination
            ) {
            case .dismissGuidedTour: guidedTour.dismiss()
            case .clearSettingsSelection: clearSettingsSelection()
            case .closeSurface: onEscape()
            }
        }
        .onChange(of: guidedTour.step) { route(to: $0) }
        .onChange(of: guidedTour.isPresented) { isPresented in
            if isPresented {
                route(to: guidedTour.step)
            } else {
                clearSettingsSelection()
                navigator.selectWorkspaceTab(.workBlock)
            }
        }
        .onReceive(popoverWillOpen) {
            clearSettingsSelection()
            navigator.resetForPopoverOpening()
        }
    }

    /// The window is resizable now, so this row has to survive being narrowed
    /// and shortened rather than merely fitting at 600x480.
    ///
    /// Three things make it survive. `fixedSize(vertical:)` on the whole row
    /// means the header reports the height it actually needs and is never
    /// compressed into it: the wordmark's box is a fixed 30pt and a squeezed
    /// fixed frame does not shrink, it clips — which is the reported cut-off
    /// top edge. `layoutPriority(1)` means the flexible workspace below gives
    /// up the space instead of the header. And both status lines wrap
    /// (`fixedSize(horizontal: false, vertical: true)`, no `lineLimit`) so a
    /// long label such as "Collection paused: Accessibility permission
    /// required" or "Checking cloud synchronization…" takes a second line at a
    /// narrow width instead of being truncated at the right edge.
    private var mainHeader: some View {
        HStack(alignment: .top, spacing: 8) {
            Image("VelvtWordmark")
                .resizable()
                .renderingMode(.template)
                .interpolation(.high)
                .scaledToFit()
                .foregroundStyle(Color.velvtText)
                .frame(width: 76, height: 30, alignment: .leading)
                .accessibilityLabel("Velvt")
                .layoutPriority(1)
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(localCollectionPresentation.label)
                        .font(.caption)
                        .foregroundStyle(localCollectionPresentation.color)
                        .multilineTextAlignment(.trailing)
                        .fixedSize(horizontal: false, vertical: true)
                    Circle()
                        .fill(localCollectionPresentation.color)
                        .frame(width: 7, height: 7)
                        .layoutPriority(1)
                }
                Text(backendStatusLabel)
                    .font(.caption2)
                    .foregroundStyle(Color.velvtMuted.opacity(0.72))
                    .multilineTextAlignment(.trailing)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: 360, alignment: .trailing)
        }
        .padding(.horizontal, 16)
        .padding(.top, 15)
        .padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true)
        .layoutPriority(1)
        .overlay {
            if guidedTour.isPresented, guidedTour.step == .statusAndRecovery {
                RoundedRectangle(cornerRadius: 7)
                    .stroke(Color.velvtPink, lineWidth: 2)
                    .padding(4)
                    .allowsHitTesting(false)
            }
        }
    }

    private var mainContent: some View {
        VStack(spacing: 0) {
            mainHeader
            Divider().opacity(0.2)
            workspace
        }
        .task {
            _ = await permissionManager?.checkStatus(for: .accessibility)
        }
    }

    private var workspace: some View {
        HStack(spacing: 0) {
            workspaceNavigationRail
                .frame(width: MenuBarPopoverLayout.navigationRailWidth)

            Divider().opacity(0.2)

            workspaceDetailPane
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    private var workspaceDetailPane: some View {
        VStack(spacing: 0) {
            if let alert = serviceAlertModel.alert {
                serviceAlertRow(alert)
                Divider().opacity(0.15)
            }
            if collectionActivityStatus.status == .running {
                gatheringInfoStatus
                Divider().opacity(0.15)
            }

            // Every tab scrolls, not only Settings. Measured at the 600pt
            // popover width against a 390pt content budget: the Now tab is
            // 358pt with an active block, 392pt once the privacy disclosure is
            // open, and 435pt with the accessibility recovery banner above it.
            // Without a scroll view on this path that last 45-77pt was simply
            // clipped, and the Now tab has no scroll view of its own anywhere
            // beneath it — so the "Start a work block" button could sit below
            // the cut with no way to reach it.
            // Settings is the exception. It is a destination list beside the
            // detail that list selects, and a master-detail nested inside a
            // page scroll scrolls the page rather than the column under the
            // pointer — the same nesting mistake the comment above describes,
            // one level down. Its two columns scroll themselves.
            if navigator.selectedWorkspaceTab == .settings {
                workspaceTransitionContent
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .layoutPriority(1)
            } else {
                ScrollView {
                    workspaceTransitionContent
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .layoutPriority(1)
            }

            Divider().opacity(0.15)
            workspaceBottomBar
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.black.opacity(0.08))
    }

    private var workspaceTransitionContent: some View {
        ZStack(alignment: .topLeading) {
            selectedWorkspaceContent
                .id(navigator.selectedWorkspaceTab)
                .transition(.opacity)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .animation(
            MenuBarMotionPolicy.shouldAnimate(reduceMotion: reduceMotion)
                ? .easeOut(duration: 0.16)
                : nil,
            value: navigator.selectedWorkspaceTab
        )
    }

    private var workspaceNavigationRail: some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(MenuBarWorkspaceTab.allCases, id: \.self) { tab in
                workspaceNavigationButton(tab)
            }

            Spacer(minLength: 12)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 10)
        .frame(maxHeight: .infinity)
        .background(Color.velvtSurface.opacity(0.55))
    }

    private func workspaceNavigationButton(_ tab: MenuBarWorkspaceTab) -> some View {
        let isSelected = navigator.selectedWorkspaceTab == tab
        return Button {
            guard !isSelected else { return }
            clearSettingsSelection()
            navigator.selectWorkspaceTab(tab)
        } label: {
            Label(tab.title, systemImage: tab.systemImage)
                .font(.caption)
                .fontWeight(isSelected ? .semibold : .medium)
                .foregroundStyle(isSelected ? Color.velvtText : Color.velvtText.opacity(0.62))
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .padding(.horizontal, 10)
                .padding(.vertical, 9)
                .background(isSelected ? Color.velvtPanelHighlight : Color.clear)
                .clipShape(RoundedRectangle(cornerRadius: 7))
        }
        .buttonStyle(.plain)
        .keyboardShortcut(tab.keyboardShortcut, modifiers: .command)
        .accessibilityLabel(tab.title)
        .accessibilityValue(isSelected ? "Selected" : "")
        .accessibilityAddTraits(isSelected ? .isSelected : [])
        .overlay {
            if guidedTour.isPresented, tourTab == tab {
                RoundedRectangle(cornerRadius: 7)
                    .stroke(Color.velvtPink, lineWidth: 2)
                    .allowsHitTesting(false)
            }
        }
    }

    @ViewBuilder
    private var selectedWorkspaceContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            if presentation.showsAccessibilityRecovery {
                PermissionRecoveryView()
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        Color.velvtSurface.opacity(0.92),
                        in: RoundedRectangle(cornerRadius: 10)
                    )
                    .overlay {
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(Color.white.opacity(0.08), lineWidth: 1)
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 12)
            }
            switch navigator.selectedWorkspaceTab {
            case .workBlock:
                MinimalDashboardWorkspaceView(
                    coordinator: coordinator,
                    workBlockCoordinator: workBlockCoordinator,
                    localDashboardCoordinator: localDashboardCoordinator,
                    onStartWorkBlock: { showsFocusSession = true },
                    highlightsInsight: guidedTour.isPresented && guidedTour.step == .earlySignal,
                    highlightsFocus: guidedTour.isPresented
                        && guidedTour.step == .focusFragmentation
                )

                DisclosureGroup("Privacy details", isExpanded: $showsSystemState) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(
                            "Raw work activity and local display labels stay on this Mac. Only privacy-safe abstractions may synchronize for summaries and insights."
                        )
                    }
                    .font(.caption)
                    .foregroundStyle(Color.velvtMuted)
                    .padding(.top, 8)
                }
                .font(.caption)
                .tint(Color.velvtText)
                .padding(.horizontal, 12)
                .padding(.bottom, 12)
                .accessibilityHint(
                    "Expands the plain-language privacy explanation"
                )

            case .history:
                HistoryWorkspaceView(
                    coordinator: coordinator,
                    localDashboardCoordinator: localDashboardCoordinator
                )
                .tourHighlight(guidedTour.isPresented && guidedTour.step == .dailyActivity)

            case .settings:
                settingsContent
            }
        }
        // Settings fills the pane: its list column and its detail column each
        // need to know how tall they are so they can scroll themselves. Every
        // other tab is inside a scroll view and must keep reporting the height
        // it actually wants.
        .frame(
            maxWidth: .infinity,
            maxHeight: navigator.selectedWorkspaceTab == .settings ? .infinity : nil,
            alignment: .top
        )
    }

    private var workspaceBottomBar: some View {
        HStack(spacing: 10) {
            if let accountStateManager, let ipcClient {
                MenuBarAccountControls(accountStateManager: accountStateManager, ipcClient: ipcClient)
            }
            Spacer(minLength: 8)
            Button {
                showsFocusSession.toggle()
            } label: {
                // The window narrows now, and something in this row has to
                // give first. It should not be the one action the row exists
                // for: without this the primary button was the last item in
                // the HStack and therefore the first to truncate, reading
                // "Start a focus sess…" from 465pt down.
                Label("Start a focus session", systemImage: "timer")
                    .fixedSize(horizontal: true, vertical: false)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .layoutPriority(1)
            .tourHighlight(guidedTour.isPresented && guidedTour.step == .today)
            .popover(isPresented: $showsFocusSession, arrowEdge: .bottom) {
                ScrollView {
                    WorkBlockView(coordinator: workBlockCoordinator)
                }
                .frame(width: 400, height: 390, alignment: .top)
                .background(Color.velvtSurface)
                .preferredColorScheme(.dark)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
        .background(Color.velvtSurface.opacity(0.32))
    }

    /// The settled state, drawn as settled.
    ///
    /// This row used to pair an indeterminate `ProgressView` with the words
    /// "Gathering info", and it rendered whenever `collectionActivityStatus`
    /// was `.running` — which is to say, the entire time the app is working
    /// correctly. `.running` is what `CollectionModule` sends once collection
    /// starts; nothing ever moves it to a finished state, because there is
    /// nothing to finish. A spinner is a promise that something will complete,
    /// so this one promised something that never arrives, and a user watching
    /// it for twenty minutes was reading it exactly as intended.
    ///
    /// It also disagreed with itself: the same condition renders as "active"
    /// elsewhere, so one state was described two ways on one screen.
    ///
    /// If a genuinely transient state is wanted here later, the honest
    /// candidate is the first cloud insight — that one really is pending, and
    /// really does resolve. It is a different signal from "is collection
    /// running" and needs its own condition, not this one.
    private var gatheringInfoStatus: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(Color.velvtGreen)
                .frame(width: 7, height: 7)
            Text("Local collection active")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    private func serviceAlertRow(_ alert: ServiceAlert) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(alert.severity == .error ? Color.red : Color.yellow)
                .frame(width: 7, height: 7)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 2) {
                Text(alert.title)
                    .font(.caption.bold())
                Text(alert.message)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
            Button("Dismiss") {
                serviceAlertModel.dismiss()
            }
            .buttonStyle(.plain)
            .font(.caption2)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 9)
    }

    /// The Settings tab: a destination list and the selected destination's
    /// detail, both inside this window.
    ///
    /// This used to be a column of rows that each opened an `NSPopover` — a
    /// second, detached window floating outside this one — and opened it on
    /// *hover*, so moving the pointer across the column threw up a window the
    /// user had not asked for. The surface is a resizable panel with a 500pt
    /// floor now, so the detail has somewhere to live in the window that is
    /// already open, and it is reached by clicking a row like every other
    /// navigation in this app.
    private var settingsContent: some View {
        GeometryReader { proxy in
            let mode = SettingsPaneLayout.mode(forPaneWidth: proxy.size.width)
            VStack(spacing: 0) {
                switch mode {
                case .sideBySide(let listWidth):
                    HStack(spacing: 0) {
                        settingsDestinationList
                            .frame(width: listWidth)
                        Divider().opacity(0.2)
                        settingsDetail(showsBackButton: false)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                case .stacked:
                    if selectedSettingsDestination == nil {
                        settingsDestinationList
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else {
                        settingsDetail(showsBackButton: true)
                    }
                }
                Divider().opacity(0.15)
                // Deliberately outside the columns and always on screen:
                // account deletion, updates and Quit belong to the app rather
                // than to any one destination, and they used to be reachable
                // no matter which submenu window was open.
                settingsFooter
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
        .onAppear { updateController.refreshAvailability() }
    }

    /// The DEBUG destination only exists when the app was built with a way to
    /// simulate an insight, exactly as the old row did.
    private var settingsDestinations: [SettingsSubmenu] {
        SettingsSubmenu.allCases.filter { destination in
            #if DEBUG
                if destination == .debug { return simulateNotification != nil }
            #endif
            return true
        }
    }

    /// A `List` with a selection binding, not the stack of buttons this used
    /// to be, and for one reason: a `List` is an `NSTableView` underneath, and
    /// the table is where macOS keyboard navigation already lives. The arrow
    /// keys move the selection between rows, the list takes focus, and the
    /// selected row keeps its highlight when focus leaves. Selecting a row is
    /// the navigation here — there is no separate activation step, and nothing
    /// opens until a row is selected.
    ///
    /// Hover draws the standard row highlight and does nothing else, which is
    /// the entire complaint this pane exists to answer. Escape backs out; see
    /// `MenuBarEscapeResolver`.
    private var settingsDestinationList: some View {
        List(selection: $selectedSettingsDestination) {
            ForEach(settingsDestinations) { destination in
                Text(destination.title)
                    .font(.caption)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                    // A sidebar row draws its label in the secondary colour,
                    // which on this background is close to unreadable at
                    // `.caption`. These rows are the navigation, not a caption
                    // under it.
                    .foregroundStyle(Color.velvtText)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .tag(destination)
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Color.velvtSurface.opacity(0.4))
        .accessibilityLabel("Settings sections")
    }

    @ViewBuilder
    private func settingsDetail(showsBackButton: Bool) -> some View {
        VStack(spacing: 0) {
            if showsBackButton {
                Button {
                    clearSettingsSelection()
                } label: {
                    Label("All Settings", systemImage: "chevron.left")
                        .font(.caption)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityHint("Returns to the list of settings sections")
                Divider().opacity(0.15)
            }
            ScrollView {
                Group {
                    if let destination = selectedSettingsDestination {
                        settingsDestinationContent(for: destination)
                    } else {
                        settingsDetailPlaceholder
                    }
                }
                .frame(maxWidth: SettingsPaneLayout.maximumDetailContentWidth, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    /// Only ever seen side by side: when the pane stacks, an empty selection
    /// shows the list itself rather than a pane telling you to go find it.
    private var settingsDetailPlaceholder: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Settings")
                .font(.headline)
            Text("Pick a section on the left to open it here.")
                .font(.caption)
                .foregroundStyle(Color.velvtMuted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
    }

    private var settingsFooter: some View {
        HStack(spacing: 8) {
            if let accountStateManager, let ipcClient {
                SettingsAccountDeletionButton(
                    accountStateManager: accountStateManager,
                    ipcClient: ipcClient
                )
            }
            Button("Check for Updates…") {
                updateController.checkForUpdates()
            }
            .buttonStyle(.bordered)
            .disabled(!updateController.canCheckForUpdates)
            Button("Quit Velvt", role: .destructive, action: onTerminate)
                .buttonStyle(.bordered)
            Spacer(minLength: 8)
            Text("Velvt \(appVersion)")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .layoutPriority(-1)
        }
        // The row has to survive the 500pt floor, where it is competing for
        // 367pt with three bordered buttons in it.
        .controlSize(.small)
        .font(.caption)
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func settingsDestinationContent(for submenu: SettingsSubmenu) -> some View {
        switch submenu {
        case .appInfo:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
                infoRow("Version", appVersion)
                infoRow("Device ID", menuStatusViewModel?.status?.deviceID ?? "Not registered")
                authenticationInfoRow()
                statusRow(
                    "Local privacy service",
                    presentation: connectionPresentation,
                    refresh: { menuStatusViewModel?.refresh() }
                )
                infoRow("Collection", localCollectionPresentation.label)
                infoRow("Cloud sync", uploadStatusDescription)
                infoRow("Last synchronized", lastSuccessfulSyncDescription)
                infoRow("Queued", "\(menuStatusViewModel?.status?.queuedEventCount ?? 0) events")
                infoRow("Next retry", nextRetryDescription)
                #if DEBUG
                    infoRow("Events collected", "\(currentActivity.collectedEventCount)")
                    infoRow("Actions logged", "\(metricsStore.actionsLogged)")
                    infoRow("Interventions", "\(metricsStore.interventions)")
                #endif
                Divider().padding(.vertical, 6)
                Button("Retry Cloud Synchronization") {
                    menuStatusViewModel?.sendAllNow()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                if restartLocalService != nil {
                    Button("Restart Local Service") { restartLocalService?() }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 6)
                }
                Button(diagnosticsCopied ? "Diagnostics Copied" : "Copy Diagnostics") {
                    copyDiagnostics()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
            }
            .onAppear { menuStatusViewModel?.refresh() }

        case .teachApps:
            if let menuStatusViewModel {
                CorrectionWorkbenchView(
                    menuStatus: menuStatusViewModel,
                    localDashboard: localDashboardCoordinator,
                    title: submenu.title
                )
            } else {
                CorrectionWorkbenchUnavailableView(title: submenu.title)
            }

        case .collectionSettings:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
                Toggle("Offline Event Collection", isOn: $collectionSettings.offlineEventCollectionEnabled)
                .toggleStyle(.switch)
                .font(.caption)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                // The single invitation opt-out. The Rust service owns and
                // enforces the setting; this toggle renders the reported
                // state and sends the change. Off means silence — nothing
                // else about the product changes.
                Toggle(
                    "Initiation Invitations",
                    isOn: Binding(
                        get: { workBlockCoordinator.invitationsEnabled },
                        set: { workBlockCoordinator.setInvitationsEnabled($0) }
                    )
                )
                .toggleStyle(.switch)
                .font(.caption)
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
                .accessibilityHint("Off silences soft-start invitations entirely")
                Button("Clear Local Work Blocks", role: .destructive) {
                    confirmsWorkBlockClear = true
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .confirmationDialog(
                    "Clear local intentions, work blocks, and results from this Mac?",
                    isPresented: $confirmsWorkBlockClear,
                    titleVisibility: .visible
                ) {
                    Button("Clear Local Work Blocks", role: .destructive) {
                        workBlockCoordinator.clearLocalData()
                    }
                    Button("Cancel", role: .cancel) {}
                }
            }

        case .onboarding:
            VStack(spacing: 0) {
                submenuTitle(submenu.title)
                Text(
                    "Replay the full first-run explanation or tour the live menu-bar interface again."
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.bottom, 10)
                Button("Replay Full Intro") {
                    clearSettingsSelection()
                    replayOnboarding?()
                    onEscape()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                Button("Take Guided Tour") {
                    clearSettingsSelection()
                    startGuidedTour?()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }

        #if DEBUG
            case .debug:
                VStack(spacing: 0) {
                    submenuTitle(submenu.title)
                    Button {
                        runDebugInsightSimulation()
                    } label: {
                        HStack {
                            Image(systemName: "bell.badge")
                            Text("Simulate Insight")
                            Spacer()
                        }
                        .contentShape(Rectangle())
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                    }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity)
                    if let debugInsightStatus {
                        Text(debugInsightStatus)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 16)
                            .padding(.bottom, 12)
                    }
                    Button {
                        workBlockCoordinator.simulateDebugInvitation()
                        clearSettingsSelection()
                    } label: {
                        HStack {
                            Image(systemName: "sunrise")
                            Text("Simulate Invitation")
                            Spacer()
                        }
                        .contentShape(Rectangle())
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                    }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity)
                    Button {
                        workBlockCoordinator.simulateDebugDemotion()
                        clearSettingsSelection()
                    } label: {
                        HStack {
                            Image(systemName: "pause.circle")
                            Text("Simulate Demotion")
                            Spacer()
                        }
                        .contentShape(Rectangle())
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                    }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity)
                    Button {
                        workBlockCoordinator.simulateDebugWeeklyDigest()
                        clearSettingsSelection()
                    } label: {
                        HStack {
                            Image(systemName: "doc.plaintext")
                            Text("Simulate Weekly Receipts")
                            Spacer()
                        }
                        .contentShape(Rectangle())
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                    }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity)
                }
        #endif
        }
    }

    private var tourTab: MenuBarWorkspaceTab? {
        switch guidedTour.step {
        case .settings:
            return .settings
        case .today, .earlySignal, .focusFragmentation, .dailyActivity, .statusAndRecovery:
            return nil
        }
    }

    private func route(to step: GuidedTourStep) {
        clearSettingsSelection()
        switch step {
        case .today, .earlySignal, .focusFragmentation, .statusAndRecovery:
            navigator.selectWorkspaceTab(.workBlock)
        case .dailyActivity:
            navigator.selectWorkspaceTab(.history)
        case .settings:
            navigator.showSettings()
        }
    }

    private var connectionPresentation: PopoverConnectionPresentation {
        PopoverConnectionPresentation(phase: serviceConnectionStatus.phase)
    }

    private var localCollectionPresentation: PopoverConnectionPresentation {
        switch presentation.statuses[.accessibility] ?? .unknown {
        case .unknown:
            return PopoverConnectionPresentation(
                label: "Checking Accessibility…",
                color: .gray
            )
        case .denied, .restricted:
            return PopoverConnectionPresentation(
                label: "Collection paused: Accessibility permission required",
                color: .yellow
            )
        case .granted:
            break
        }
        if collectionActivityStatus.status == .running {
            return PopoverConnectionPresentation(label: "Collection active", color: .green)
        }
        return PopoverConnectionPresentation(label: "Collection paused", color: .yellow)
    }

    private var backendStatusLabel: String {
        guard let accountStateManager else { return "Cloud status unavailable" }
        if accountStateManager.requiresReauthentication {
            return "Sign in required"
        }
        guard case .loggedIn = accountStateManager.accountState else {
            return "Sign in required for synchronization"
        }
        guard let status = menuStatusViewModel?.status else {
            return "Checking cloud synchronization…"
        }
        if !status.cloudReady {
            return status.queuedEventCount > 0
                ? "Working offline · \(status.queuedEventCount) queued"
                : "Cloud unreachable"
        }
        if status.uploadStatus == "retrying" || status.uploadStatus == "rate_limited" {
            return "Cloud synchronization retrying"
        }
        return "Cloud synchronized"
    }

    private var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
            ?? Bundle.main.object(forInfoDictionaryKey: "VelvtClientVersion") as? String
            ?? "Development"
    }

    private var authenticationPresentation: AuthenticationStatusPresentation {
        guard let accountStateManager else {
            return AuthenticationStatusPresentation(accountState: .loggedOut, email: nil)
        }
        return AuthenticationStatusPresentation(
            accountState: accountStateManager.accountState,
            email: accountStateManager.accountEmail,
            requiresReauthentication: accountStateManager.requiresReauthentication
        )
    }

    private var isAuthenticated: Bool {
        guard let accountStateManager else { return true }
        if case .loggedIn = accountStateManager.accountState {
            return true
        }
        return false
    }

    private var uploadStatusDescription: String {
        guard let status = menuStatusViewModel?.status else { return "Unknown" }
        switch status.uploadStatus {
        case "ready":
            return "Ready"
        case "pending":
            return withNextUploadAttempt("\(status.pendingUploadBatchCount) pending", status)
        case "retrying":
            return retryDescription(status)
        case "auth_required":
            return withNextUploadAttempt("Sign in required", status)
        case "network_unavailable":
            return withNextUploadAttempt("Network unavailable", status)
        case "rate_limited":
            return retryDescription(status)
        case "privacy_rejected":
            return withNextUploadAttempt("Privacy check failed", status)
        default:
            return withNextUploadAttempt(status.lastUploadErrorCode ?? status.uploadStatus, status)
        }
    }

    private func retryDescription(_ status: MenuStatus) -> String {
        let prefix: String
        if let error = status.lastUploadErrorCode, !error.isEmpty {
            prefix = "\(status.failedUploadBatchCount) retrying · \(error)"
        } else {
            prefix = "\(status.failedUploadBatchCount) retrying"
        }
        return withNextUploadAttempt(prefix, status)
    }

    private func withNextUploadAttempt(_ description: String, _ status: MenuStatus) -> String {
        guard let retryAt = status.nextUploadAttemptAt else { return description }
        return "\(description) · next retry \(retryAt.formatted(date: .omitted, time: .shortened))"
    }

    private var lastSuccessfulSyncDescription: String {
        guard let date = menuStatusViewModel?.status?.lastSuccessfulSyncAt else {
            return "Not yet"
        }
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private var nextRetryDescription: String {
        guard let date = menuStatusViewModel?.status?.nextUploadAttemptAt else {
            return "No retry scheduled"
        }
        return date.formatted(date: .omitted, time: .shortened)
    }

    private func copyDiagnostics() {
        let status = menuStatusViewModel?.status
        let accountStatus: String
        if accountStateManager?.requiresReauthentication == true {
            accountStatus = "sign_in_required"
        } else if isAuthenticated {
            accountStatus = "authenticated"
        } else {
            accountStatus = "signed_out"
        }
    let protocolVersion =
      Bundle.main.object(
                forInfoDictionaryKey: "VelvtProtocolVersion"
            ) as? String ?? "unknown"
        let lines = [
            "Velvt privacy-safe diagnostics",
            "app_version=\(appVersion)",
            "protocol_version=\(protocolVersion)",
            "local_service=\(String(describing: serviceConnectionStatus.phase))",
            "collection=\(collectionDiagnosticCode)",
            "account=\(accountStatus)",
            "backend=\(status?.uploadStatus ?? "unknown")",
            "queued_event_count=\(status?.queuedEventCount ?? 0)",
            "last_successful_sync=\(status?.lastSuccessfulSyncAt?.ISO8601Format() ?? "none")",
            "next_retry=\(status?.nextUploadAttemptAt?.ISO8601Format() ?? "none")",
            "last_error_code=\(status?.lastUploadErrorCode ?? "none")",
        ]
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
        diagnosticsCopied = true
    }

    private var collectionDiagnosticCode: String {
        switch collectionActivityStatus.status {
        case .idle: "idle"
        case .running: "active"
        case .permissionRevoked: "permission_required"
        case .error: "error"
        }
    }

    private func clearSettingsSelection() {
        selectedSettingsDestination = nil
    }

    private func runDebugInsightSimulation() {
        guard let simulateNotification else { return }
        debugInsightStatus = "Preparing simulated insight…"
        Task {
            let result = await simulateNotification()
            switch result {
            case .scheduled:
                debugInsightStatus = "Insight updated and notification scheduled by macOS."
            case .permissionDenied:
                debugInsightStatus =
                    "Insight updated, but notifications are disabled in System Settings."
            case .schedulingFailed:
                debugInsightStatus =
                    "Insight updated, but macOS could not schedule the notification."
            }
        }
    }

    private func infoRow(_ title: String, _ value: String) -> some View {
    HStack {
      Text(title).foregroundStyle(.secondary)
      Spacer()
      Text(value).lineLimit(1).truncationMode(.middle)
    }
        .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }
    private func authenticationInfoRow() -> some View {
        let presentation = authenticationPresentation
        return HStack(spacing: 7) {
            Text("Authentication").foregroundStyle(.secondary)
            Spacer()
            Text(presentation.text)
                .lineLimit(1)
                .truncationMode(.middle)
            Circle()
                .fill(presentation.indicatorColor == .green ? Color.green : Color.red)
                .frame(width: 7, height: 7)
        }
        .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }
  private func statusRow(
    _ title: String, presentation: PopoverConnectionPresentation, refresh: @escaping () -> Void
  ) -> some View {
        HStack(spacing: 7) {
            Text(title).foregroundStyle(.secondary)
            Spacer()
            Button(action: refresh) {
                Text(presentation.label)
                    .foregroundStyle(presentation.color)
            }
            .buttonStyle(.plain)
            .help("Click to refresh status")
            Circle().fill(presentation.color).frame(width: 7, height: 7)
        }
        .font(.caption).padding(.horizontal, 16).padding(.vertical, 7)
    }

    private func submenuTitle(_ title: String) -> some View {
        Text(title)
            .font(.headline)
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
    }
}

/// The correction workbench, extracted so that it actually observes the model
/// it renders.
///
/// `MenuBarPopoverView` holds `MenuStatusViewModel` as a plain `let`, not an
/// `@ObservedObject` — an `@ObservedObject` cannot be optional. So nothing in
/// this surface was subscribed to the model that publishes corrections: the
/// queued rows, the saved-corrections list and the service's "correction
/// saved" confirmation all redrew only when some *unrelated* observed object
/// happened to publish. The confirmation clears itself after six seconds, so
/// whether the user ever saw the acknowledgement for the correction they just
/// made came down to whether an event happened to be captured inside that
/// window. Taking the model non-optionally here restores the subscription.
struct CorrectionWorkbenchView: View {
    @ObservedObject var menuStatus: MenuStatusViewModel
    @ObservedObject var localDashboard: LocalDashboardCoordinator
    let title: String
    @State private var confirmsReset = false

    var body: some View {
        VStack(spacing: 0) {
            Text(title)
                .font(.headline)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            explanation
            // The activity rows. They arrived here from the Patterns tab,
            // where the same data was drawn as a seven-day stacked chart with
            // a percentage column — the literal Screen Time artifact, and the
            // single strongest reason the product read as a tracker. The rows
            // are the affordance for choosing something to correct, so they
            // are kept; the week and the percentages are not.
            LocalActivityCorrectionList(
                snapshot: localDashboard.snapshot,
                onCorrectActivity: { segment, category, localName in
                    guard
                        let eventID = segment.representativeEventID,
                        let stableID = segment.stableID
                    else { return }
                    menuStatus.correct(
                        eventID: eventID,
                        stableID: stableID,
                        category: category,
                        localActivityName: localName
                    )
                },
                onUndoActivity: { segment in
                    guard let stableID = segment.stableID else { return }
                    menuStatus.undoCorrection(stableID: stableID)
                }
            )
            .padding(.horizontal, 16)
            .padding(.bottom, 10)

            Divider().padding(.vertical, 6)
            sectionLabel(
                "Waiting to sync (\(menuStatus.status?.queuedEventCount ?? 0))"
            )
            queuedEventRows

            Divider().padding(.vertical, 6)
            sectionLabel("Saved corrections")
            CorrectionHistoryBrowser(model: menuStatus)

            if let sendError = menuStatus.sendError {
                Text(sendError)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
            }
            // The correction is already saved by the time this appears.
            // Copy comes from the service verbatim so the confirmation says
            // exactly what changed and for how long.
            if let acknowledgment = menuStatus.correctionAcknowledgment {
                Label(acknowledgment, systemImage: "checkmark.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
                    .accessibilityLabel(acknowledgment)
            }

            Divider().padding(.top, 8)
            Button("Retry Cloud Synchronization") { menuStatus.sendAllNow() }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.top, 12)
            Button("Reset Local Activity Corrections", role: .destructive) {
                confirmsReset = true
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .onAppear {
            menuStatus.refresh()
            localDashboard.refresh()
        }
        // Redraw the activity rows off the service's own confirmation that a
        // correction was taken, not off the click that requested it.
        //
        // The two commands travel over one actor-isolated socket client on two
        // unstructured tasks, so a dashboard request fired immediately after a
        // correction can reach the router first and rebuild the rows from
        // pre-correction data. The router then has nothing further to push,
        // and `LocalDashboardCoordinator` only refreshes on a work-block
        // message, on reconnect, or on appear — so the corrected label could
        // stay wrong on screen indefinitely. The acknowledgement cannot arrive
        // before the correction has been written, which makes it the one
        // signal that is safe to refresh on.
        .onChange(of: menuStatus.correctionAcknowledgment) { acknowledgment in
            guard acknowledgment != nil else { return }
            localDashboard.refresh()
        }
        .confirmationDialog(
            "Reset all local activity and category corrections on this Mac?",
            isPresented: $confirmsReset,
            titleVisibility: .visible
        ) {
            Button("Reset Corrections", role: .destructive) {
                menuStatus.resetClassificationLearning()
            }
            Button("Cancel", role: .cancel) {}
        }
    }

    @ViewBuilder
    private var queuedEventRows: some View {
        let queuedEvents = Array((menuStatus.status?.queuedEvents ?? []).prefix(10))
        if queuedEvents.isEmpty {
            Text("Nothing is waiting to sync.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
        } else {
            // No scroll view of its own. The list is capped at ten rows by the
            // service, and a 190pt scroll view nested inside the submenu's own
            // scroll view meant a wheel gesture over these rows moved the
            // inner list and then stopped, instead of continuing down to the
            // saved corrections below it.
            VStack(alignment: .leading, spacing: 0) {
                ForEach(queuedEvents) { event in
                    QueuedEventCorrectionRow(
                        event: event,
                        onSave: { category, activityName in
                            menuStatus.correct(
                                event,
                                category: category,
                                localActivityName: activityName
                            )
                        },
                        onUndo: { menuStatus.undoCorrection(event) }
                    )
                }
            }
        }
    }

    private func sectionLabel(_ text: String) -> some View {
        Text(text)
            .font(.caption.bold())
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.bottom, 6)
    }

    private var explanation: some View {
        Text(Self.explanationCopy)
            .font(.caption2)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.bottom, 10)
            .accessibilityLabel(Self.explanationCopy)
    }

    /// 05 § 2, verbatim. It is the whole framing of this surface: not a report
    /// on the user, a place where the user corrects the software.
    static let explanationCopy =
        "Velvt gets these wrong sometimes. Fixing one here fixes it everywhere, on this Mac only — nothing about it ever syncs."
}

/// Shown when the correction workbench has no service connection behind it.
struct CorrectionWorkbenchUnavailableView: View {
    let title: String

    var body: some View {
        VStack(spacing: 0) {
            Text(title)
                .font(.headline)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            Text(
                "The local privacy service is not connected, so there is nothing to correct yet."
            )
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
        }
    }
}

private struct GuidedTourBar: View {
    @ObservedObject var model: GuidedTourModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            tourCopy
            controls
        }
        .padding(.horizontal, 14)
        .padding(.top, 11)
        .padding(.bottom, 16)
        .background(Color.velvtPanel)
        .accessibilityElement(children: .contain)
    }

    private var tourCopy: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("\(model.progressLabel) · \(model.step.title)")
                .font(.caption.bold())
                .foregroundStyle(Color.velvtText)
            Text(model.step.detail)
                .font(.caption2)
                .foregroundStyle(Color.velvtMuted)
                .lineLimit(2)
                .accessibilityLabel(model.step.detail)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Tour step \(model.progressLabel), \(model.step.title)")
        .accessibilityValue(model.step.detail)
    }

    private var controls: some View {
        HStack(alignment: .center) {
            Button("Skip tour") { model.dismiss() }
                .buttonStyle(.plain)
            Spacer(minLength: 16)
            HStack(spacing: 8) {
                Button("Back") { model.goBack() }
                    .disabled(!model.canGoBack)
                Button(model.isLastStep ? "Done" : "Next") { model.advance() }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .tint(Color.velvtPink)
    }
}

private extension String {
    var nilIfBlank: String? {
        trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : self
    }
}

private struct MenuBarAccountControls: View {
    @ObservedObject private var accountStateManager: AccountStateManager
    @StateObject private var authViewModel: AuthViewModel
    @State private var authenticationMode: AuthViewModel.AuthMode = .logIn
    @State private var showsAuthentication = false
    init(accountStateManager: AccountStateManager, ipcClient: any IPCClientProtocol) {
        self.accountStateManager = accountStateManager
    _authViewModel = StateObject(
      wrappedValue: AuthViewModel(accountStateManager: accountStateManager, ipcClient: ipcClient))
    }
    var body: some View {
        Group {
            switch accountStateManager.accountState {
            case .loggingIn: ProgressView("Signing in").controlSize(.small)
            case .loggingOut: ProgressView("Signing out").controlSize(.small)
      case .pendingErasure:
        Text("Account deletion in progress").font(.caption).foregroundStyle(.secondary)
            default:
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 8) {
            ForEach(
              Array(
                MenuBarAccountActionResolver.actions(for: accountStateManager.accountState)
                  .enumerated()), id: \.offset
            ) { _, action in actionButton(for: action) }
                    }
                    if let error = authViewModel.errorMessage {
                        Text(error)
                            .font(.caption2)
                            .foregroundStyle(.red)
                    }
                }
            }
        }
        .sheet(isPresented: $showsAuthentication) {
      MenuBarAuthenticationView(
        authViewModel: authViewModel, accountStateManager: accountStateManager,
        initialMode: authenticationMode, dismiss: { showsAuthentication = false })
        }
    }
    @ViewBuilder private func actionButton(for action: MenuBarAccountAction) -> some View {
        switch action {
    case .authenticate(let mode):
      Button(mode == .logIn ? signInLabel : "Sign Up") {
        authenticationMode = mode
        authViewModel.authMode = mode
        showsAuthentication = true
      }
        case .logOut: Button("Log Out", role: .destructive) { authViewModel.logOut() }
        case .deleteAccount: EmptyView()
        }
    }
    private var signInLabel: String {
        accountStateManager.requiresReauthentication ? "Reauthenticate" : "Sign In"
    }
}

private struct SettingsAccountDeletionButton: View {
    @ObservedObject private var accountStateManager: AccountStateManager
    @StateObject private var authViewModel: AuthViewModel

    init(accountStateManager: AccountStateManager, ipcClient: any IPCClientProtocol) {
        self.accountStateManager = accountStateManager
        _authViewModel = StateObject(
            wrappedValue: AuthViewModel(
                accountStateManager: accountStateManager,
                ipcClient: ipcClient
            )
        )
    }

    var body: some View {
        if case .loggedIn = accountStateManager.accountState {
            Button("Delete Account", role: .destructive) {
                authViewModel.requestAccountDeletion()
            }
            .buttonStyle(.bordered)
            .confirmationDialog(
                "Delete your Velvt account? This request cannot be undone.",
                isPresented: Binding(
                    get: { authViewModel.showDeleteConfirmation },
                    set: { if !$0 { authViewModel.cancelAccountDeletion() } }
                ),
                titleVisibility: .visible
            ) {
                Button("Delete Account", role: .destructive) {
                    Task { await authViewModel.confirmAccountDeletion() }
                }
                Button("Cancel", role: .cancel) { authViewModel.cancelAccountDeletion() }
            } message: {
                Text(
                    "Velvt deletes behavioral data and disables authentication. It retains only an anonymized account record and the erasure/audit records required to prove deletion completed."
                )
            }
        }
    }
}

private struct MenuBarAuthenticationView: View {
    @ObservedObject var authViewModel: AuthViewModel
    @ObservedObject var accountStateManager: AccountStateManager
    let initialMode: AuthViewModel.AuthMode
    let dismiss: () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
      Text(authViewModel.authMode == .signUp ? "Create your account" : "Welcome back").font(
        .title3.bold())
            CredentialTextField(placeholder: "Email", text: $authViewModel.email)
            CredentialTextField(placeholder: "Password", text: $authViewModel.password, isSecure: true)
            if let error = authViewModel.errorMessage { Text(error).font(.caption).foregroundStyle(.red) }
      HStack {
        Button("Cancel", action: dismiss)
        Spacer()
        Button(authViewModel.authMode == .signUp ? "Create Account" : "Sign In") {
          Task {
            if authViewModel.authMode == .signUp {
              await authViewModel.signUp()
            } else {
              await authViewModel.logIn()
            }
          }
        }
        .buttonStyle(.borderedProminent)
        .disabled(!authViewModel.canSubmitCredentials)
      }
      Button(
        authViewModel.authMode == .signUp ? "I already have an account" : "Create a new account"
      ) { authViewModel.toggleAuthMode() }.buttonStyle(.plain).font(.caption).foregroundStyle(
        .secondary)
    }.padding(24).frame(width: 360).onAppear { authViewModel.authMode = initialMode }.onChange(
      of: accountStateManager.accountState
    ) { if case .loggedIn = $0 { dismiss() } }
    }
}
