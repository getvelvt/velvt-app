import AppKit
import Combine
import Foundation

/// Main-actor presentation bridge for Rust-owned meaningful-work state.
///
/// This type issues direct user/OS lifecycle commands and publishes the exact
/// Rust snapshot. It does not persist intention text, aggregate events, derive
/// evidence, or author behavioral claims.
@MainActor
public final class WorkBlockCoordinator: ObservableObject, InterventionDeliveryRecording {
    @Published public private(set) var snapshot: WorkBlockSnapshot?
    @Published public private(set) var commandError: String?
    /// The Rust-authored quiet-hours offer, present until the user replies.
    /// Swift renders it verbatim and never re-derives the pattern.
    @Published public private(set) var quietHoursOffer: QuietHoursOffer?
    /// The Rust-authored initiation invitation, present until the user
    /// replies or the service invalidates it. Swift renders the body verbatim
    /// and never re-derives good hours, caps, or backoff.
    @Published public private(set) var invitation: InitiationInvitation?
    /// The Rust-owned invitation opt-out state, mirrored for the settings
    /// toggle. Swift never assumes it; it renders what the service reports.
    @Published public private(set) var invitationsEnabled = true
    /// The Rust-owned auto-demotion state, mirrored for the disclosure card.
    /// Swift renders the reported counts and copy verbatim; the state machine,
    /// its versioned threshold, and re-promotion live entirely in Rust.
    @Published public private(set) var demotionState: DemotionState?
    /// The Rust-generated weekly receipts digest, present until acknowledged.
    /// Swift renders the stored counts verbatim and never recomputes one.
    @Published public private(set) var weeklyDigest: WeeklyDigest?
    /// The one grounded explanation sentence for the live intervention.
    /// Present only while that intervention's card is showing; there is no
    /// input, reply, or thread anywhere on this surface.
    @Published public private(set) var explanation: InterventionExplanation?
    /// The Rust-owned setting for the question after a block (proto v38),
    /// mirrored for the Settings toggle. `nil` until the service reports it,
    /// and the toggle is drawn only while Rust says it is `available`.
    @Published public private(set) var ratingPromptSettings: RatingPromptSettings?
    /// The block whose "Answer the question" link was opened. Opening it is a
    /// disclosure on this Mac's screen, not a fact the service records: the
    /// Not now that put the link there already is.
    @Published private var ratingLinkOpenedFor: UUID?
    /// An answer sent and not yet confirmed by the service's reply.
    @Published private var ratingAnswerInFlight: RatingAcknowledgement?
    /// The acknowledgement for the block just answered, shown above its card
    /// once the service's reply no longer carries the question.
    @Published private var ratingAcknowledgement: RatingAcknowledgement?

    private let ipcClient: any IPCClientProtocol
    private var cancellables = Set<AnyCancellable>()
    private var sendChain: Task<Void, Never>?
    private let utcOffsetSeconds: () -> Int
    private let flushPendingDwell: (@MainActor () async -> Void)?
    private let now: () -> Date
    private let sleep: @Sendable (TimeInterval) async throws -> Void
    private var deadlineFlush: (key: DeadlineFlushKey, task: Task<Void, Never>)?
    private var ratingExpiryRefresh: (key: RatingExpiryKey, task: Task<Void, Never>)?

    /// How long before a block's planned end the dwell in progress is closed
    /// for a block that runs out rather than being ended.
    ///
    /// The service finishes a timed-out block itself, at `ends_at`, and the
    /// only thing Swift hears afterwards is the finished snapshot. A report
    /// that arrives then lands on a block that is no longer active, so it has
    /// to arrive first. One second costs the block one second of coverage.
    nonisolated static let deadlineFlushLeadSeconds: TimeInterval = 1

    private struct DeadlineFlushKey: Equatable {
        let blockID: UUID
        let endsAt: Date
    }

