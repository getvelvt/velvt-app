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

/// Loads IPC and application configuration from the process environment.
public struct EnvironmentConfigLoader: ConfigLoading {
    private let environment: [String: String]

    public init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        self.environment = environment
    }

    public func load() throws -> FocusAgentConfig {
        guard let socketPath = environment["VELVT_SOCKET_PATH"], !socketPath.isEmpty else {
            throw ConfigError.missingValue(name: "VELVT_SOCKET_PATH")
        }
        guard
            let protocolValue = environment["VELVT_PROTOCOL_VERSION"],
            let protocolVersion = Int(protocolValue),
            protocolVersion > 0
        else {
            throw ConfigError.invalidValue(name: "VELVT_PROTOCOL_VERSION")
        }
        guard let clientVersion = environment["VELVT_CLIENT_VERSION"], !clientVersion.isEmpty else {
            throw ConfigError.missingValue(name: "VELVT_CLIENT_VERSION")
        }

        return FocusAgentConfig(
            socketPath: socketPath,
            protocolVersion: protocolVersion,
            clientVersion: clientVersion,
            apnsEnvironment: .development
        )
    }
}

public enum ConfigError: Error, Equatable {
    case missingValue(name: String)
    case invalidValue(name: String)
}
