import SwiftUI

public enum WorkBlockDurationChoice: String, CaseIterable, Identifiable {
  case twentyFive
  case fifty
  case custom

  public var id: String { rawValue }
}

/// The duration window the local service accepts for a declared block.
///
/// Mirrored from `planned_duration_seconds INTEGER NOT NULL CHECK(... BETWEEN
/// 300 AND 10800)` in `0009_work_blocks.sql` and re-checked in
/// `WorkBlockManager::start`, which answers anything outside it with
/// `invalid_work_block_request`. Every surface that can send a start command
/// reads the bounds from here so the stepper's range and the suggested-action
/// buttons' gate cannot drift apart from each other or from the schema.
public enum WorkBlockDurationLimits {
  public static let minimumSeconds = 300
  public static let maximumSeconds = 10_800
  public static let minutesRange = (minimumSeconds / 60)...(maximumSeconds / 60)
  public static let minuteStep = 5

  public static func acceptsMinutes(_ minutes: Int) -> Bool {
    (minimumSeconds...maximumSeconds).contains(minutes * 60)
  }
}

public struct WorkBlockView: View {
  @ObservedObject private var coordinator: WorkBlockCoordinator
  @State private var intention = ""
  @State private var durationChoice: WorkBlockDurationChoice = .twentyFive
  @State private var customMinutes = 30
  @State private var purpose: WorkBlockPurpose?
  @State private var intensity: WorkBlockIntensity = .medium
  @State private var plansAnotherSession = false

  public init(coordinator: WorkBlockCoordinator) {
    self.coordinator = coordinator
  }

  /// Snapshot-render seam. The duration controls are `@State`, so the
  /// minimum and the maximum a person can actually choose cannot be looked
  /// at without a way to seed them. Internal, so only the test bundle can
  /// reach it; the shipping call site is the public initializer above.
  init(
    coordinator: WorkBlockCoordinator,
    durationChoice: WorkBlockDurationChoice,
    customMinutes: Int
  ) {
    self.coordinator = coordinator
    _durationChoice = State(initialValue: durationChoice)
    _customMinutes = State(initialValue: customMinutes)
  }

