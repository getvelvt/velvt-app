import Combine
import Foundation
import XCTest

@testable import VelvtMac

/// The needs-a-category list in Settings → Apps & Sites: teaching an app or a
/// site from it, what is sent back with the key, and the words around it.
@MainActor
final class NeedsACategoryListTests: XCTestCase {
    private let appKey = String(repeating: "a", count: 64)
    private let siteKey = String(repeating: "b", count: 64)
    private let unnamedKey = String(repeating: "c", count: 64)

    // MARK: - Teaching

    func testTeachingAnApplicationTakesItsRowAtOnceAndNamesTheRuleAfterIt() async throws {
        let (sut, client, messages) = shownList()
        try await waitUntil { sut.unclassifiedTriage != nil }
        let before = client.sentMessages.count

        sut.teach(application, category: "FOCUS_WORK")

        // Synchronously, before any answer: the row is the user's own action.
        XCTAssertEqual(sut.unclassifiedTriage?.entries.map(\.id), [site.id, unnamed.id])
        XCTAssertEqual(sut.unclassifiedTriage?.windowDays, 7)
        try await waitUntil { client.sentMessages.count >= before + 3 }
        let sent = Array(client.sentMessages[before...])
        XCTAssertEqual(
            sent.first,
            .setApplicationCategory(.init(appStableID: appKey, category: "FOCUS_WORK", activityName: "Code")))
        // Both re-reads follow the write; they travel on separate tasks, so
        // not in a fixed order between themselves. The list is re-read
        // because it is on screen.
        XCTAssertEqual(
            Set(sent.dropFirst().map { String(describing: $0) }),
            Set(
                [
                    ClientMessage.requestCorrectionHistory(.init(query: nil, offset: 0)),
                    .requestUnclassifiedTriage(.init(lookbackDays: MenuStatusViewModel.triageLookbackDays)),
                ].map { String(describing: $0) }))

        messages.send(.menuStatus(status(acknowledging: "Got it — Code counts as focus work from now on.")))
        try await waitUntil { sut.correctionAcknowledgment != nil }
        XCTAssertEqual(sut.acknowledgmentOrigin, .application)
    }

    /// A site is answered with its key alone. Its display name is its
    /// hostname, which Velvt keeps only until the site is taught; sent back it
    /// would be stored a second time as the rule's name.
    func testTeachingASiteSendsSetSiteCategoryAndNeverItsHostname() async throws {
        let (sut, client, messages) = shownList()
        try await waitUntil { sut.unclassifiedTriage != nil }
        let before = client.sentMessages.count

        sut.teach(site, category: "REFERENCE")

        XCTAssertEqual(sut.unclassifiedTriage?.entries.map(\.id), [application.id, unnamed.id])
        try await waitUntil { client.sentMessages.count >= before + 3 }
        XCTAssertEqual(
            client.sentMessages[before],
            .setSiteCategory(.init(siteStableID: siteKey, category: "REFERENCE", activityName: nil)))
        for message in client.sentMessages {
            let encoded = String(decoding: try IPCMessageCodec.makeEncoder().encode(message), as: UTF8.self)
            XCTAssertFalse(encoded.contains("wiki.example"), "a hostname went back over the socket")
        }

        messages.send(
            .menuStatus(
                status(
                    acknowledging:
                        "Got it — every page of wiki.example, in every browser, counts as reference from now on.")))
        try await waitUntil { sut.correctionAcknowledgment != nil }
        XCTAssertEqual(sut.acknowledgmentOrigin, .application)
    }

    /// "Unnamed application" is the client's wording for a row, not a name.
    /// Protocol 32 sent it back, and every later window of the app was
    /// labelled with it.
    func testAnUnnamedApplicationIsTaughtWithNoName() async throws {
        let (sut, client, _) = shownList()
        try await waitUntil { sut.unclassifiedTriage != nil }
        XCTAssertEqual(QueuedEventPresentation.name(of: unnamed), "Unnamed application")

        sut.teach(unnamed, category: "SYSTEM")

        try await waitUntil {
            client.sentMessages.contains(
                .setApplicationCategory(.init(appStableID: self.unnamedKey, category: "SYSTEM", activityName: nil)))
        }
        XCTAssertFalse(
            client.sentMessages.contains {
                if case .setApplicationCategory(let answer) = $0 { return answer.activityName != nil }
                return false
            })
    }

