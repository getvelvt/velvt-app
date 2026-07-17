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

  public init(coordinator: WorkBlockCoordinator) {
    self.coordinator = coordinator
  }

  public var body: some View {
    Group {
      if let snapshot = coordinator.snapshot {
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
  }

  private var startForm: some View {
    VStack(alignment: .leading, spacing: 12) {
      Text("Protect one work block")
        .font(.headline)

      TextField("Intention (optional)", text: $intention)
        .textFieldStyle(.roundedBorder)
        .onChange(of: intention) { value in
          intention = String(value.prefix(120)).replacingOccurrences(of: "\n", with: " ")
        }
        .accessibilityLabel("Optional local intention")
        .accessibilityHint("Stored only on this Mac for a short time")

      Picker("Duration", selection: $durationChoice) {
        Text("25 min").tag(WorkBlockDurationChoice.twentyFive)
        Text("50 min").tag(WorkBlockDurationChoice.fifty)
        Text("Custom").tag(WorkBlockDurationChoice.custom)
      }
      .pickerStyle(.segmented)

      if durationChoice == .custom {
        Stepper("\(customMinutes) minutes", value: $customMinutes, in: 5...180, step: 5)
          .font(.caption)
          .accessibilityLabel("Custom duration")
          .accessibilityValue("\(customMinutes) minutes")
      }

      HStack(spacing: 10) {
        Picker("Purpose", selection: $purpose) {
          Text("No purpose").tag(nil as WorkBlockPurpose?)
          ForEach(WorkBlockPurpose.allCases) { value in
            Text(purposeLabel(value)).tag(value as WorkBlockPurpose?)
          }
        }
        Picker("Intensity", selection: $intensity) {
          ForEach(WorkBlockIntensity.allCases) { value in
            Text(intensityLabel(value)).tag(value)
          }
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

      Button("Start") {
        coordinator.startBlock(
          intention: intention.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty,
          durationSeconds: durationSeconds,
          purpose: purpose,
          intensity: intensity
        )
      }
      .buttonStyle(.borderedProminent)
      .keyboardShortcut(.defaultAction)
      .accessibilityHint("Starts this bounded work block on the local service")
    }
    .padding(16)
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

        HStack(spacing: 14) {
          resultMetric("Elapsed", result.elapsedDurationSeconds)
          resultMetric("Longest stretch", result.longestUninterruptedSeconds)
          VStack(alignment: .leading, spacing: 2) {
            Text("Switch-aways / returns").font(.caption2).foregroundStyle(.secondary)
            Text("\(result.switchAwayCount) / \(result.recoveryCount)")
              .font(.caption.bold().monospacedDigit())
          }
        }

        Text(coverageLabel(result))
          .font(.caption2)
          .foregroundStyle(.secondary)

        Button(result.nextAction.label) { coordinator.acceptRecovery() }
          .buttonStyle(.borderedProminent)
          .keyboardShortcut(.defaultAction)
          .accessibilityHint("Starts one ten-minute local recovery block")
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

  private func durationLabel(_ seconds: Int) -> String {
    String(format: "%d:%02d", max(0, seconds) / 60, max(0, seconds) % 60)
  }
}

extension String {
  fileprivate var nilIfEmpty: String? { isEmpty ? nil : self }
}