  public var body: some View {
    Group {
      if let offer = coordinator.quietHoursOffer {
        quietHoursOfferCard(offer)
      }
      if let demotion = coordinator.demotionState, demotion.state == .demoted {
        demotionDisclosureCard(demotion)
      }
      if let digest = coordinator.weeklyDigest {
        weeklyDigestCard(digest)
      }
      // The invitation and the drift offer are not drawn here. This view's one
      // call site is the popover behind the panel's primary button, and each
      // card was reachable only by taking the action it exists to prompt:
      // `WorkBlockProactiveCards`, drawn by the panel body, is their one home.
      // A live block outranks the planning form. The form used to win, so
      // "Plan another session" followed by an invitation accepted anywhere
      // else left a start form sitting on top of a running block; and the
      // Start button used to clear the flag on press, which flashed the
      // finished block's result card back for however long the round trip
      // took. The flag is cleared when the service confirms, below.
      if let snapshot = coordinator.snapshot,
        snapshot.phase == .active || snapshot.phase == .paused
      {
        activeBlock(snapshot)
      } else if plansAnotherSession {
        startForm
      } else if let snapshot = coordinator.snapshot {
        switch snapshot.phase {
        case .idle:
          startForm
        case .active, .paused:
          EmptyView()
        case .completed, .abandoned, .expired:
          resultView(snapshot)
        }
      } else {
        HStack(spacing: VelvtMetrics.spaceSM) {
          ProgressView().controlSize(.small).tint(VelvtPalette.signal)
          Text("Loading local work block…")
            .font(VelvtType.caption())
            .foregroundStyle(VelvtInk.tertiaryOnInk)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(VelvtMetrics.cardPadding)
      }
    }
    .accessibilityElement(children: .contain)
    .onAppear { coordinator.refreshInvitation() }
    .onChange(of: coordinator.snapshot?.phase) { phase in
      // The form closes when the service confirms the block, not when the
      // button is pressed. If the send fails there is nothing to go back to
      // and the typed intention goes with it; the intention is cleared here
      // instead, once it has been handed over, so it is not sitting in the
      // field for the next session.
      if phase == .active || phase == .paused {
        plansAnotherSession = false
        intention = ""
      }
    }
  }

  private var startForm: some View {
    VStack(alignment: .leading, spacing: VelvtMetrics.spaceMD) {
      Text("Start a focus session")
        .velvtHeading()

      Text("Choose the time and kind of work you want to protect.")
        .velvtBody(12)
        .fixedSize(horizontal: false, vertical: true)

      TextField("Intention (optional)", text: $intention)
        .textFieldStyle(.roundedBorder)
        .font(VelvtType.body(12))
        .tint(VelvtPalette.crimson)
        .onChange(of: intention) { value in
          intention = String(value.prefix(120)).replacingOccurrences(of: "\n", with: " ")
        }
        .accessibilityLabel("Optional local intention")
        .accessibilityHint("Stored only on this Mac for a short time")

      Text("Duration")
        .font(VelvtType.label())
        .tracking(VelvtType.labelTracking)
        .textCase(.uppercase)
        .foregroundStyle(VelvtInk.labelOnInk)

      Picker("Duration", selection: $durationChoice) {
        Text("25 min").tag(WorkBlockDurationChoice.twentyFive)
        Text("50 min").tag(WorkBlockDurationChoice.fifty)
        Text("Custom").tag(WorkBlockDurationChoice.custom)
      }
      .pickerStyle(.segmented)
      .labelsHidden()
      .font(VelvtType.body(12))
      .tint(VelvtPalette.crimson)

      if durationChoice == .custom {
        Stepper(
          "\(customMinutes) minutes", value: $customMinutes,
          in: WorkBlockDurationLimits.minutesRange, step: WorkBlockDurationLimits.minuteStep)
          .font(VelvtType.caption())
          .foregroundStyle(VelvtInk.secondaryOnInk)
          .accessibilityLabel("Custom duration")
          .accessibilityValue("\(customMinutes) minutes")
      }

      HStack(alignment: .top, spacing: VelvtMetrics.spaceMD) {
        VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
          Text("Work type")
            .font(VelvtType.label())
            .tracking(VelvtType.labelTracking)
            .textCase(.uppercase)
            .foregroundStyle(VelvtInk.labelOnInk)
          Picker("Work type", selection: $purpose) {
            Text("General focus").tag(nil as WorkBlockPurpose?)
            ForEach(WorkBlockPurpose.allCases) { value in
              Text(purposeLabel(value)).tag(value as WorkBlockPurpose?)
            }
          }
          .labelsHidden()
          .font(VelvtType.body(12))
          .tint(VelvtPalette.crimson)
          .frame(maxWidth: .infinity)
        }

        VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
          Text("Guidance")
            .font(VelvtType.label())
            .tracking(VelvtType.labelTracking)
            .textCase(.uppercase)
            .foregroundStyle(VelvtInk.labelOnInk)
          Picker("Guidance", selection: $intensity) {
            ForEach(WorkBlockIntensity.allCases) { value in
              Text(intensityLabel(value)).tag(value)
            }
          }
          .labelsHidden()
          .font(VelvtType.body(12))
          .tint(VelvtPalette.crimson)
          .frame(maxWidth: .infinity)
        }
      }

      Text(intensityExplanation)
        .font(VelvtType.caption(10.5))
        .foregroundStyle(VelvtInk.tertiaryOnInk)
        .fixedSize(horizontal: false, vertical: true)

      if let error = coordinator.commandError {
        Text(error)
          .font(VelvtType.caption())
          .foregroundStyle(VelvtPalette.signal)
          .accessibilityLabel("Work block error: \(error)")
      }

      Button(startButtonLabel) {
        coordinator.startBlock(
          intention: intention.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty,
          durationSeconds: durationSeconds,
          purpose: purpose,
          intensity: intensity
        )
      }
      .buttonStyle(VelvtPrimaryButtonStyle())
      .keyboardShortcut(.defaultAction)
      .accessibilityHint("Starts this bounded work block on the local service")
    }
    .padding(VelvtMetrics.cardPadding)
  }

  /// The demotion disclosure (D5; roadmap invariant 4): shown as respect,
  /// never hidden. The body copy is Rust-authored and rendered verbatim;
  /// the detail line shows the exact counts and versioned constants the
  /// deterministic rule evaluated, and the one button is the manual reset.
  private func demotionDisclosureCard(_ state: DemotionState) -> some View {
    VelvtCard(padding: VelvtMetrics.spaceMD) {
      VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
        Label(DigestFraming.demotionTitle, systemImage: "pause.circle")
          .velvtHeading(14)

        if let disclosure = state.disclosure {
          Text(disclosure)
            .velvtBody(12)
            .fixedSize(horizontal: false, vertical: true)
        }

        Text(DigestFraming.demotionDetail(state))
          .font(VelvtType.caption(10.5))
          .foregroundStyle(VelvtInk.tertiaryOnInk)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityLabel("Demotion detail. \(DigestFraming.demotionDetail(state))")

        HStack(spacing: VelvtMetrics.spaceSM) {
          Button(DigestFraming.resumeLabel) {
            coordinator.resetDemotion()
          }
          .buttonStyle(VelvtPrimaryButtonStyle())
          .accessibilityHint("Resumes nudges; the evidence record is unchanged")

          Spacer(minLength: 0)
        }
      }
    }
    .padding([.horizontal, .top], VelvtMetrics.cardPadding)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(
      "Velvt has gone quiet. \(state.disclosure ?? DigestFraming.demotionDetail(state))")
  }

  /// The weekly receipts digest (D6, D8): one card, not a dashboard.
  /// Recoveries and completions lead, the wrong-intervention count appears
  /// exactly once, and every number is the stored count verbatim.
  private func weeklyDigestCard(_ digest: WeeklyDigest) -> some View {
    WeeklyDigestCard(digest: digest, onAcknowledge: coordinator.acknowledgeWeeklyDigest)
      .padding([.horizontal, .top], VelvtMetrics.cardPadding)
  }

  /// The next-morning quiet-hours offer. One tap accepts; declining is a
  /// single calm action the service remembers. Copy comes from Rust
  /// verbatim, and the card never re-asks on its own.
  private func quietHoursOfferCard(_ offer: QuietHoursOffer) -> some View {
    VelvtCard(padding: VelvtMetrics.spaceMD) {
      VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
        Label("Quiet hours", systemImage: "moon")
          .velvtHeading(14)

        Text(offer.body)
          .velvtBody(12)
          .fixedSize(horizontal: false, vertical: true)

        HStack(spacing: VelvtMetrics.spaceSM) {
          Button("Turn on quiet hours") {
            coordinator.respondToQuietHoursOffer(accepted: true)
          }
          .buttonStyle(VelvtPrimaryButtonStyle())
          .accessibilityHint("Velvt holds its own notifications overnight")

          Button("No thanks") {
            coordinator.respondToQuietHoursOffer(accepted: false)
          }
          .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
          .accessibilityHint("Keeps everything exactly as it is")

          Spacer(minLength: 0)
        }
      }
    }
    .padding([.horizontal, .top], VelvtMetrics.cardPadding)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Quiet hours offer. \(offer.body)")
  }

  private func activeBlock(_ snapshot: WorkBlockSnapshot) -> some View {
    VStack(alignment: .leading, spacing: VelvtMetrics.spaceMD) {
      HStack(alignment: .firstTextBaseline) {
        VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
          Text(snapshot.phase == .paused ? "Work block paused" : "Work block active")
            .velvtHeading()
          if let intention = snapshot.intention {
            Text(intention)
              .velvtBody(12)
              .lineLimit(1)
              .truncationMode(.tail)
          }
        }
        Spacer()
        if snapshot.recoveredAfterRestart {
          // Sage, the brand's one affirmative hue, and deliberately quiet:
          // coming back is worth marking, never worth rewarding.
          Text("Recovered")
            .font(VelvtType.label(9.5))
            .tracking(VelvtType.labelTracking)
            .textCase(.uppercase)
            .foregroundStyle(VelvtInk.affirmative)
        }
      }

      HStack(spacing: VelvtMetrics.spaceLG) {
        timeColumn(
          "Elapsed", seconds: snapshot.elapsedDurationSeconds, snapshot: snapshot, countsDown: false
        )
        timeColumn(
          "Remaining", seconds: snapshot.remainingDurationSeconds, snapshot: snapshot,
          countsDown: true)
        VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
          Text("Category")
            .font(VelvtType.label(9.5))
            .tracking(VelvtType.labelTracking)
            .textCase(.uppercase)
            .foregroundStyle(VelvtInk.tertiaryOnInk)
          Text(categoryLabel(snapshot.currentCategory))
            .font(VelvtType.bodyEmphasis(12))
            .foregroundStyle(VelvtInk.primaryOnInk)
            .lineLimit(1)
        }
      }

      Text(snapshot.statusLine)
        .velvtBody(12)
        .fixedSize(horizontal: false, vertical: true)

      if let error = coordinator.commandError {
        Text(error)
          .font(VelvtType.caption())
          .foregroundStyle(VelvtPalette.signal)
      }

      HStack {
        if snapshot.phase == .paused {
          Button("Resume") { coordinator.resume() }
            .keyboardShortcut(.defaultAction)
        } else {
          Button("Pause") { coordinator.pause() }
            .keyboardShortcut("p", modifiers: [.command])
        }
        Spacer()
        Button("End", role: .destructive) { coordinator.end() }
          // A custom ButtonStyle discards SwiftUI's `role: .destructive`
          // presentation, so on the secondary style End rendered identically
          // to Pause beside it. The destructive style, applied closer to the
          // button than the row's, restores the distinction.
          .buttonStyle(VelvtDestructiveButtonStyle())
      }
      // Pause and Resume are calm, equal choices, so neither takes the one
      // crimson action on the surface: that slot belongs to the drift offer
      // when there is one.
      .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
    }
    .padding(VelvtMetrics.cardPadding)
  }

  private func resultView(_ snapshot: WorkBlockSnapshot) -> some View {
    VStack(alignment: .leading, spacing: VelvtMetrics.spaceMD) {
      Text(resultTitle(snapshot.phase))
        .velvtHeading()
      Text(snapshot.statusLine)
        .velvtBody(12)

      if let result = snapshot.result {
        Text(result.observation)
          .font(VelvtType.body(13))
          .lineSpacing(VelvtType.bodySpacing(13))
          .foregroundStyle(VelvtInk.primaryOnInk)
          .fixedSize(horizontal: false, vertical: true)

        Text(evidenceLabel(result))
          .font(VelvtType.caption())
          .foregroundStyle(VelvtInk.secondaryOnInk)
          .fixedSize(horizontal: false, vertical: true)

        // The one calm DND reconciliation line, authored in Rust: what
        // completed and what was held. Positive framing; never a late nudge.
        if let reconciliation = result.reconciliation {
          Label(reconciliation, systemImage: "moon")
            .font(VelvtType.caption())
            .foregroundStyle(VelvtInk.secondaryOnInk)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel("Do Not Disturb summary. \(reconciliation)")
        }

        // The qualifier goes above the numbers it qualifies, in the same
        // words the local dashboard's work-block card already uses, from the
        // same two fields. This card shows elapsed and the dashboard card
        // shows elapsed, and only one of them said what the other numbers
        // beside it were measured over: `25m / 25m` next to a 17-second
        // longest stretch is unreadable without it. `nil` when coverage is
        // good, so this is not a second notice bolted onto the existing one
        // — it is the existing one, on the other surface that shows elapsed.
        if let notice = coverageNotice(result) {
          Text(notice)
            .font(VelvtType.caption(10.5))
            .foregroundStyle(VelvtInk.tertiaryOnInk)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel(notice)
        }

        // Recoveries lead, and are counted rather than rated. A number that
        // can only go up cannot be lost, which is what a streak gets wrong:
        // coming back four times is the achievement, not going unbroken.
        // Switch-aways stay visible as the denominator, never as a score.
        HStack(spacing: VelvtMetrics.spaceMD) {
          VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
            Text("Recoveries")
              .font(VelvtType.label(9.5))
              .tracking(VelvtType.labelTracking)
              .textCase(.uppercase)
              .foregroundStyle(VelvtInk.tertiaryOnInk)
            Text("\(result.recoveryCount)")
              .font(VelvtType.measurement(13).monospacedDigit())
              .foregroundStyle(VelvtPalette.signal)
          }
          resultMetric("Longest stretch", result.longestUninterruptedSeconds)
          resultMetric("Elapsed", result.elapsedDurationSeconds)
          VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
            Text("Switch-aways")
              .font(VelvtType.label(9.5))
              .tracking(VelvtType.labelTracking)
              .textCase(.uppercase)
              .foregroundStyle(VelvtInk.tertiaryOnInk)
            Text("\(result.switchAwayCount)")
              .font(VelvtType.body(12).monospacedDigit())
              .foregroundStyle(VelvtInk.tertiaryOnInk)
          }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
          "Came back \(result.recoveryCount) times after \(result.switchAwayCount) switch-aways"
        )

        Text(coverageLabel(result))
          .font(VelvtType.caption(10.5))
          .foregroundStyle(VelvtInk.tertiaryOnInk)

        // The gentle re-entry action, offered by Rust only on an invited
        // block that ended early. Label comes from the registry verbatim;
        // it takes the prominent slot and the default shortcut when shown.
        if result.nextAction.actionID == "soft_restart_10" {
          Button(result.nextAction.label) { coordinator.acceptRecovery() }
            .buttonStyle(VelvtPrimaryButtonStyle())
            .keyboardShortcut(.defaultAction)
            .accessibilityHint("Starts a ten-minute block on the local service")

          planAnotherButton(prominent: false)
        } else {
          planAnotherButton(prominent: true)
        }
      } else {
        // A terminal block with no result row on the snapshot. The service
        // writes one on finish, but a block that ended while the app was not
        // running comes back through restart recovery without one, and
        // `clearWorkBlockData` removes it. Every button used to live inside
        // `if let result`, so this state rendered a title, a status line, and
        // no way out of itself at all.
        planAnotherButton(prominent: true)
      }

      if let error = coordinator.commandError {
        Text(error)
          .font(VelvtType.caption())
          .foregroundStyle(VelvtPalette.signal)
      }
    }
    .padding(VelvtMetrics.cardPadding)
  }

  @ViewBuilder
  private func planAnotherButton(prominent: Bool) -> some View {
    if prominent {
      Button("Plan another session") { plansAnotherSession = true }
        .buttonStyle(VelvtPrimaryButtonStyle())
        .keyboardShortcut(.defaultAction)
        .accessibilityHint("Choose the next session's work type and duration")
    } else {
      Button("Plan another session") { plansAnotherSession = true }
        .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
        .accessibilityHint("Choose the next session's work type and duration")
    }
  }

  /// Elapsed and remaining for a live block.
  ///
  /// Both live values hang off `ends_at`, which is the only number on this
  /// snapshot that stays true as wall-clock advances. The service publishes
  /// state on commands and on one deadline sleep and never on a timer, so
  /// `elapsed_duration_seconds` was true when the message was sent and is
  /// stale by exactly however long the panel took to open. Anchoring a
  /// count-up on `now - elapsed` therefore restarted the clock at whatever
  /// the last push happened to say: ten minutes into a block whose start
  /// command was the last push, Elapsed read 0:00 and kept counting from
  /// there.
  ///
  /// Nothing is re-derived. The service defines
  /// `ends_at = started_at + planned + total_paused` and
  /// `remaining = planned - elapsed`, so:
  ///
  ///     remaining(now) = ends_at - now
  ///     elapsed(now)   = now - (ends_at - planned)
  ///
  /// `ends_at` rather than `started_at` is what carries paused time, which is
  /// why the two agree across a pause and resume without Swift ever holding a
  /// `total_paused_seconds` of its own.
  @ViewBuilder
  private func timeColumn(
    _ title: String,
    seconds: Int,
    snapshot: WorkBlockSnapshot,
    countsDown: Bool
  ) -> some View {
    VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
      Text(title)
        .font(VelvtType.label(9.5))
        .tracking(VelvtType.labelTracking)
        .textCase(.uppercase)
        .foregroundStyle(VelvtInk.tertiaryOnInk)
      if snapshot.phase == .active, let endsAt = snapshot.endsAt {
        if countsDown {
          Text(timerInterval: Date()...max(Date(), endsAt), countsDown: true)
            .font(VelvtType.measurement(15).monospacedDigit())
            .foregroundStyle(VelvtInk.primaryOnInk)
        } else {
          Text(
            timerInterval: endsAt.addingTimeInterval(
              -TimeInterval(snapshot.plannedDurationSeconds))...Date.distantFuture,
            countsDown: false
          )
          .font(VelvtType.measurement(15).monospacedDigit())
          .foregroundStyle(VelvtInk.primaryOnInk)
        }
      } else {
        // Paused, or an active block the service sent without a deadline.
        // The last number it published, in the same shape the running timer
        // draws, so pausing a ninety-minute block cannot turn 1:29:00 into
        // 89:00. The old branching ran a count-*up* here whenever `ends_at`
        // was missing, including for the Remaining column.
        Text(DurationText.clock(seconds))
          .font(VelvtType.measurement(15).monospacedDigit())
          .foregroundStyle(VelvtInk.primaryOnInk)
      }
    }
  }

  private func resultMetric(_ title: String, _ seconds: Int) -> some View {
    VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
      Text(title)
        .font(VelvtType.label(9.5))
        .tracking(VelvtType.labelTracking)
        .textCase(.uppercase)
        .foregroundStyle(VelvtInk.tertiaryOnInk)
      Text(DurationText.compact(seconds))
        .font(VelvtType.measurement(13).monospacedDigit())
        .foregroundStyle(VelvtInk.primaryOnInk)
    }
  }

  private var durationSeconds: Int {
    switch durationChoice {
    case .twentyFive: 25 * 60
    case .fifty: 50 * 60
    case .custom: customMinutes * 60
    }
  }

  private var startButtonLabel: String {
    "Start \(durationSeconds / 60)-minute session"
  }

  private var intensityExplanation: String {
    switch intensity {
    case .light: "Light uses short, minimal status wording."
    case .medium: "Medium uses balanced status wording."
    case .intense: "Intense is more explicit, with the same calm, non-shaming evidence rules."
    }
  }

  private func purposeLabel(_ value: WorkBlockPurpose) -> String {
    switch value {
    case .deepWork: "Deep work"
    case .study: "Study"
    case .creativePractice: "Creative practice"
    case .healthyTechUse: "Healthy tech use"
    case .workLifeBoundary: "Work-life boundary"
    }
  }

  private func intensityLabel(_ value: WorkBlockIntensity) -> String {
    value.rawValue.capitalized
  }

  private func categoryLabel(_ value: String?) -> String {
    guard let value else { return "Unclear" }
    return value.replacingOccurrences(of: "_", with: " ").capitalized
  }

  private func resultTitle(_ phase: WorkBlockPhase) -> String {
    switch phase {
    case .completed: "Work block complete"
    case .abandoned: "Work block ended"
    case .expired: "Work block expired"
    default: "Work block result"
    }
  }

  /// The sentence the local dashboard's work-block card already shows, from
  /// the same two stored fields. `nil` when coverage is good — there is
  /// nothing to qualify — which is also what keeps this from becoming a
  /// second low-coverage warning.
  private func coverageNotice(_ result: WorkBlockResult) -> String? {
    CoverageNotice.sentence(
      isGood: result.coverage == .good,
      isEmpty: result.coverage == .insufficient && result.coverageRatio <= 0,
      coverageRatio: result.coverageRatio,
      switchLabel: "switch-aways")
  }

  private func coverageLabel(_ result: WorkBlockResult) -> String {
    let confidence = "\(result.confidence.rawValue.capitalized) confidence"
    // When the sentence above the numbers has already stated the fraction,
    // this line does not restate it: one coverage statement per card.
    guard coverageNotice(result) == nil else { return confidence }
    return "\(result.coverage.rawValue.capitalized) coverage · \(confidence)"
  }

  private func evidenceLabel(_ result: WorkBlockResult) -> String {
    guard let category = result.safeEvidenceCategory else {
      return "Evidence: no supported category was recorded."
    }
    return "Evidence category: \(categoryLabel(category))."
  }

}

