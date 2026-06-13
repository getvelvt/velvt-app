import AppKit
import SwiftUI

/// App module - owns application lifecycle and menu bar setup.
/// Does NOT own event capture, IPC processing, abstraction, cloud calls, or
/// insight generation.

/// Coordinates startup and shutdown of the application.
public protocol AppLifecycleManaging: AnyObject {
    /// Starts application-owned services.
    func start() async throws

    /// Stops application-owned services.
    func stop() async
}

/// Creates and manages the menu bar status item.
public protocol StatusItemManaging: AnyObject {
    /// Installs the status item in the system menu bar.
    func install()

    /// Removes the status item from the system menu bar.
    func remove()
}

/// AppKit delegate used by the SwiftUI application entry point.
public final class AppDelegate: NSObject, NSApplicationDelegate {
    public func applicationDidFinishLaunching(_ notification: Notification) {
        fatalError("not implemented")
    }

    public func applicationWillTerminate(_ notification: Notification) {
        fatalError("not implemented")
    }
}

/// Concrete placeholder for menu bar status-item ownership.
public final class StatusItemController: StatusItemManaging {
    public init() {}

    public func install() {
        fatalError("not implemented")
    }

    public func remove() {
        fatalError("not implemented")
    }
}

/// FocusAgent executable entry point.
@main
public struct FocusAgentApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    public init() {}

    public var body: some Scene {
        fatalError("not implemented")
    }
}