    /// The row went before the service answered, so a refusal has to bring it
    /// back: the refusal is caught, said, and the list is read again from the
    /// service, whose answer is the authority on what is still unanswered.
    func testARefusedTeachSaysSoAndBringsTheRowBack() async throws {
        let (sut, client, messages) = shownList()
        try await waitUntil { sut.unclassifiedTriage != nil }
        sut.teach(site, category: "REFERENCE")
        XCTAssertFalse(sut.unclassifiedTriage?.entries.contains(site) ?? true)
        try await waitUntil { self.triageRequests(client) == 2 }

        messages.send(
            .errorResponse(
                ErrorResponse(
                    code: "invalid_site_stable_id",
                    message: "Unable to save this site. Try again later.",
                    relatedEventID: nil)))
        try await waitUntil { sut.sendError != nil }
        XCTAssertEqual(sut.sendError, "Unable to save this site. Try again later.")
        try await waitUntil { self.triageRequests(client) == 3 }

        messages.send(.unclassifiedTriage(fullList()))
        try await waitUntil { sut.unclassifiedTriage?.entries.contains(self.site) ?? false }
        XCTAssertEqual(sut.unclassifiedTriage?.entries.map(\.id), fullList().entries.map(\.id))
    }

    func testATeachTheSocketCannotCarryIsReportedForTheKindItWas() async throws {
        let (sut, client, _) = shownList()
        try await waitUntil { sut.unclassifiedTriage != nil }
        client.shouldThrowOnSend = IPCError.notConnected

        sut.teach(site, category: "REFERENCE")
        try await waitUntil { sut.sendError != nil }
        XCTAssertEqual(sut.sendError, "Unable to save this site. Try again later.")

        sut.teach(application, category: "FOCUS_WORK")
        try await waitUntil { sut.sendError == "Unable to save this app. Try again later." }
    }

    /// The list is read again after a write only once something has shown it:
    /// a person who never opened the section never pays for the query.
    func testTheListIsReadAgainAfterATeachOnlyWhileItIsShown() async throws {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)