extension String {
  fileprivate var nilIfEmpty: String? { isEmpty ? nil : self }
}

/// A card Velvt raises without being asked.
public enum WorkBlockProactiveCard: Equatable {
  /// The drift offer for a block that is running. Time-critical, so it leads.
  case intervention
  /// The at-most-one-a-day initiation invitation.
  case invitation
}

/// The two cards Velvt raises on its own initiative, as their own view so the
/// panel body can draw them.
///
/// Both used to live inside `WorkBlockView`, whose one call site is the popover
/// behind the panel's primary button. An invitation exists to ask someone to
/// declare a block who has not decided to, and a drift offer exists to reach
/// someone whose attention has already gone elsewhere, so each was reachable
/// only by taking the action it exists to prompt. A notification tap opens the
/// panel, not that popover, so the offer's one instruction landed on a surface
/// with no reply buttons on it. The weekly digest was lifted out of the same
/// popover for the same reason; these two were what was left.
///
/// One home each, deliberately. The panel and the focus-session popover can be
/// on screen together, and two sets of reply buttons for one drift offer is two
/// chances to record a reply the person did not make.
///
/// The panel's view tree outlives the panel being on screen: the window is
/// ordered out on close, not torn down. So `onAppear` alone no longer means
/// "someone could see this", and the drift card's sighting (`card_seen_at`,
/// migration 0032) is reported only while `surfaceIsOnScreen` says the surface
/// drawing it is actually on screen: when the card appears on a visible panel,
/// or when a panel already holding the card comes on screen.
public struct WorkBlockProactiveCards: View {
  @ObservedObject private var coordinator: WorkBlockCoordinator
  private let surfaceIsOnScreen: Bool

