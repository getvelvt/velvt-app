import Foundation
import ServiceManagement
import os

/// The login-item calls `OpenAtLoginDefault` makes, behind a seam so tests
/// never touch the real Login Items list.
protocol LoginItemRegistering {
    var status: SMAppService.Status { get }
    func register() throws
}

struct MainAppLoginItem: LoginItemRegistering {
    var status: SMAppService.Status { SMAppService.mainApp.status }

    func register() throws {
        try SMAppService.mainApp.register()
    }
}

/// Velvt opens at login. It is a default, not a setting.
///
/// A menu-bar app that has to be reopened by hand after every restart stops
/// at the first one, and nothing tells the person. So Velvt adds itself to
/// Login Items once per Mac, on the first launch from an Applications folder,
/// and never touches the registration again. macOS's own list in System
/// Settings > General > Login Items is the off switch: the record that the
/// default was applied outlives the registration, so a later launch cannot
/// undo someone removing Velvt there. There is no Velvt setting for it and no
/// question about it.
struct OpenAtLoginDefault {
    static let appliedDefaultsKey = "velvt.open_at_login.default_applied"

    enum Outcome: Equatable {
        /// Applied on an earlier launch. The Login Items list decides now.
        case alreadyApplied
        /// Not running from an Applications folder: a mounted DMG, a
        /// translocated download, or a development build. Nothing is
        /// recorded, so the next launch, from Applications, applies it.
        case notInApplications
        /// Already in Login Items, so nothing was added.
        case alreadyRegistered
        case registered
        /// macOS refused. Nothing is recorded, so the next launch tries again.
        case failed
    }

    private static let log = Logger(subsystem: "com.velvt.mac", category: "OpenAtLogin")

    private let defaults: UserDefaults
    private let loginItem: any LoginItemRegistering
    private let bundleURL: URL
    private let applicationsDirectories: [URL]

    init(
        defaults: UserDefaults = .standard,
        loginItem: any LoginItemRegistering = MainAppLoginItem(),
        bundleURL: URL = Bundle.main.bundleURL,
        applicationsDirectories: [URL] = FileManager.default.urls(
            for: .applicationDirectory,
            in: [.localDomainMask, .userDomainMask]
        )
    ) {
        self.defaults = defaults
        self.loginItem = loginItem
        self.bundleURL = bundleURL
        self.applicationsDirectories = applicationsDirectories
    }

    /// Whether macOS will open Velvt at the next login. Onboarding says so
    /// only when it is true.
    var opensAtLogin: Bool {
        loginItem.status == .enabled
    }

    @discardableResult
    func apply() -> Outcome {
        guard !defaults.bool(forKey: Self.appliedDefaultsKey) else { return .alreadyApplied }
        guard isInApplicationsFolder else { return .notInApplications }
        let outcome: Outcome
        switch loginItem.status {
        case .enabled, .requiresApproval:
            outcome = .alreadyRegistered
        default:
            do {
                try loginItem.register()
                outcome = .registered
            } catch {
                Self.log.error(
                    "open_at_login register failed code=\((error as NSError).code, privacy: .public)"
                )
                return .failed
            }
        }
        defaults.set(true, forKey: Self.appliedDefaultsKey)
        Self.log.info("open_at_login default applied outcome=\(String(describing: outcome), privacy: .public)")
        return outcome
    }

    private var isInApplicationsFolder: Bool {
        guard bundleURL.pathExtension == "app" else { return false }
        let folder = Self.canonicalPath(bundleURL.deletingLastPathComponent())
        return applicationsDirectories.contains { Self.canonicalPath($0) == folder }
    }

    private static func canonicalPath(_ url: URL) -> String {
        url.standardizedFileURL.resolvingSymlinksInPath().path
    }
}
