import Foundation

/// What an application declares about itself in its own `Info.plist`.
///
/// Facts, never conclusions. Nothing here is interpreted on this side of the
/// socket: the strings are reported verbatim and the Rust service decides
/// whether any of them mean anything — most declared categories mean nothing at
/// all (`productivity` alone covers four Velvt categories).
///
/// Reading an application's own `Info.plist` needs no TCC permission: it is
/// world-readable metadata the developer shipped. Every failure — no bundle
/// URL, an unreadable or malformed plist, a missing key, a sandbox denial —
/// produces `.absent`, which must classify exactly as the event did before
/// these fields existed.
public struct DeclaredAppMetadata: Equatable, Sendable {
    /// Nothing was declared, or nothing could be read. The two are deliberately
    /// indistinguishable: neither is evidence of anything.
    public static let absent = DeclaredAppMetadata(declaredAppCategory: nil, documentTypeIDs: [])

    /// The raw `LSApplicationCategoryType` value, trimmed only of surrounding
    /// whitespace.
    public let declaredAppCategory: String?

    /// The `LSItemContentTypes` declared across `CFBundleDocumentTypes`,
    /// flattened, deduplicated and sorted, within the protocol's bounds.
    public let documentTypeIDs: [String]

    public init(declaredAppCategory: String?, documentTypeIDs: [String]) {
        self.declaredAppCategory = declaredAppCategory
        self.documentTypeIDs = DeclaredDocumentTypeBounds.representable(documentTypeIDs)
    }

    public var isAbsent: Bool {
        declaredAppCategory == nil && documentTypeIDs.isEmpty
    }
}

/// Reads the declared metadata for a running application.
public protocol DeclaredAppMetadataReading: AnyObject {
    func metadata(for application: RunningApplication) -> DeclaredAppMetadata
}

/// Reads `Contents/Info.plist` out of an application bundle, once per bundle
/// identifier for the lifetime of the process.
///
/// The cache is the point. `metadata(for:)` sits on the window-activation path,
/// which fires on every switch and every title change, and an application's own
/// shipped plist cannot change while it is running. Failures are cached too: an
/// application whose plist cannot be read must not be re-stat'ed on every event
/// for the rest of the session.
public final class BundleInfoPlistMetadataProvider: DeclaredAppMetadataReading {
    /// Loads a property list from a file URL, or returns `nil` for any failure.
    public typealias PropertyListLoader = (URL) -> [String: Any]?

    /// An `Info.plist` larger than this is treated as unreadable.
    ///
    /// The file belongs to a third party and is read on the activation path;
    /// Xcode's — the largest measured on this machine — is under 200 KB, so a
    /// file orders of magnitude past that is not a plist worth blocking a
    /// window switch on.
    private static let maximumPropertyListBytes = 4 * 1024 * 1024

    private let loadPropertyList: PropertyListLoader
    private let lock = NSLock()
    private var cache: [String: DeclaredAppMetadata] = [:]

    public init(
        loadPropertyList: @escaping PropertyListLoader = {
            BundleInfoPlistMetadataProvider.readPropertyList(at: $0)
        }
    ) {
        self.loadPropertyList = loadPropertyList
    }

    public func metadata(for application: RunningApplication) -> DeclaredAppMetadata {
        // The bundle identifier is the cache key, so an application without one
        // is never cached — and never read either, since there is nothing to
        // key a later lookup on.
        guard let bundleIdentifier = application.bundleIdentifier else {
            return .absent
        }
        if let cached = lock.withLock({ cache[bundleIdentifier] }) {
            return cached
        }
        // Read outside the lock: a slow or hostile filesystem must not block
        // another thread's cache hit. Two threads racing the first read for one
        // application repeat the work and agree on the result, which is cheaper
        // than holding a lock across file I/O.
        let metadata = application.bundleURL.flatMap { read(bundleAt: $0) } ?? .absent
        lock.withLock { cache[bundleIdentifier] = metadata }
        return metadata
    }

    /// The default loader: `Contents/Info.plist` off disk, `nil` on any failure.
    public static func readPropertyList(at url: URL) -> [String: Any]? {
        guard let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize,
            size <= maximumPropertyListBytes
        else {
            return nil
        }
        guard let data = try? Data(contentsOf: url),
            let plist = try? PropertyListSerialization.propertyList(
                from: data, options: [], format: nil)
        else {
            return nil
        }
        return plist as? [String: Any]
    }

    private func read(bundleAt bundleURL: URL) -> DeclaredAppMetadata? {
        let infoPlistURL = bundleURL.appendingPathComponent("Contents/Info.plist")
        guard let plist = loadPropertyList(infoPlistURL) else {
            return nil
        }
        return DeclaredAppMetadata(
            declaredAppCategory: Self.declaredAppCategory(in: plist),
            documentTypeIDs: Self.documentTypeIDs(in: plist)
        )
    }

    private static func declaredAppCategory(in plist: [String: Any]) -> String? {
        guard let declared = plist["LSApplicationCategoryType"] as? String else {
            return nil
        }
        let trimmed = declared.trimmingCharacters(in: .whitespacesAndNewlines)
        // An empty value declares nothing; reporting `""` would be reporting a
        // fact that is not one.
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func documentTypeIDs(in plist: [String: Any]) -> [String] {
        guard let declarations = plist["CFBundleDocumentTypes"] as? [[String: Any]] else {
            return []
        }
        // Every element is type-checked rather than force-cast: this is a third
        // party's file, and a plist that does not match its own documented shape
        // is a plist we know nothing from, not a crash.
        var identifiers: [String] = []
        for declaration in declarations {
            guard let contentTypes = declaration["LSItemContentTypes"] as? [Any] else {
                continue
            }
            identifiers.append(contentsOf: contentTypes.compactMap { $0 as? String })
        }
        // Deduplication, sorting and the protocol's bounds all live in one
        // place, shared with the IPC boundary.
        return DeclaredDocumentTypeBounds.representable(identifiers)
    }
}
