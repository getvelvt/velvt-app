import Foundation

/// The parts of Copy Diagnostics that describe the build and the Mac, and the
/// mail draft that Contact support… opens.
///
/// Every value here is a version, a build number or an architecture: nothing
/// drawn from what the person was doing, and no account email or device
/// identifier. The draft holds exactly the text Copy Diagnostics copies, and
/// the person reads it, and can change it, before anything is sent. Velvt
/// sends nothing itself.
enum SupportDiagnostics {
    static let supportAddress = "hi@getvelvt.com"
    static let mailSubject = "Velvt support"
    static let mailPrompt = "What happened?"

    /// `CFBundleVersion`, the build number that orders releases in
    /// docs/RELEASES.md. Two builds can share a marketing version.
    static func buildNumber(infoDictionary: [String: Any]?) -> String {
        guard let value = infoDictionary?["CFBundleVersion"] as? String, !value.isEmpty else {
            return "unknown"
        }
        return value
    }

    static func macOSVersion(_ version: OperatingSystemVersion) -> String {
        "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
    }

    /// The architecture this process runs as. A universal build runs as
    /// x86_64 on Apple silicon only under Rosetta, where the helper and the
    /// app pay translation costs that an Intel Mac does not.
    static func architecture(processIsARM64: Bool, processIsTranslated: Bool) -> String {
        if processIsARM64 { return "arm64" }
        return processIsTranslated ? "x86_64_rosetta" : "x86_64"
    }

    static var currentArchitecture: String {
        #if arch(arm64)
            architecture(processIsARM64: true, processIsTranslated: false)
        #else
            architecture(processIsARM64: false, processIsTranslated: processIsTranslated())
        #endif
    }

    /// A `mailto:` draft to support holding `diagnostics` under a line for the
    /// person's own words.
    ///
    /// The query is encoded by hand, to the unreserved set. `URLQueryItem`
    /// leaves `+` unencoded, which a mail app can read as a space, and its
    /// handling of `=` and `&` has varied across Foundation versions; every
    /// diagnostics line is `key=value`. Encoding by hand gives the same draft
    /// on every macOS.
    static func mailDraftURL(diagnostics: String) -> URL? {
        let body = "\(mailPrompt)\n\n\n\n\(diagnostics)\n"
        var components = URLComponents()
        components.scheme = "mailto"
        components.path = supportAddress
        components.percentEncodedQuery = [
            "subject=\(percentEncoded(mailSubject))",
            "body=\(percentEncoded(body))",
        ].joined(separator: "&")
        return components.url
    }

    private static let unreservedCharacters = CharacterSet(
        charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"
    )

    private static func percentEncoded(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: unreservedCharacters) ?? ""
    }

    private static func processIsTranslated() -> Bool {
        var translated: Int32 = 0
        var size = MemoryLayout<Int32>.size
        guard sysctlbyname("sysctl.proc_translated", &translated, &size, nil, 0) == 0 else {
            return false
        }
        return translated == 1
    }
}