    private struct RatingExpiryKey: Equatable {
        let blockID: UUID
        let expiresAt: Date
    }

    /// - Parameter flushPendingDwell: Closes the dwell the person is in right
    ///   now and returns once its report has gone to the service. The service
    ///   is told about a dwell only when it ends, so without this the one in
    ///   progress at the end of a block never reaches it, and the block's
    ///   result drops it. Called only at the end of an active block and just
    ///   before `ends_at`, never at a pause or a sleep (see `pause()`).
    ///   `nil` sends the commands alone.
    public init(
        ipcClient: any IPCClientProtocol,
        utcOffsetSeconds: @escaping () -> Int = { TimeZone.current.secondsFromGMT() },
        flushPendingDwell: (@MainActor () async -> Void)? = nil,
        now: @escaping () -> Date = Date.init,
        sleep: @escaping @Sendable (TimeInterval) async throws -> Void = { seconds in
            try await Task.sleep(for: .seconds(seconds))
        }
    ) {
        self.ipcClient = ipcClient
        self.utcOffsetSeconds = utcOffsetSeconds
        self.flushPendingDwell = flushPendingDwell
        self.now = now
        self.sleep = sleep
    }

    public func start(
        messages: some Publisher<ServerMessage, Never>,
        connectionStatus: some Publisher<ConnectionStatus, Never>,
        workspaceNotifications: NotificationCenter = NSWorkspace.shared.notificationCenter,
        systemNotifications: NotificationCenter = .default
    ) {
        messages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in
                switch message {
                case .workBlockState(let snapshot):
                    self?.snapshot = snapshot
                    self?.commandError = nil
                    self?.scheduleDeadlineFlush(for: snapshot)
                    // A live block supersedes any invitation card; the service has
                    // already expired the stored invitation.
                    if snapshot.phase == .active || snapshot.phase == .paused {
                        self?.invitation = nil
                    }
                    // The sentence explains exactly one shown card: it leaves with it.
                    if snapshot.activeIntervention == nil {
                        self?.explanation = nil
                    }
                    self?.reconcileQuestionAfterABlock(with: snapshot)
                case .ratingPromptSettings(let settings):
                    self?.ratingPromptSettings = settings
                case .quietHoursOffer(let offer):
                    self?.quietHoursOffer = offer
                case .initiationInvitation(let invitation):
                    self?.invitation = invitation
                case .initiationSettings(let settings):
                    self?.invitationsEnabled = settings.invitationsEnabled
                    if !settings.invitationsEnabled {
                        self?.invitation = nil
                    }
                case .demotionState(let state):
                    self?.demotionState = state
                case .weeklyDigest(let digest):
                    self?.weeklyDigest = digest
                case .interventionExplanation(let explanation):
                    if self?.snapshot?.blockID == explanation.blockID {
                        self?.explanation = explanation
                    }
                case .errorResponse(let error)
                where error.code.hasPrefix("work_block_")
                    || error.code.hasPrefix("invalid_work_block_"):
                    self?.commandError = error.message
                    // An answer to the question after a block is answered with
                    // a snapshot or, when the service cannot build one, with
                    // this error. Either settles it, so the five answers are
                    // offered again instead of staying disabled until the
                    // next snapshot. Nothing is acknowledged.
                    self?.ratingAnswerInFlight = nil
                default:
                    break
                }
            }
            .store(in: &cancellables)