        sut.teach(site, category: "REFERENCE")
        try await waitUntil { client.sentMessages.count >= 2 }
        try await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(triageRequests(client), 0)
        XCTAssertNil(sut.unclassifiedTriage)
    }

    func testOnlyANameTheServiceWouldAcceptIsSentAsARuleName() {
        XCTAssertEqual(MenuStatusViewModel.ruleName(forApplicationNamed: "  Code  "), "Code")
        XCTAssertNil(MenuStatusViewModel.ruleName(forApplicationNamed: nil))
        XCTAssertNil(MenuStatusViewModel.ruleName(forApplicationNamed: "   "))
        XCTAssertNil(MenuStatusViewModel.ruleName(forApplicationNamed: "Tab\tName"))
        XCTAssertNil(MenuStatusViewModel.ruleName(forApplicationNamed: String(repeating: "x", count: 49)))
        XCTAssertEqual(
            MenuStatusViewModel.ruleName(forApplicationNamed: String(repeating: "x", count: 48)),
            String(repeating: "x", count: 48))
    }

    // MARK: - Words

    func testTheHeadlineCountsSitesAndAppsAndAgreesInNumber() {
        XCTAssertEqual(
            UnclassifiedTriageSection.headline(for: [site, site, application]),
            "2 sites and 1 app you used this week don't have a category yet.")
        XCTAssertEqual(
            UnclassifiedTriageSection.headline(for: [site]),
            "1 site you used this week doesn't have a category yet.")
        XCTAssertEqual(
            UnclassifiedTriageSection.headline(for: [application, unnamed, application]),
            "3 apps you used this week don't have a category yet.")
        XCTAssertEqual(
            UnclassifiedTriageSection.headline(for: [application]),
            "1 app you used this week doesn't have a category yet.")
        XCTAssertEqual(UnclassifiedTriageSection.counts(sites: 1, apps: 2), "1 site and 2 apps")
    }

    /// Settings words this section in Swift, so it is held to the contract that
    /// quotes it.
    func testTheSectionCopyIsTheContractsCopy() throws {
        // Line breaks in the Markdown are not breaks in the sentence.
        let contract = try String(
            contentsOf: repositoryRoot.appendingPathComponent("docs/classification-v2-contract.md"),
            encoding: .utf8
        )
        .split(whereSeparator: \.isWhitespace)
        .joined(separator: " ")
        XCTAssertTrue(contract.contains(UnclassifiedTriageSection.invitationCopy))
        XCTAssertTrue(contract.contains("Settings → \"\(SettingsSubmenu.teachApps.title)\""))
        XCTAssertTrue(contract.contains("\"Apps and sites Velvt couldn't categorize\""))
        XCTAssertTrue(contract.contains("{counts} you used this week don't have a category yet."))
        XCTAssertEqual(SettingsSubmenu.teachApps.title, "Apps & Sites")
    }

    func testASiteRuleIsSaidAsOneAndHasNoNameUntilOneIsTyped() {
        XCTAssertEqual(CorrectionScopePresentation.scopeLabel(for: .site), "THIS SITE")
        XCTAssertEqual(
            CorrectionScopePresentation.reach(of: .site),
            "Applies to every page of this site, in every browser.")
        XCTAssertEqual(CorrectionScopePresentation.removalLabel(for: .site), "Remove")
        XCTAssertEqual(CorrectionScopePresentation.scopeLabel(for: .app), "THIS APP")
        XCTAssertEqual(CorrectionScopePresentation.removalLabel(for: .window), "Undo")

        let unnamedRule = ClassificationCorrectionSummary(
            stableID: siteKey, label: "reference:site", localLabel: nil, category: "REFERENCE",
            updatedAt: Date(), scope: .site)
        XCTAssertEqual(QueuedEventPresentation.activity(unnamedRule), "Unnamed site")
        XCTAssertEqual(CorrectionScopePresentation.editableName(for: unnamedRule), "")

        let namedRule = ClassificationCorrectionSummary(
            stableID: siteKey, label: "reference:site", localLabel: "Team wiki", category: "REFERENCE",
            updatedAt: Date(), scope: .site)
        XCTAssertEqual(QueuedEventPresentation.activity(namedRule), "Team wiki")
        XCTAssertEqual(CorrectionScopePresentation.editableName(for: namedRule), "Team wiki")
    }

    // MARK: - Fixtures

    private var application: UnclassifiedTriageEntry {
        UnclassifiedTriageEntry(
            kind: .application, stableID: appKey, displayName: "Code", secondsObserved: 3_600, eventCount: 9)
    }

    private var site: UnclassifiedTriageEntry {
        UnclassifiedTriageEntry(
            kind: .site, stableID: siteKey, displayName: "wiki.example", secondsObserved: 1_800, eventCount: 6)
    }

    private var unnamed: UnclassifiedTriageEntry {
        UnclassifiedTriageEntry(
            kind: .application, stableID: unnamedKey, displayName: nil, secondsObserved: 600, eventCount: 2)
    }

    private func fullList() -> UnclassifiedTriage {
        UnclassifiedTriage(entries: [application, site, unnamed], windowDays: 7)
    }

    /// A view model whose list is on screen and has been answered.
    private func shownList() -> (MenuStatusViewModel, FakeIPCClient, PassthroughSubject<ServerMessage, Never>) {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let sut = MenuStatusViewModel(ipcClient: client, messages: messages)
        sut.refreshUnclassifiedTriage()
        messages.send(.unclassifiedTriage(fullList()))
        return (sut, client, messages)
    }

    private func status(acknowledging acknowledgment: String) -> MenuStatus {
        MenuStatus(
            deviceID: nil, cloudReady: false, uploadStatus: "network_unavailable", lastUploadErrorCode: nil,
            nextUploadAttemptAt: nil, pendingUploadBatchCount: 0, failedUploadBatchCount: 0,
            rejectedUploadBatchCount: 0, queuedEventCount: 0, queuedEvents: [],
            correctionAcknowledgment: acknowledgment)
    }

    private func triageRequests(_ client: FakeIPCClient) -> Int {
        client.sentMessages.filter {
            if case .requestUnclassifiedTriage = $0 { return true }
            return false
        }.count
    }

    private var repositoryRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private func waitUntil(
        timeout: Duration = .seconds(2),
        condition: @escaping @MainActor () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !condition() {
            if clock.now >= deadline {
                XCTFail("condition timed out")
                return
            }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
    }
}
