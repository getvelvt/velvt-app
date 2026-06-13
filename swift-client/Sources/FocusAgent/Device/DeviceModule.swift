import Foundation

/// Device module - owns local device identity and APNs token state.
/// Does NOT call cloud APIs directly, capture activity, or store auth tokens.

/// Non-secret local device identity.
public struct DeviceIdentity: Equatable, Sendable {
    public let deviceID: UUID

    public init(deviceID: UUID) {
        self.deviceID = deviceID
    }
}

/// Device registration state surfaced to the app.
public enum DeviceState: Equatable, Sendable {
    case unregistered
    case pending
    case registered
    case revoked
}

/// Stores the APNs token locally.
public protocol APNSTokenStoring: AnyObject {
    func load() throws -> Data?
    func save(_ token: Data) throws
    func clear() throws
}

/// Coordinates local device state for relay through Rust.
public protocol DeviceManaging: AnyObject {
    var state: DeviceState { get }
    func identity() throws -> DeviceIdentity
    func updateAPNSToken(_ token: Data) throws
}

public enum DeviceError: Error, Equatable {
    case identityUnavailable
    case tokenStorageFailed(code: Int)
}