        connectionStatus
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                guard status == .connected else { return }
                self?.send(.requestWorkBlockState)
                self?.send(.requestInitiationSettings)
                self?.refreshRatingPromptSettings()
                self?.refreshInvitation()
                self?.refreshDemotionState()
                self?.refreshWeeklyDigest()
            }
            .store(in: &cancellables)

        // The service pauses an active block on sleep. Like a pause, it
        // sends no flush first: see `pause()`.
        workspaceNotifications.publisher(for: NSWorkspace.willSleepNotification)
            .sink { [weak self] _ in self?.reportLifecycle(.sleep) }
            .store(in: &cancellables)
        workspaceNotifications.publisher(for: NSWorkspace.didWakeNotification)
            .sink { [weak self] _ in
                self?.reportLifecycle(.wake)
                self?.refreshInvitation()
                self?.refreshWeeklyDigest()
            }
            .store(in: &cancellables)
        systemNotifications.publisher(for: .NSSystemClockDidChange)
            .sink { [weak self] _ in self?.reportLifecycle(.clockChanged) }
            .store(in: &cancellables)
        systemNotifications.publisher(for: .NSSystemTimeZoneDidChange)
            .sink { [weak self] _ in self?.reportLifecycle(.timeZoneChanged) }
            .store(in: &cancellables)
    }

    public func startBlock(
        intention: String?,
        durationSeconds: Int,
        purpose: WorkBlockPurpose?,
        intensity: WorkBlockIntensity
    ) {
        send(
            .startWorkBlock(
                .init(
                    intention: intention,
                    plannedDurationSeconds: durationSeconds,
                    purpose: purpose,
                    intensity: intensity
                )))
    }

    /// Pauses the block without reporting the dwell in progress first.
    ///
    /// A flush closes the dwell and re-opens it at the moment of the pause.
    /// The service recognises a closed report as the continuation of a dwell
    /// the drift gate already decided on by that dwell's start
    /// (`continues_decided_dwell` in `work_block/mod.rs`); after a re-anchor
    /// the closed report sent once the block resumes no longer matches, so
    /// the gate decided on it afresh and could make an offer that the
    /// pre-registered drift policy (version 5) does not make. Decisions under
    /// that policy must not change, so neither a pause nor the sleep that
    /// pauses a block flushes.
    public func pause() {
        guard let blockID = snapshot?.blockID else { return }
        send(.pauseWorkBlock(.init(blockID: blockID)))
    }

    public func resume() {
        guard let blockID = snapshot?.blockID else { return }
        send(.resumeWorkBlock(.init(blockID: blockID)))
    }

    /// Reports the dwell in progress, then ends the block.
    ///
    /// The order is the fix. The service closes the block's last ledger row
    /// where that dwell's closed report says it ended, and a dwell is
    /// reported closed only when the person leaves it. Ended without the
    /// report, a block spent entirely in one app had no measured time at all.
    /// A paused block is ended without a flush: the block is not measuring
    /// time while paused.
    public func end() {
        guard let blockID = snapshot?.blockID else { return }
        cancelDeadlineFlush()
        send(
            .endWorkBlock(.init(blockID: blockID)),
            flushingDwellFirst: snapshot?.phase == .active)
    }

    public func acceptRecovery() {
        guard let blockID = snapshot?.blockID,
            let actionID = snapshot?.result?.nextAction.actionID
        else { return }
        send(.acceptWorkBlockRecovery(.init(blockID: blockID, actionID: actionID)))
    }

    /// Sends the user's explicit reply to a live drift offer.
    ///
    /// Guarded on an unanswered offer being present so a stale view cannot report
    /// against a card the service has already resolved.
    public func respondToIntervention(_ response: InterventionResponse) {
        guard let blockID = snapshot?.blockID,
            snapshot?.activeIntervention != nil
        else { return }
        send(.reportInterventionOutcome(.init(blockID: blockID, response: response)))
    }

    /// Reports that the drift card was actually on screen.
    ///
    /// Not a reply, and it must never be treated as one: it records that the
    /// offer reached the user, so an unanswered offer can afterwards be told
    /// apart from one that was never delivered. Guarded on an unanswered offer
    /// for the same reason the reply path is — a stale view must not report
    /// against a card the service has already resolved. The service keeps the
    /// first sighting, so re-opening the popover is harmless.
    public func reportInterventionCardSeen() {
        guard let blockID = snapshot?.blockID,
            snapshot?.activeIntervention != nil
        else { return }
        send(.interventionCardSeen(.init(blockID: blockID)))
    }

    /// Reports what became of the OS notification for `blockID`'s drift offer
    /// (proto v35).
    ///
    /// Not a reply, and it must never be treated as one. Unlike the commands
    /// here it is not guarded on a live offer: a withdrawal is reported once
    /// the attempt it interrupted returns, after the offer is gone, and the
    /// service keeps the fact for a resolved offer too. It clears no error and
    /// sets none, because nothing the person asked for depends on it; a
    /// service that is offline never hears it, and the row keeps no channel.
    public func reportInterventionDelivery(blockID: UUID, channel: InterventionDeliveryChannel) {
        enqueue { [ipcClient] in
            try? await ipcClient.send(.interventionDelivery(.init(blockID: blockID, channel: channel)))
        }
    }

    /// Sends the one-tap reply to a quiet-hours offer and dismisses the card.
    /// Declining changes nothing else; the service remembers it locally.
    public func respondToQuietHoursOffer(accepted: Bool) {
        guard quietHoursOffer != nil else { return }
        quietHoursOffer = nil
        send(.respondQuietHoursOffer(.init(accepted: accepted)))
    }

    /// Asks the service whether one invitation is pending right now. The
    /// service owns every gate; asking is always safe and never doubles a
    /// live invitation.
    public func refreshInvitation() {
        send(.requestInitiationInvitation(.init(utcOffsetSeconds: utcOffsetSeconds())))
    }

    /// Reads the current Rust-owned demotion state for the disclosure card.
    public func refreshDemotionState() {
        send(.requestDemotionState)
    }

    /// The user's explicit one-tap resume from the demoted state. The service
    /// restarts its evaluation window and replies with the new state.
    public func resetDemotion() {
        guard demotionState?.state == .demoted else { return }
        send(.resetInterventionDemotion)
    }

    /// Asks the service whether the weekly receipts digest is ready. The
    /// service owns generation, count sourcing, and the quiet-hours/Focus
    /// holds; asking is always safe.
    public func refreshWeeklyDigest() {
        send(.requestWeeklyDigest(.init(utcOffsetSeconds: utcOffsetSeconds())))
    }

    /// The one-tap close on the digest card. Bookkeeping only.
    public func acknowledgeWeeklyDigest() {
        guard let weeklyDigest else { return }
        self.weeklyDigest = nil
        send(
            .acknowledgeWeeklyDigest(
                .init(weekStartLocalDate: weeklyDigest.weekStartLocalDate)))
    }

    /// The one-tap "explain this nudge". Guarded on a live card so a stale
    /// view cannot ask about an intervention the service has resolved; the
    /// request carries no user text and there is no follow-up.
    public func requestExplanation() {
        guard let blockID = snapshot?.blockID,
            snapshot?.activeIntervention != nil
        else { return }
        send(
            .requestInterventionExplanation(
                .init(blockID: blockID, utcOffsetSeconds: utcOffsetSeconds())))
    }

    /// One tap on the invitation starts the declared block through the
    /// existing start command, carrying the invitation id so the service can
    /// record the content-free origin marker.
    public func acceptInvitation() {
        guard let invitation else { return }
        self.invitation = nil
        send(
            .startWorkBlock(
                .init(
                    intention: nil,
                    plannedDurationSeconds: invitation.durationSeconds,
                    purpose: nil,
                    intensity: .medium,
                    invitationID: invitation.invitationID
                )))
    }

    /// The one-tap dismissal. Recorded by the service; only ever reduces
    /// future invitations.
    public func dismissInvitation() {
        guard let invitation else { return }
        self.invitation = nil
        send(.dismissInitiationInvitation(.init(invitationID: invitation.invitationID)))
    }

    /// The single opt-out. The service owns and enforces the setting; the
    /// toggle re-renders from the service's reply.
    public func setInvitationsEnabled(_ enabled: Bool) {
        if !enabled {
            invitation = nil
        }
        send(.setInitiationSettings(.init(invitationsEnabled: enabled)))
    }

    // MARK: The question after a block (proto v38)

    /// What the question after a block puts on a result surface right now.
    ///
    /// Read from Rust's snapshot of an ended block and from the taps this
    /// coordinator has sent, and from nothing else. Every case carries
    /// Rust's words. `nil` on a live or idle block whatever the snapshot
    /// carries, because the question is never drawn during a block, and once
    /// the deadline Rust gave has passed, because an answer then would be
    /// refused.
    public var questionAfterABlock: QuestionAfterABlock? {
        guard let snapshot, snapshot.phase.hasEnded, let blockID = snapshot.blockID else {
            return nil
        }
        if let ratingAcknowledgement, ratingAcknowledgement.blockID == blockID {
            return .acknowledged(ratingAcknowledgement.text)
        }
        if let prompt = snapshot.ratingPrompt, prompt.blockID == blockID, now() < prompt.expiresAt {
            switch prompt.presentation {
            case .question:
                return .question(prompt, viaLink: false)
            case .link:
                return ratingLinkOpenedFor == blockID ? .question(prompt, viaLink: true) : .link(prompt)
            }
        }
        if let optIn = snapshot.ratingOptIn {
            return .invitation(optIn)
        }
        return nil
    }

    /// Whether the ended block's observation and numbers are held back on
    /// every surface that shows them. True only while the question is up for
    /// the first time: the card would anchor the answer, and an answer given
    /// after the card is a separate stratum that Rust records only through
    /// Not now or the link.
    public var withholdsBlockSummary: Bool {
        if case .question(_, viaLink: false) = questionAfterABlock { true } else { false }
    }

    /// Whether an answer has been sent and the service has not replied yet.
    /// The answers are disabled meanwhile so one tap is one answer.
    public var isSendingRatingAnswer: Bool {
        ratingAnswerInFlight != nil
    }

    /// Reports that the question was drawn on a surface someone can see. A
    /// fact, never an answer; the service keeps the first. It clears no
    /// error and sets none, because nothing the person asked for depends on
    /// it.
    public func reportRatingShown() {
        guard case .question(let prompt, viaLink: false) = questionAfterABlock else { return }
        enqueue { [ipcClient] in
            try? await ipcClient.send(.workBlockRatingShown(.init(blockID: prompt.blockID)))
        }
    }

    /// One of the five answers. `via_link` is set when the question was
    /// reopened from the link under the card, so Rust files the answer as
    /// given after the card was seen.
    public func answerRating(_ answer: WorkBlockRatingAnswer) {
        guard ratingAnswerInFlight == nil,
            case .question(let prompt, let viaLink) = questionAfterABlock
        else { return }
        ratingAnswerInFlight = RatingAcknowledgement(
            blockID: prompt.blockID, text: prompt.acknowledgement, expiresAt: prompt.expiresAt)
        send(
            .rateWorkBlock(.init(blockID: prompt.blockID, answer: answer, viaLink: viaLink)),
            onFailure: { [weak self] in self?.ratingAnswerInFlight = nil })
    }

    /// Not now on the question. The first time, it is reported, and Rust
    /// answers with the card and the link. On a question reopened from the
    /// link it closes what the link opened and reports nothing: that Not now
    /// is already on record.
    public func declineRatingForNow() {
        guard case .question(let prompt, let viaLink) = questionAfterABlock else { return }
        if viaLink {
            ratingLinkOpenedFor = nil
            return
        }
        send(.workBlockRatingNotNow(.init(blockID: prompt.blockID)))
    }

    /// The quiet link under the card after Not now: brings the question back
    /// above the card, which now stays visible.
    public func openRatingLink() {
        guard case .link(let prompt) = questionAfterABlock else { return }
        ratingLinkOpenedFor = prompt.blockID
    }

    /// A tap on the one-time invitation: which button, and nothing else.
    /// Rust decides what it records and answers with the snapshot, which no
    /// longer carries the invitation; the setting is read again so the
    /// Settings toggle agrees.
    public func respondToRatingInvitation(turnOn: Bool) {
        guard case .invitation = questionAfterABlock else { return }
        send(.respondRatingOptIn(.init(turnOn: turnOn)))
        refreshRatingPromptSettings()
    }

    /// The Settings toggle. The service owns and enforces the setting, and
    /// refuses on until this Mac's first 14 days of blocks are over; the
    /// toggle re-renders from its reply. That reply does not resend the
    /// snapshot, so an ended block's state is asked for again: turning the
    /// question off withdraws an open question, and the result view must not
    /// keep drawing it.
    public func setRatingPromptEnabled(_ enabled: Bool) {
        send(.setRatingPromptEnabled(.init(enabled: enabled)))
        if snapshot?.phase.hasEnded == true {
            send(.requestWorkBlockState)
        }
    }

    /// Reads the question's setting for the Settings pane.
    public func refreshRatingPromptSettings() {
        send(.requestRatingPromptSettings)
    }

    /// Settles the question's local state against a new snapshot, then arms
    /// the one refresh at the question's deadline.
    ///
    /// The first snapshot after an answer settles it either way. If it is
    /// for the answered block, no longer carries the question, and arrived
    /// before the question's deadline, the answer was taken and its
    /// acknowledgement shows. A reply at or after the deadline acknowledges
    /// nothing: the service may have refused the answer as late, and that
    /// reply omits the question too, so the result shows without "Noted.".
    /// If the reply still carries the question, the answer was not recorded,
    /// and the answers are offered again rather than left disabled. State
    /// about another block is dropped.
    private func reconcileQuestionAfterABlock(with snapshot: WorkBlockSnapshot) {
        if let inFlight = ratingAnswerInFlight {
            if snapshot.blockID == inFlight.blockID, snapshot.ratingPrompt == nil,
                now() < inFlight.expiresAt
            {
                ratingAcknowledgement = inFlight
            }
            ratingAnswerInFlight = nil
        }
        if let acknowledged = ratingAcknowledgement?.blockID, acknowledged != snapshot.blockID {
            ratingAcknowledgement = nil
        }
        if let opened = ratingLinkOpenedFor,
            opened != snapshot.blockID || snapshot.ratingPrompt?.presentation != .link
        {
            ratingLinkOpenedFor = nil
        }
        scheduleRatingExpiryRefresh(for: snapshot)
    }

    /// Asks for the ended block's state once, at the question's deadline.
    ///
    /// The service publishes state on commands and one block deadline, never
    /// on a timer, so without this a panel left open past the deadline keeps
    /// drawing a question the service would refuse an answer to. The reply
    /// is Rust's, without the question. Re-armed only when the block or the
    /// deadline changes, and disarmed by any snapshot without a question.
    private func scheduleRatingExpiryRefresh(for snapshot: WorkBlockSnapshot) {
        guard snapshot.phase.hasEnded,
            let prompt = snapshot.ratingPrompt,
            prompt.blockID == snapshot.blockID
        else {
            cancelRatingExpiryRefresh()
            return
        }
        let key = RatingExpiryKey(blockID: prompt.blockID, expiresAt: prompt.expiresAt)
        guard ratingExpiryRefresh?.key != key else { return }
        cancelRatingExpiryRefresh()
        let delay = max(0, prompt.expiresAt.timeIntervalSince(now()))
        let task = Task { [weak self, sleep] in
            do {
                try await sleep(delay)
            } catch {
                return
            }
            guard !Task.isCancelled, let self,
                self.snapshot?.ratingPrompt?.blockID == prompt.blockID
            else { return }
            self.enqueue { [ipcClient = self.ipcClient] in
                try? await ipcClient.send(.requestWorkBlockState)
            }
        }
        ratingExpiryRefresh = (key, task)
    }

    private func cancelRatingExpiryRefresh() {
        ratingExpiryRefresh?.task.cancel()
        ratingExpiryRefresh = nil
    }

    public func clearLocalData() {
        quietHoursOffer = nil
        invitation = nil
        weeklyDigest = nil
        explanation = nil
        demotionState = nil
        ratingLinkOpenedFor = nil
        ratingAnswerInFlight = nil
        ratingAcknowledgement = nil
        send(.clearWorkBlockData)
        // The cleared record derives a fresh (active) demotion state.
        refreshDemotionState()
    }

    #if DEBUG
        /// Debug-only synthetic invitation so the packaged debug build can
        /// demonstrate the surface without touching the local database. Mirrors
        /// `NotificationDeliveryCoordinator.simulateDebugInsightReceipt`: the
        /// one deliberate place a debug harness authors a payload. Accepting
        /// sends a real start command; the service does not recognize the
        /// synthetic id and records a plain manual start.
        public func simulateDebugInvitation() {
            invitation = InitiationInvitation(
                invitationID: UUID(),
                actionID: "soft_start_25",
                body: "You usually focus well around now — want a 25-minute soft start?",
                durationSeconds: 1_500,
                policyVersion: 1
            )
        }

        /// Debug-only synthetic demoted state so the packaged debug build can
        /// demonstrate the disclosure without a real wrong-intervention stream.
        /// Resuming sends the real reset command, which the service answers
        /// with its true (active) state.
        public func simulateDebugDemotion() {
            demotionState = DemotionState(
                state: .demoted,
                wrongCount: 4,
                deliveredCount: 16,
                thresholdPercent: 15,
                minimumSample: 10,
                windowDays: 14,
                thresholdPolicyVersion: 1,
                repromotionPolicyVersion: 1,
                demotedAt: Date(),
                disclosure:
                    "Velvt is getting these nudges wrong too often, so it has gone quiet: no nudges will be sent for now, and you can resume them at any time."
            )
        }

        /// Debug-only synthetic weekly digest so the packaged debug build can
        /// demonstrate the receipts surface. Acknowledging sends the real
        /// command; the service ignores an unknown week.
        public func simulateDebugWeeklyDigest() {
            weeklyDigest = WeeklyDigest(
                weekStartLocalDate: "2026-07-27",
                blocksDeclared: 5,
                blocksCompleted: 3,
                recoveries: 4,
                wrongInterventions: 1,
                invitationsAccepted: 2,
                withheld: 1,
                headline: "You returned 4 times and completed 3 of 5 blocks this week.",
                digestVersion: 1
            )
        }
    #endif

    public func reportLifecycle(_ event: WorkBlockLifecycleEvent) {
        send(.workBlockLifecycle(.init(event: event)))
    }

    /// Arms the flush for a block that runs out instead of being ended.
    ///
    /// Re-armed only when the block or its `ends_at` changes: a resume moves
    /// the deadline, and every other snapshot of the same block leaves the
    /// pending flush alone. Anything but an active block disarms it, so a
    /// block that was ended or paused is never flushed again later.
    private func scheduleDeadlineFlush(for snapshot: WorkBlockSnapshot) {
        guard flushPendingDwell != nil,
            snapshot.phase == .active,
            let blockID = snapshot.blockID,
            let endsAt = snapshot.endsAt
        else {
            cancelDeadlineFlush()
            return
        }
        let key = DeadlineFlushKey(blockID: blockID, endsAt: endsAt)
        guard deadlineFlush?.key != key else { return }
        cancelDeadlineFlush()
        guard endsAt > now() else { return }
        let delay = max(0, endsAt.timeIntervalSince(now()) - Self.deadlineFlushLeadSeconds)
        let task = Task { [weak self, sleep] in
            do {
                try await sleep(delay)
            } catch {
                return
            }
            guard !Task.isCancelled, let self,
                self.snapshot?.blockID == blockID,
                self.snapshot?.phase == .active
            else { return }
            self.enqueue { [weak self] in await self?.flushPendingDwell?() }
        }
        deadlineFlush = (key, task)
    }

    private func cancelDeadlineFlush() {
        deadlineFlush?.task.cancel()
        deadlineFlush = nil
    }

    private func send(
        _ message: ClientMessage,
        flushingDwellFirst: Bool = false,
        onFailure: (@MainActor () -> Void)? = nil
    ) {
        commandError = nil
        let flush = flushingDwellFirst ? flushPendingDwell : nil
        enqueue { [weak self, ipcClient] in
            await flush?()
            do {
                try await ipcClient.send(message)
            } catch {
                await MainActor.run {
                    self?.commandError = "The local service is offline. Your work block was not changed."
                    onFailure?()
                }
            }
        }
    }

    /// Runs `operation` after everything this coordinator has already
    /// queued, so a flush and the command it precedes reach the service in
    /// the order they were asked for.
    private func enqueue(_ operation: @escaping @MainActor () async -> Void) {
        let previous = sendChain
        sendChain = Task {
            await previous?.value
            await operation()
        }
    }
}