  public init(coordinator: WorkBlockCoordinator, surfaceIsOnScreen: Bool) {
    self.coordinator = coordinator
    self.surfaceIsOnScreen = surfaceIsOnScreen
  }

  /// Which cards this view draws right now, in the order it draws them.
  ///
  /// The only inputs are the coordinator's state. Whether the focus-session
  /// popover is open is not one of them, which is the point of the move; view
  /// composition is not observable from a unit test and this is.
  var presentedCards: [WorkBlockProactiveCard] {
    var cards: [WorkBlockProactiveCard] = []
    if coordinator.snapshot?.activeIntervention != nil {
      cards.append(.intervention)
    }
    if coordinator.invitation != nil {
      cards.append(.invitation)
    }
    return cards
  }

  public var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      if let intervention = coordinator.snapshot?.activeIntervention {
        interventionCard(intervention)
          // Reported from the render, and only while the surface is on screen:
          // an offer that lands while the panel is closed has not reached
          // anyone, and saying otherwise would recreate the exact ambiguity
          // `card_seen_at` records its way out of. The service keeps the first
          // sighting, so reporting again on every opening is harmless.
          .onAppear { reportSightingIfOnScreen() }
          .onChange(of: surfaceIsOnScreen) { _ in reportSightingIfOnScreen() }
      }
      if let invitation = coordinator.invitation {
        invitationCard(invitation)
      }
    }
    .accessibilityElement(children: .contain)
  }

  /// Reports the drift card as seen when, and only when, the surface drawing
  /// it is on screen. The coordinator adds its own guard: nothing is sent
  /// unless an unanswered offer is live.
  func reportSightingIfOnScreen() {
    guard surfaceIsOnScreen else { return }
    coordinator.reportInterventionCardSeen()
  }

  /// The in-app surface for a live drift offer.
  ///
  /// This is where a reply comes from, and the only place one can be made. The
  /// notification carries the offer to someone who is not looking at the panel;
  /// this card is what they find when they open it, on whichever tab it opens
  /// on. An OS notification depends on authorization and is suppressed by
  /// Focus, so this is the primary path, not a fallback. Every reply is
  /// recorded, so silence stays distinguishable from disagreement.
  ///
  /// Copy comes from Rust verbatim. Swift does not reinterpret the evidence or
  /// offer an action outside the registry.
  private func interventionCard(_ intervention: ActiveIntervention) -> some View {
    // Paper stock: a drift offer is one sentence addressed to the person, not
    // a panel of data about them, and the guide reserves paper for exactly
    // that. The offer itself sits in the blush inset — the tint the guide
    // keeps for a small experiment the reader is free to decline.
    VelvtPaperCard(padding: VelvtMetrics.spaceMD) {
      VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
        Text(intervention.title)
          .velvtHeading(14, onPaper: true)

        Text(intervention.body)
          .velvtBody(12, onPaper: true)
          .fixedSize(horizontal: false, vertical: true)

        VelvtInsetPanel {
          HStack(spacing: VelvtMetrics.spaceSM) {
            Button("Back to work") {
              coordinator.respondToIntervention(.acceptedAction)
            }
            .buttonStyle(VelvtPrimaryButtonStyle())

            Spacer(minLength: 0)

            // Declining carries the same weight of presence as accepting. A
            // dismissal drawn as a faint glyph next to a filled button is
            // pressure, and pressure is the one thing the guide rules out.
            Button {
              coordinator.respondToIntervention(.dismissed)
            } label: {
              Image(systemName: "xmark")
            }
            .buttonStyle(VelvtSecondaryButtonStyle(onPaper: true))
            .accessibilityLabel("Dismiss this suggestion")
          }
        }

        // Disagreement is evidence against the detector, so each kind of "you
        // were wrong" is a first-class reply rather than a shrug. "I was
        // focused" leads: it is the only reply that says the offer should never
        // have fired, and a false positive Velvt cannot see is one it cannot
        // stop making.
        HStack(spacing: VelvtMetrics.spaceMD) {
          Button("I was focused") {
            coordinator.respondToIntervention(.wasFocused)
          }
          .accessibilityHint("Tells Velvt this suggestion was wrong — you were working")

          Button("Wrong category") {
            coordinator.respondToIntervention(.wrongClassification)
          }

          Button("Not helpful") {
            coordinator.respondToIntervention(.notHelpful)
          }

          Spacer(minLength: 0)
        }
        .buttonStyle(.plain)
        .font(VelvtType.caption())
        .foregroundStyle(VelvtInk.secondaryOnPaper)

        // The one-tap explanation (D7): the sentence is Rust-authored from
        // the stored evidence and rendered verbatim. One sentence, no input
        // field, no reply, no thread — this affordance is the chat gate, not
        // a chat.
        if let explanation = coordinator.explanation {
          Label(explanation.sentence, systemImage: "text.magnifyingglass")
            .font(VelvtType.caption(10.5))
            .lineSpacing(VelvtType.bodySpacing(10.5))
            .foregroundStyle(VelvtInk.tertiaryOnPaper)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel("Explanation. \(explanation.sentence)")
        } else {
          Button(DigestFraming.explainLabel) {
            coordinator.requestExplanation()
          }
          .buttonStyle(.plain)
          .font(VelvtType.caption(10.5))
          .foregroundStyle(VelvtInk.labelOnPaper)
          .accessibilityHint("Shows one sentence about the evidence behind this nudge")
        }
      }
    }
    .padding([.horizontal, .top], VelvtMetrics.cardPadding)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("\(intervention.title). \(intervention.body)")
  }

  /// The initiation invitation. At most one per day, extended by the
  /// deterministic Rust policy; Swift renders the body verbatim and can
  /// only accept (a declared block through the existing start command) or
  /// dismiss. Declining is calm and costless.
  private func invitationCard(_ invitation: InitiationInvitation) -> some View {
    VelvtCard(padding: VelvtMetrics.spaceMD) {
      VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
        Label("Soft start", systemImage: "sunrise")
          .velvtHeading(14)

        Text(invitation.body)
          .velvtBody(12)
          .fixedSize(horizontal: false, vertical: true)

        HStack(spacing: VelvtMetrics.spaceSM) {
          Button("Start now") {
            coordinator.acceptInvitation()
          }
          .buttonStyle(VelvtPrimaryButtonStyle())
          .accessibilityHint("Starts a declared soft-start block on the local service")

          Button("Not now") {
            coordinator.dismissInvitation()
          }
          .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
          .accessibilityHint("Dismisses this invitation; future invitations only get rarer")

          Spacer(minLength: 0)
        }
      }
    }
    .padding([.horizontal, .top], VelvtMetrics.cardPadding)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Soft start invitation. \(invitation.body)")
  }
}

