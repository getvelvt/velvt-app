import SwiftUI

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
    private let onEscape: () -> Void

    public init(
        presentation: PermissionPresentationModel,
        coordinator: ConcreteDisplayDataCoordinator,
        onEscape: @escaping () -> Void
    ) {
        self.presentation = presentation
        self.coordinator = coordinator
        self.onEscape = onEscape
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Velvt")
                    .font(.headline)
                    .foregroundStyle(.primary)
                Spacer()
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
