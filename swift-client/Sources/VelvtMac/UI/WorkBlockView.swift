import SwiftUI

public enum WorkBlockDurationChoice: String, CaseIterable, Identifiable {
  case twentyFive
  case fifty
  case custom

  public var id: String { rawValue }
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
      if let invitation = coordinator.invitation {
        invitationCard(invitation)
      }
      if plansAnotherSession {
        startForm
      } else if let snapshot = coordinator.snapshot {
        switch snapshot.phase {
        case .idle:
          startForm
        case .active, .paused:
          activeBlock(snapshot)
        case .completed, .abandoned, .expired:
          resultView(snapshot)
        }
      } else {
        HStack(spacing: 8) {
          ProgressView().controlSize(.small)
          Text("Loading local work block…")
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
      }
    }
    .accessibilityElement(children: .contain)
    .onAppear { coordinator.refreshInvitation() }
  }

  private var startForm: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Start a focus session")
        .font(.headline)

      Text("Choose the time and kind of work you want to protect.")
        .font(.caption)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      TextField("Intention (optional)", text: $intention)
        .textFieldStyle(.roundedBorder)
        .onChange(of: intention) { value in
          intention = String(value.prefix(120)).replacingOccurrences(of: "\n", with: " ")
        }
        .accessibilityLabel("Optional local intention")
        .accessibilityHint("Stored only on this Mac for a short time")

      Text("Duration")
        .font(.caption.bold())
        .foregroundStyle(.secondary)

      Picker("Duration", selection: $durationChoice) {
        Text("25 min").tag(WorkBlockDurationChoice.twentyFive)
        Text("50 min").tag(WorkBlockDurationChoice.fifty)
        Text("Custom").tag(WorkBlockDurationChoice.custom)
      }
      .pickerStyle(.segmented)
      .labelsHidden()

      if durationChoice == .custom {
        Stepper("\(customMinutes) minutes", value: $customMinutes, in: 5...180, step: 5)
          .font(.caption)
          .accessibilityLabel("Custom duration")
          .accessibilityValue("\(customMinutes) minutes")
      }

      HStack(alignment: .top, spacing: 12) {
        VStack(alignment: .leading, spacing: 5) {
          Text("Work type")
            .font(.caption.bold())
            .foregroundStyle(.secondary)
          Picker("Work type", selection: $purpose) {
            Text("General focus").tag(nil as WorkBlockPurpose?)
            ForEach(WorkBlockPurpose.allCases) { value in
              Text(purposeLabel(value)).tag(value as WorkBlockPurpose?)
            }
          }
          .labelsHidden()
          .frame(maxWidth: .infinity)
        }

        VStack(alignment: .leading, spacing: 5) {
          Text("Guidance")
            .font(.caption.bold())
            .foregroundStyle(.secondary)
          Picker("Guidance", selection: $intensity) {
            ForEach(WorkBlockIntensity.allCases) { value in
              Text(intensityLabel(value)).tag(value)
            }
          }
          .labelsHidden()
          .frame(maxWidth: .infinity)
        }
      }

      Text(intensityExplanation)
        .font(.caption2)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      if let error = coordinator.commandError {
        Text(error)
          .font(.caption)
          .foregroundStyle(.red)
          .accessibilityLabel("Work block error: \(error)")
      }

      Button(startButtonLabel) {
        coordinator.startBlock(
          intention: intention.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty,
          durationSeconds: durationSeconds,
          purpose: purpose,
          intensity: intensity
        )
        plansAnotherSession = false
      }
      .buttonStyle(.borderedProminent)
      .tint(Color.velvtPink)
      .keyboardShortcut(.defaultAction)
      .accessibilityHint("Starts this bounded work block on the local service")
    }
    .padding(16)
  }

  /// The in-app surface for a live drift offer.
  ///
  /// This is the primary path, not a fallback for the notification: it always
  /// renders, whereas an OS notification depends on authorization and is
  /// suppressed by Focus. Every reply is recorded, so silence stays
  /// distinguishable from disagreement.
  ///
  /// Copy comes from Rust verbatim. Swift does not reinterpret the evidence or
  /// offer an action outside the registry.
  private func interventionCard(_ intervention: ActiveIntervention) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(intervention.title)
        .font(.subheadline.bold())

      Text(intervention.body)
        .font(.caption)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      HStack(spacing: 8) {
        Button("Back to work") {
          coordinator.respondToIntervention(.acceptedAction)
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)

        Spacer(minLength: 0)

        Button {
          coordinator.respondToIntervention(.dismissed)
        } label: {
          Image(systemName: "xmark")
        }
        .buttonStyle(.plain)
        .controlSize(.small)
        .foregroundStyle(.secondary)
        .accessibilityLabel("Dismiss this suggestion")
      }

      // Disagreement is evidence against the detector, so each kind of "you
      // were wrong" is a first-class reply rather than a shrug. "I was
      // focused" leads: it is the only reply that says the offer should never
      // have fired, and a false positive Velvt cannot see is one it cannot
      // stop making.
      HStack(spacing: 12) {
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
      .font(.caption)
      .foregroundStyle(.secondary)

      // The one-tap explanation (D7): the sentence is Rust-authored from
      // the stored evidence and rendered verbatim. One sentence, no input
      // field, no reply, no thread — this affordance is the chat gate, not
      // a chat.
      if let explanation = coordinator.explanation {
        Label(explanation.sentence, systemImage: "text.magnifyingglass")
          .font(.caption2)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityLabel("Explanation. \(explanation.sentence)")
      } else {
        Button(DigestFraming.explainLabel) {
          coordinator.requestExplanation()
        }
        .buttonStyle(.plain)
        .controlSize(.small)
        .font(.caption2)
        .foregroundStyle(.secondary)
        .accessibilityHint("Shows one sentence about the evidence behind this nudge")
      }
    }
    .padding(10)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .accessibilityElement(children: .contain)
    .accessibilityLabel("\(intervention.title). \(intervention.body)")
  }

  /// The demotion disclosure (D5; roadmap invariant 4): shown as respect,
  /// never hidden. The body copy is Rust-authored and rendered verbatim;
  /// the detail line shows the exact counts and versioned constants the
  /// deterministic rule evaluated, and the one button is the manual reset.
  private func demotionDisclosureCard(_ state: DemotionState) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Label(DigestFraming.demotionTitle, systemImage: "pause.circle")
        .font(.subheadline.bold())

      if let disclosure = state.disclosure {
        Text(disclosure)
          .font(.caption)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }

      Text(DigestFraming.demotionDetail(state))
        .font(.caption2)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityLabel("Demotion detail. \(DigestFraming.demotionDetail(state))")

      HStack(spacing: 8) {
        Button(DigestFraming.resumeLabel) {
          coordinator.resetDemotion()
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .accessibilityHint("Resumes nudges; the evidence record is unchanged")

        Spacer(minLength: 0)
      }
    }
    .padding(10)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .padding([.horizontal, .top], 16)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(
      "Velvt has gone quiet. \(state.disclosure ?? DigestFraming.demotionDetail(state))")
  }

  /// The weekly receipts digest (D6, D8): one card, not a dashboard.
  /// Recoveries and completions lead, the wrong-intervention count appears
  /// exactly once, and every number is the stored count verbatim.
  private func weeklyDigestCard(_ digest: WeeklyDigest) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Label(DigestFraming.digestTitle, systemImage: "doc.plaintext")
        .font(.subheadline.bold())

      Text(digest.headline)
        .font(.caption)
        .fixedSize(horizontal: false, vertical: true)

      VStack(alignment: .leading, spacing: 3) {
        digestRow(DigestFraming.returnedLabel, digest.recoveries)
        digestRow(DigestFraming.completedLabel, digest.blocksCompleted)
        digestRow(DigestFraming.declaredLabel, digest.blocksDeclared)
        digestRow(DigestFraming.invitationsLabel, digest.invitationsAccepted)
        digestRow(DigestFraming.wrongLabel, digest.wrongInterventions)
        digestRow(DigestFraming.withheldLabel, digest.withheld)
      }

      HStack(spacing: 8) {
        Button(DigestFraming.acknowledgeLabel) {
          coordinator.acknowledgeWeeklyDigest()
        }
        .controlSize(.small)
        .accessibilityHint("Closes this week's receipts")

        Spacer(minLength: 0)
      }
    }
    .padding(10)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .padding([.horizontal, .top], 16)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Weekly receipts. \(digest.headline)")
  }

  private func digestRow(_ label: String, _ count: Int) -> some View {
    HStack {
      Text(label)
        .font(.caption2)
        .foregroundStyle(.secondary)
      Spacer(minLength: 8)
      Text("\(count)")
        .font(.caption2.bold().monospacedDigit())
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(label), \(count)")
  }

  /// The next-morning quiet-hours offer. One tap accepts; declining is a
  /// single calm action the service remembers. Copy comes from Rust
  /// verbatim, and the card never re-asks on its own.
  private func quietHoursOfferCard(_ offer: QuietHoursOffer) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Label("Quiet hours", systemImage: "moon")
        .font(.subheadline.bold())

      Text(offer.body)
        .font(.caption)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      HStack(spacing: 8) {
        Button("Turn on quiet hours") {
          coordinator.respondToQuietHoursOffer(accepted: true)
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .accessibilityHint("Velvt holds its own notifications overnight")

        Button("No thanks") {
          coordinator.respondToQuietHoursOffer(accepted: false)
        }
        .controlSize(.small)
        .accessibilityHint("Keeps everything exactly as it is")

        Spacer(minLength: 0)
      }
    }
    .padding(10)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .padding([.horizontal, .top], 16)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Quiet hours offer. \(offer.body)")
  }

  /// The initiation invitation. At most one per day, extended by the
  /// deterministic Rust policy; Swift renders the body verbatim and can
  /// only accept (a declared block through the existing start command) or
  /// dismiss. Declining is calm and costless.
  private func invitationCard(_ invitation: InitiationInvitation) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Label("Soft start", systemImage: "sunrise")
        .font(.subheadline.bold())

      Text(invitation.body)
        .font(.caption)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      HStack(spacing: 8) {
        Button("Start now") {
          coordinator.acceptInvitation()
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .accessibilityHint("Starts a declared soft-start block on the local service")

        Button("Not now") {
          coordinator.dismissInvitation()
        }
        .controlSize(.small)
        .accessibilityHint("Dismisses this invitation; future invitations only get rarer")

        Spacer(minLength: 0)
      }
    }
    .padding(10)
    .background(Color.primary.opacity(0.06))
    .clipShape(RoundedRectangle(cornerRadius: 8))
    .padding([.horizontal, .top], 16)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Soft start invitation. \(invitation.body)")
  }

  private func activeBlock(_ snapshot: WorkBlockSnapshot) -> some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(alignment: .firstTextBaseline) {
        VStack(alignment: .leading, spacing: 3) {
          Text(snapshot.phase == .paused ? "Work block paused" : "Work block active")
            .font(.headline)
          if let intention = snapshot.intention {
            Text(intention)
              .font(.subheadline)
              .lineLimit(1)
              .truncationMode(.tail)
          }
        }
        Spacer()
        if snapshot.recoveredAfterRestart {
          Text("Recovered")
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
      }

      HStack(spacing: 20) {
        timeColumn(
          "Elapsed", seconds: snapshot.elapsedDurationSeconds, snapshot: snapshot, countsDown: false
        )
        timeColumn(
          "Remaining", seconds: snapshot.remainingDurationSeconds, snapshot: snapshot,
          countsDown: true)
        VStack(alignment: .leading, spacing: 2) {
          Text("Category").font(.caption2).foregroundStyle(.secondary)
          Text(categoryLabel(snapshot.currentCategory))
            .font(.caption.bold())
            .lineLimit(1)
        }
      }

      Text(snapshot.statusLine)
        .font(.caption)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)

      if let intervention = snapshot.activeIntervention {
        interventionCard(intervention)
      }

      if let error = coordinator.commandError {
        Text(error).font(.caption).foregroundStyle(.red)
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
      }
      .buttonStyle(.bordered)
    }
    .padding(16)
  }

  private func resultView(_ snapshot: WorkBlockSnapshot) -> some View {
    VStack(alignment: .leading, spacing: 12) {
      Text(resultTitle(snapshot.phase))
        .font(.headline)
      Text(snapshot.statusLine)
        .font(.caption)
        .foregroundStyle(.secondary)

      if let result = snapshot.result {
        Text(result.observation)
          .font(.body)
          .fixedSize(horizontal: false, vertical: true)

        Text(evidenceLabel(result))
          .font(.caption)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)

        // The one calm DND reconciliation line, authored in Rust: what
        // completed and what was held. Positive framing; never a late nudge.
        if let reconciliation = result.reconciliation {
          Label(reconciliation, systemImage: "moon")
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel("Do Not Disturb summary. \(reconciliation)")
        }

        // Recoveries lead, and are counted rather than rated. A number that
        // can only go up cannot be lost, which is what a streak gets wrong:
        // coming back four times is the achievement, not going unbroken.
        // Switch-aways stay visible as the denominator, never as a score.
        HStack(spacing: 14) {
          VStack(alignment: .leading, spacing: 2) {
            Text("Recoveries").font(.caption2).foregroundStyle(.secondary)
            Text("\(result.recoveryCount)")
              .font(.caption.bold().monospacedDigit())
          }
          resultMetric("Longest stretch", result.longestUninterruptedSeconds)
          resultMetric("Elapsed", result.elapsedDurationSeconds)
          VStack(alignment: .leading, spacing: 2) {
            Text("Switch-aways").font(.caption2).foregroundStyle(.secondary)
            Text("\(result.switchAwayCount)")
              .font(.caption.monospacedDigit())
              .foregroundStyle(.secondary)
          }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
          "Came back \(result.recoveryCount) times after \(result.switchAwayCount) switch-aways"
        )

        Text(coverageLabel(result))
          .font(.caption2)
          .foregroundStyle(.secondary)

        // The gentle re-entry action, offered by Rust only on an invited
        // block that ended early. Label comes from the registry verbatim;
        // it takes the prominent slot and the default shortcut when shown.
        if result.nextAction.actionID == "soft_restart_10" {
          Button(result.nextAction.label) { coordinator.acceptRecovery() }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .accessibilityHint("Starts a ten-minute block on the local service")

          Button("Plan another session") { plansAnotherSession = true }
            .accessibilityHint("Choose the next session's work type and duration")
        } else {
          Button("Plan another session") { plansAnotherSession = true }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .accessibilityHint("Choose the next session's work type and duration")
        }
      }

      if let error = coordinator.commandError {
        Text(error).font(.caption).foregroundStyle(.red)
      }
    }
    .padding(16)
  }

  @ViewBuilder
  private func timeColumn(
    _ title: String,
    seconds: Int,
    snapshot: WorkBlockSnapshot,
    countsDown: Bool
  ) -> some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(title).font(.caption2).foregroundStyle(.secondary)
      if snapshot.phase == .active {
        if countsDown, let endsAt = snapshot.endsAt {
          Text(timerInterval: Date()...max(Date(), endsAt), countsDown: true)
            .font(.caption.bold().monospacedDigit())
        } else {
          let effectiveStart = Date().addingTimeInterval(-TimeInterval(seconds))
          Text(timerInterval: effectiveStart...Date.distantFuture, countsDown: false)
            .font(.caption.bold().monospacedDigit())
        }
      } else {
        Text(durationLabel(seconds))
          .font(.caption.bold().monospacedDigit())
      }
    }
  }

  private func resultMetric(_ title: String, _ seconds: Int) -> some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(title).font(.caption2).foregroundStyle(.secondary)
      Text(durationLabel(seconds)).font(.caption.bold().monospacedDigit())
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

  private func coverageLabel(_ result: WorkBlockResult) -> String {
    "\(result.coverage.rawValue.capitalized) coverage · \(result.confidence.rawValue.capitalized) confidence"
  }

  private func evidenceLabel(_ result: WorkBlockResult) -> String {
    guard let category = result.safeEvidenceCategory else {
      return "Evidence: no supported category was recorded."
    }
    return "Evidence category: \(categoryLabel(category))."
  }

  private func durationLabel(_ seconds: Int) -> String {
    String(format: "%d:%02d", max(0, seconds) / 60, max(0, seconds) % 60)
  }
}

extension String {
  fileprivate var nilIfEmpty: String? { isEmpty ? nil : self }
}