/// This week's receipts, as their own view so more than one surface can show
/// them.
///
/// The digest used to be reachable only from inside the focus-session sheet,
/// behind "Start a focus session" — so a completed week's receipts existed,
/// were correct, and were invisible unless the user happened to open the one
/// sheet that renders them. A summary of the week belongs on the tab named for
/// the week. Acknowledging in either place clears it in both, because both read
/// the same coordinator.
public struct WeeklyDigestCard: View {
  let digest: WeeklyDigest
  let onAcknowledge: () -> Void

  public init(digest: WeeklyDigest, onAcknowledge: @escaping () -> Void) {
    self.digest = digest
    self.onAcknowledge = onAcknowledge
  }

  public var body: some View {
    VelvtCard(padding: VelvtMetrics.spaceMD) {
      VStack(alignment: .leading, spacing: VelvtMetrics.spaceSM) {
        Label(DigestFraming.digestTitle, systemImage: "doc.plaintext")
          .velvtHeading(14)

        Text(digest.headline)
          .velvtBody(12)
          .fixedSize(horizontal: false, vertical: true)

        VStack(alignment: .leading, spacing: VelvtMetrics.spaceXS) {
          row(DigestFraming.returnedLabel, digest.recoveries)
          row(DigestFraming.completedLabel, digest.blocksCompleted)
          row(DigestFraming.declaredLabel, digest.blocksDeclared)
          row(DigestFraming.invitationsLabel, digest.invitationsAccepted)
          row(DigestFraming.wrongLabel, digest.wrongInterventions)
          row(DigestFraming.withheldLabel, digest.withheld)
        }

        HStack(spacing: VelvtMetrics.spaceSM) {
          Button(DigestFraming.acknowledgeLabel, action: onAcknowledge)
            .buttonStyle(VelvtSecondaryButtonStyle(onPaper: false))
            .accessibilityHint("Closes this week's receipts")
          Spacer(minLength: 0)
        }
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Weekly receipts. \(digest.headline)")
  }

  private func row(_ label: String, _ count: Int) -> some View {
    HStack {
      Text(label)
        .font(VelvtType.caption(11))
        .foregroundStyle(VelvtInk.secondaryOnInk)
      Spacer(minLength: VelvtMetrics.spaceSM)
      // Every row is the stored count, drawn the same way. Nothing here is
      // colour-coded better or worse than anything else.
      Text("\(count)")
        .font(VelvtType.measurement(11.5).monospacedDigit())
        .foregroundStyle(VelvtInk.primaryOnInk)
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(label), \(count)")
  }
}
