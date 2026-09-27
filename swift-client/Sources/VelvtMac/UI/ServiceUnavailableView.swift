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
        VStack(spacing: VelvtMetrics.cardPadding) {
            // A state marker, not an alarm: signal is the brand's emphasis hue,
            // and it leaves crimson to the one action on the surface. Nothing
            // here is coloured as failure.
            Image(systemName: "exclamationmark.triangle")
                .font(VelvtType.title(40))
                .foregroundStyle(VelvtInk.labelOnInk)

            Text("Service Unavailable")
                .velvtHeading()

            if case .failed(let error) = serviceManager.state {
                Text(error.localizedDescription)
                    .velvtBody(12)
                    .multilineTextAlignment(.center)
            }

            Button("Try Again") {
                Task { @MainActor in
                    if let onRetry {
                        await onRetry()
                    } else {
                        await serviceManager.ensureInstalled()
                        await serviceManager.start()
                    }
                }
            }
            .buttonStyle(VelvtPrimaryButtonStyle())
        }
        .padding(32)
        .frame(minWidth: 320)
        .background(VelvtSurface.ground)
    }
}
