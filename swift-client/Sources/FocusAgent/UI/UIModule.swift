import SwiftUI

/// UI module - owns menu bar, onboarding, permission-status, and privacy
/// disclosure views. Does NOT capture events, perform abstraction, call cloud
/// APIs, or generate insight text.

/// Accessibility and notification permission state shown to the user.
public struct PermissionStatus: Equatable, Sendable {
    public let accessibilityGranted: Bool
    public let notificationsGranted: Bool

    public init(accessibilityGranted: Bool, notificationsGranted: Bool) {
        self.accessibilityGranted = accessibilityGranted
        self.notificationsGranted = notificationsGranted
    }
}

/// Provides permission state without owning permission prompts.
public protocol PermissionStatusProviding {
    func currentStatus() async -> PermissionStatus
}

public struct MenuBarView: View {
    public init() {}

    public var body: some View {
        fatalError("not implemented")
    }
}

public struct OnboardingView: View {
    public init() {}

    public var body: some View {
        fatalError("not implemented")
    }
}

public struct PermissionStatusView: View {
    public init() {}

    public var body: some View {
        fatalError("not implemented")
    }
}

public struct PrivacyDisclosureView: View {
    public init() {}

    public var body: some View {
        fatalError("not implemented")
    }
}

