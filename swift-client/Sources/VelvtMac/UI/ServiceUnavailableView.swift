import SwiftUI

/// Shown when the bundled Rust service cannot be installed or started.
/// Blocks all normal app UI until state transitions to .running.
struct ServiceUnavailableView: View {
    @ObservedObject var serviceManager: ServiceManager

    /// Re-runs the full launch sequence (install → start → wait for IPC
    /// socket → connect), not just the SMAppService steps — the failure may
    /// have come from the socket/IPC stage rather than from ServiceManager
    /// itself. Falls back to the SMAppService-only retry when no closure is
    /// injected (e.g. existing call sites / previews).
    var onRetry: (() async -> Void)?

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 40))
                .foregroundStyle(.orange)

            Text("Service Unavailable")
                .font(.headline)

            if case .failed(let error) = serviceManager.state {
                Text(error.localizedDescription)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }

            Button("Try Again") {
                Task { @MainActor in
                    if let onRetry {
                        await onRetry()
                    } else {
                        await serviceManager.ensureInstalled()
                        await serviceManager.ensureUpToDate()
                        await serviceManager.start()
                    }
                }
            }
            .buttonStyle(.borderedProminent)
        }
        .padding(32)
        .frame(minWidth: 320)
    }
}
