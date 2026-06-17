import SwiftUI

/// Shown when the bundled Rust service cannot be installed or started.
/// Blocks all normal app UI until state transitions to .running.
struct ServiceUnavailableView: View {
    @ObservedObject var serviceManager: ServiceManager

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
                    do {
                        try await serviceManager.ensureInstalled()
                        try await serviceManager.ensureUpToDate()
                        try await serviceManager.start()
                    } catch {
                        serviceManager.state = .failed(error)
                    }
                }
            }
            .buttonStyle(.borderedProminent)
        }
        .padding(32)
        .frame(minWidth: 320)
    }
}
