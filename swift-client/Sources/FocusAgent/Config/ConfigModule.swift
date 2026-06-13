import Foundation

/// Config module - owns typed local build and runtime configuration.
/// Does NOT hardcode protocol versions or socket paths, load auth tokens, or
/// own service lifecycle.

/// APNs environment selected by build configuration.
public enum APNSEnvironment: String, Equatable, Sendable {
    case development
    case production
}

/// Typed local client configuration.
public struct FocusAgentConfig: Equatable, Sendable {
    public let socketPath: String
    public let protocolVersion: Int
    public let clientVersion: String
    public let apnsEnvironment: APNSEnvironment

    public init(
        socketPath: String,
        protocolVersion: Int,
        clientVersion: String,
        apnsEnvironment: APNSEnvironment
    ) {
        self.socketPath = socketPath
        self.protocolVersion = protocolVersion
        self.clientVersion = clientVersion
        self.apnsEnvironment = apnsEnvironment
    }
}

/// Loads typed client configuration from build resources.
public protocol ConfigLoading {
    func load() throws -> FocusAgentConfig
}

public enum ConfigError: Error, Equatable {
    case missingValue(name: String)
    case invalidValue(name: String)
}