/// Owns the Rust-authored bounded live dashboard payload. Swift only stores
/// and renders the received snapshot; it does not inspect event history or
/// derive switch rates.
@MainActor
public final class LocalDashboardCoordinator: ObservableObject {
    @Published public private(set) var snapshot: LocalDashboardSnapshot?
    @Published public private(set) var commandError: String?

    private let ipcClient: any IPCClientProtocol
    private var cancellables = Set<AnyCancellable>()

    public init(ipcClient: any IPCClientProtocol) {
        self.ipcClient = ipcClient
    }

    public func start(
        messages: some Publisher<ServerMessage, Never>,
        connectionStatus: some Publisher<ConnectionStatus, Never>
    ) {
        messages
            .receive(on: RunLoop.main)
            .sink { [weak self] message in
                switch message {
                case .localDashboard(let snapshot):
                    self?.snapshot = snapshot
                    self?.commandError = nil
                case .workBlockState:
                    self?.refresh()
                case .errorResponse(let error) where error.code == "local_dashboard_unavailable":
                    self?.commandError = error.message
                default:
                    break
                }
            }
            .store(in: &cancellables)

        connectionStatus
            .removeDuplicates()
            .receive(on: RunLoop.main)
            .sink { [weak self] status in
                guard status == .connected else { return }
                self?.refresh()
            }
            .store(in: &cancellables)
    }

    public func refresh() {
        Task {
            do {
                try await ipcClient.send(
                    .requestLocalDashboard(
                        .init(
                            windowSeconds: 3600,
                            utcOffsetSeconds: TimeZone.current.secondsFromGMT()
                        )
                    )
                )
            } catch {
                commandError = "The local dashboard is temporarily unavailable."
            }
        }
    }
}

final class UnavailableWorkBlockIPCClient: IPCClientProtocol {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { $0.finish() }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        Just(.disconnected).eraseToAnyPublisher()
    }
    func connect() async throws {}
    func disconnect() {}
    func send(_ message: ClientMessage) async throws { throw IPCError.notConnected }
}

final class UnavailableLocalDashboardIPCClient: IPCClientProtocol {
    let incomingMessages: AsyncStream<ServerMessage> = AsyncStream { $0.finish() }
    var connectionStatus: AnyPublisher<ConnectionStatus, Never> {
        Just(.disconnected).eraseToAnyPublisher()
    }
    func connect() async throws {}
    func disconnect() {}
    func send(_ message: ClientMessage) async throws { throw IPCError.notConnected }
}
