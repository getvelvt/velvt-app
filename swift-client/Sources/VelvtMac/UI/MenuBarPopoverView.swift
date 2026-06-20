import Combine
import SwiftUI

@MainActor
public final class ServiceConnectionStatusModel: ObservableObject {
    @Published public private(set) var status: ConnectionStatus = .disconnected

    private var cancellable: AnyCancellable?

    public init(connectionStatus: AnyPublisher<ConnectionStatus, Never>) {
        cancellable = connectionStatus
            .receive(on: RunLoop.main)
            .sink { [weak self] in
                self?.status = $0
            }
    }
}

struct ServiceConnectionStatusLabel: View {
    let status: ConnectionStatus

    var body: some View {
        Label(text, systemImage: symbolName)
            .font(.caption2)
            .foregroundStyle(color)
    }

    private var text: String {
        switch status {
        case .connected:
            return "Service connected"
        case .connecting, .handshaking:
            return "Starting service"
        case .reconnecting:
            return "Reconnecting service"
        case .disconnected:
            return "Service unavailable"
        }
    }

    private var symbolName: String {
        switch status {
        case .connected:
            return "checkmark.circle.fill"
        case .connecting, .handshaking, .reconnecting:
            return "arrow.triangle.2.circlepath"
        case .disconnected:
            return "wifi.slash"
        }
    }

    private var color: Color {
        switch status {
        case .connected:
            return .green
        case .connecting, .handshaking, .reconnecting:
            return .orange
        case .disconnected:
            return .secondary
        }
    }
}

// MARK: - MenuBarPopoverView

/// Root content hosted by the `NSPopover` owned by `MenuBarController`.
///
/// Embeds the S6 display content (`InsightCardView` and `HistoryListView`,
/// via `VelvtPopoverContentView`) and the accessibility/notification status
/// banners previously shown in the menu bar window. Tab order follows
/// declaration order in `VelvtPopoverContentView` — insight card, then each
/// history row — since every focusable element there opts in via
/// `.focusable()`. Escape closes the popover via `onEscape`.
public struct MenuBarPopoverView: View {
    @ObservedObject private var presentation: PermissionPresentationModel
    @ObservedObject private var coordinator: ConcreteDisplayDataCoordinator
    @ObservedObject private var serviceConnectionStatus: ServiceConnectionStatusModel
    private let onEscape: () -> Void

    public init(
        presentation: PermissionPresentationModel,
        coordinator: ConcreteDisplayDataCoordinator,
        serviceConnectionStatus: ServiceConnectionStatusModel,
        onEscape: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.coordinator = coordinator
        self.serviceConnectionStatus = serviceConnectionStatus
        self.onEscape = onEscape
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Velvt")
                    .font(.headline)
                    .foregroundStyle(.primary)
                Spacer()
                ServiceConnectionStatusLabel(status: serviceConnectionStatus.status)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)

            Divider().opacity(0.2)

            if presentation.showsAccessibilityRecovery {
                PermissionRecoveryView()
                    .padding(16)
            } else {
                VelvtPopoverContentView(coordinator: coordinator)
            }

            if presentation.statuses[.notifications] == .denied {
                Divider().opacity(0.15)
                Text("Notifications are off. Daily insights remain available here.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
            }
        }
        .frame(width: 280)
        .preferredColorScheme(.dark)
        .onExitCommand(perform: onEscape)
    }
}
