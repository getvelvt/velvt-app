import SwiftUI

/// UI module - owns menu bar, onboarding, permission-status, and privacy
/// disclosure views. Does NOT capture events, perform abstraction, call cloud
/// APIs, or generate insight text.

public struct PermissionStatusView: View {
    public init() {}

    public var body: some View {
        Text("Permission status")
    }
}

public struct PrivacyDisclosureView: View {
    public init() {}

    public var body: some View {
        Text("Raw activity stays on this Mac.")
    }
}
