import Foundation

/// Everything Velvt holds about you on this Mac, as a file you own.
///
/// A product whose argument is "your data stays on your device" has to be able
/// to hand it back, and there was no way to get it out at all — no export, no
/// dump, nothing. This builds the document from the same local snapshot the UI
/// draws, so what you export is exactly what you were shown, and it needs no
/// new IPC message: the protocol is frozen at v28 and adding one to ship an
/// export would be the wrong trade.
///
/// It deliberately does not attempt to be the whole database. The raw event
/// buffer is short-lived by design and holds the unabstracted labels the rest
/// of the product exists to keep local; `rawDatabaseNote` points at the file so
/// anyone who wants literally everything can take it, rather than being handed
/// a partial export that implies it is complete.
public struct LocalDataExport: Codable, Equatable, Sendable {
  public struct Activity: Codable, Equatable, Sendable {
    public let label: String
    public let category: String
    public let seconds: Int
    public let percentage: Int
  }

  public struct Day: Codable, Equatable, Sendable {
    public let date: String
    public let observedSeconds: Int
    public let coverage: String
    public let state: String
    public let activities: [Activity]

    private enum CodingKeys: String, CodingKey {
      case date, coverage, state, activities
      case observedSeconds = "observed_seconds"
    }
  }

  public struct Week: Codable, Equatable, Sendable {
    public let headline: String
    public let blocksDeclared: Int
    public let blocksCompleted: Int
    public let recoveries: Int
    public let invitationsAccepted: Int
    public let wrongInterventions: Int
    public let withheld: Int

    private enum CodingKeys: String, CodingKey {
      case headline, recoveries, withheld
      case blocksDeclared = "blocks_declared"
      case blocksCompleted = "blocks_completed"
      case invitationsAccepted = "invitations_accepted"
      case wrongInterventions = "wrong_interventions"
    }
  }

  public let schema: String
  public let exportedAt: Date
  public let days: [Day]
  public let week: Week?
  public let rawDatabaseNote: String

  private enum CodingKeys: String, CodingKey {
    case schema, days, week
    case exportedAt = "exported_at"
    case rawDatabaseNote = "raw_database_note"
  }

  public static let schemaVersion = "velvt.local-export.v1"

  public static let rawDatabaseNote =
    "This file covers the observations Velvt has shown you. The complete local database, including the short-lived raw event buffer, is a single SQLite file at ~/.velvt/velvt-service.sqlite3 which you may copy at any time."

  public init(snapshot: LocalDashboardSnapshot?, digest: WeeklyDigest?, exportedAt: Date) {
    schema = Self.schemaVersion
    self.exportedAt = exportedAt
    rawDatabaseNote = Self.rawDatabaseNote
    days = (snapshot?.dailyActivity ?? []).map { day in
      Day(
        date: day.date,
        observedSeconds: day.activeSeconds,
        coverage: day.coverage.rawValue,
        state: day.state.rawValue,
        activities: day.segments.map { segment in
          Activity(
            label: segment.label,
            category: segment.category,
            seconds: segment.durationSeconds,
            percentage: segment.percentage
          )
        }
      )
    }
    week = digest.map { digest in
      Week(
        headline: digest.headline,
        blocksDeclared: digest.blocksDeclared,
        blocksCompleted: digest.blocksCompleted,
        recoveries: digest.recoveries,
        invitationsAccepted: digest.invitationsAccepted,
        wrongInterventions: digest.wrongInterventions,
        withheld: digest.withheld
      )
    }
  }

  /// Pretty-printed with sorted keys so two exports of the same data are the
  /// same bytes — a file a person can diff, and a test can pin.
  public func encoded() throws -> Data {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    encoder.dateEncodingStrategy = .iso8601
    return try encoder.encode(self)
  }

  /// `velvt-export-2026-08-27.json`. Dated, because a person exporting twice
  /// wants two files, not a silent overwrite.
  public static func suggestedFilename(for date: Date) -> String {
    let formatter = DateFormatter()
    formatter.dateFormat = "yyyy-MM-dd"
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.timeZone = .current
    return "velvt-export-\(formatter.string(from: date)).json"
  }
}
