import Foundation

/// Loads typed client configuration from values baked into Info.plist at
/// build time via xcconfig INFOPLIST_KEY_ settings. No runtime environment
/// reads occur in this path.
public struct BundleConfigLoader: ConfigLoading {
    private let infoDictionary: [String: Any]

    public init(bundle: Bundle = .main) {
        self.infoDictionary = bundle.infoDictionary ?? [:]
    }

    /// Initialiser for tests: supply the dictionary directly without a real Bundle.
    init(infoDictionary: [String: Any]) {
        self.infoDictionary = infoDictionary
    }

    public func load() throws -> FocusAgentConfig {
        let info = infoDictionary

        guard let socketPath = info["VelvtSocketPath"] as? String, !socketPath.isEmpty else {
            throw ConfigError.missingValue(name: "VelvtSocketPath")
        }

        guard
            let versionString = info["VelvtProtocolVersion"] as? String,
            let protocolVersion = Int(versionString),
            protocolVersion > 0
        else {
            throw ConfigError.invalidValue(name: "VelvtProtocolVersion")
        }

        guard let clientVersion = info["VelvtClientVersion"] as? String, !clientVersion.isEmpty else {
            throw ConfigError.missingValue(name: "VelvtClientVersion")
        }

        guard let apnsRaw = info["VelvtAPNSEnv"] as? String,
              let apnsEnvironment = APNSEnvironment(rawValue: apnsRaw)
        else {
            throw ConfigError.invalidValue(name: "VelvtAPNSEnv")
        }

        return FocusAgentConfig(
            socketPath: socketPath,
            protocolVersion: protocolVersion,
            clientVersion: clientVersion,
            apnsEnvironment: apnsEnvironment
        )
    }
}
