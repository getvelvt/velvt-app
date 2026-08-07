import AppKit
import Combine
import Foundation

/// Main-actor presentation bridge for Rust-owned meaningful-work state.
///
/// This type issues direct user/OS lifecycle commands and publishes the exact
/// Rust snapshot. It does not persist intention text, aggregate events, derive
/// evidence, or author behavioral claims.
@MainActor
public final class WorkBlockCoordinator: ObservableObject {
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

  private let ipcClient: any IPCClientProtocol
  private var cancellables = Set<AnyCancellable>()
  private var sendChain: Task<Void, Never>?
  private let utcOffsetSeconds: () -> Int

  public init(
    ipcClient: any IPCClientProtocol,
    utcOffsetSeconds: @escaping () -> Int = { TimeZone.current.secondsFromGMT() }
  ) {
    self.ipcClient = ipcClient
    self.utcOffsetSeconds = utcOffsetSeconds
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
          // A live block supersedes any invitation card; the service has
          // already expired the stored invitation.
          if snapshot.phase == .active || snapshot.phase == .paused {
            self?.invitation = nil
          }
          // The sentence explains exactly one shown card: it leaves with it.
          if snapshot.activeIntervention == nil {
            self?.explanation = nil
          }
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
        self?.refreshInvitation()
        self?.refreshDemotionState()
        self?.refreshWeeklyDigest()
      }
      .store(in: &cancellables)

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

  public func pause() {
    guard let blockID = snapshot?.blockID else { return }
    send(.pauseWorkBlock(.init(blockID: blockID)))
  }

  public func resume() {
    guard let blockID = snapshot?.blockID else { return }
    send(.resumeWorkBlock(.init(blockID: blockID)))
  }

  public func end() {
    guard let blockID = snapshot?.blockID else { return }
    send(.endWorkBlock(.init(blockID: blockID)))
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

  public func clearLocalData() {
    quietHoursOffer = nil
    invitation = nil
    weeklyDigest = nil
    explanation = nil
    demotionState = nil
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

  private func send(_ message: ClientMessage) {
    commandError = nil
    let previous = sendChain
    sendChain = Task { [weak self, ipcClient] in
      await previous?.value
      do {
        try await ipcClient.send(message)
      } catch {
        await MainActor.run {
          self?.commandError = "The local service is offline. Your work block was not changed."
        }
      }
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
