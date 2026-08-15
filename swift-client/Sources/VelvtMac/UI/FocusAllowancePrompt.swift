import AppKit
import Foundation
import SwiftUI

/// The single Focus-allowance ask (roadmap invariant 8: ride existing habits;
/// integrate with macOS Focus rather than competing with it).
///
/// Every notification Velvt sends is `.active` interruption level, which macOS
/// suppresses under any Focus mode: it lands silently in Notification Centre
/// with no banner and no sound. The person most likely to have Focus on is the
/// person doing deep work — exactly the person a drift offer is for — so
/// without this the offer cannot reach the user it was built for.
///
/// There is no API to allowlist an app in someone's Focus, and there should
/// not be. Velvt never reads or writes the Focus configuration. This opens the
/// user's own settings and the change is entirely theirs.
///
/// Asked once, remembered forever. Declining changes nothing else about the
/// product: the offer still renders in the popover, it simply will not ring
/// while Focus is on.
@MainActor
public final class FocusAllowancePromptModel: ObservableObject {
    public static let askedDefaultsKey = "focusAllowanceAsked"

    @Published public private(set) var hasRequested = false

    private let defaults: UserDefaults
    private let openFocusSettings: @MainActor () -> Void

    public init(
        defaults: UserDefaults = .standard,
        openFocusSettings: @escaping @MainActor () -> Void =
            FocusAllowancePromptModel.openSystemFocusSettings
    ) {
        self.defaults = defaults
        self.openFocusSettings = openFocusSettings
    }

    /// True once the one ask has been made, on this or any earlier launch.
    public var hasBeenAsked: Bool {
        defaults.bool(forKey: Self.askedDefaultsKey)
    }

    /// The affirmative path: open the user's Focus settings so they can add
    /// Velvt themselves. Marked asked first, so a failure to open the panel
    /// cannot produce a prompt that returns on every launch.
    public func allowAndOpenFocusSettings() {
        defaults.set(true, forKey: Self.askedDefaultsKey)
        hasRequested = true
        openFocusSettings()
    }

    /// Declining is one tap and changes nothing else about the product.
    public func skip() {
        defaults.set(true, forKey: Self.askedDefaultsKey)
    }

    public static func openSystemFocusSettings() {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.Focus-Settings.extension")
        else { return }
        NSWorkspace.shared.open(url)
    }
}

/// The ask itself. Copy states the cost plainly and promises restraint, because
/// the permission being requested is the right to interrupt a Focus mode.
public struct FocusAllowanceView: View {
    @ObservedObject private var model: FocusAllowancePromptModel
    private let onContinue: () -> Void

    public init(model: FocusAllowancePromptModel, onContinue: @escaping () -> Void) {
        self.model = model
        self.onContinue = onContinue
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Let Velvt through your Focus")
                .font(.title2.weight(.semibold))

            Text(
                "When a Focus mode is on, macOS silences Velvt along with everything else — "
                    + "so a nudge would arrive after the work block it was meant to protect."
            )
            .foregroundStyle(.secondary)

            Text(
                "You can add Velvt to the apps your Focus allows. We spend that permission "
                    + "like it's expensive: one offer per block, never outside one, and quieter "
                    + "every time you push one away."
            )
            .foregroundStyle(.secondary)

            if model.hasRequested {
                Text(
                    "In System Settings → Focus, pick the Focus you use for work, "
                        + "open Allowed Notifications, and add Velvt."
                )
                .font(.callout)
                .padding(10)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
            }

            HStack {
                Button("Not now") {
                    model.skip()
                    onContinue()
                }
                .buttonStyle(.bordered)

                Spacer()

                if model.hasRequested {
                    Button("Done") { onContinue() }
                        .buttonStyle(.borderedProminent)
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Open Focus Settings") {
                        model.allowAndOpenFocusSettings()
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                }
            }
        }
        .frame(maxWidth: 460, alignment: .leading)
        .padding(28)
    }
}
