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
    private var ipcClient: (any IPCClientProtocol)?

    public func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            let config = try EnvironmentConfigLoader().load()
            let client: any IPCClientProtocol = UnixSocketIPCClient(
                socketPath: config.socketPath,
                protocolVersion: config.protocolVersion,
                clientVersion: config.clientVersion
            )
            ipcClient = client
            Task.detached {
                do {
                    try await client.connect()
                } catch let IPCError.versionMismatch(expected, got) {
                    await MainActor.run {
                        let alert = NSAlert()
                        alert.alertStyle = .critical
                        alert.messageText = "Velvt update required"
                        alert.informativeText = "IPC protocol version \(got) is incompatible with required version \(expected)."
                        alert.runModal()
                    }
                } catch {
                    // The IPC client owns retry behavior for transport failures.
                }
            }
        } catch {
            ipcClient = nil
        }
    }

    public func applicationWillTerminate(_ notification: Notification) {
        ipcClient?.disconnect()
    }
}

/// Concrete placeholder for menu bar status-item ownership.
public final class StatusItemController: StatusItemManaging {
    public init() {}

    public func install() {
        // Status-item behavior is outside the S1 IPC scaffold scope.
    }

    public func remove() {
        // Status-item behavior is outside the S1 IPC scaffold scope.
    }
}

/// FocusAgent executable entry point.
@main
public struct VelvtMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    public init() {}

    public var body: some Scene {
        WindowGroup {
            Text("Velvt")
                .frame(minWidth: 320, minHeight: 180)
        }
    }
}
