import AppKit
import Combine
import SwiftUI
import XCTest

@testable import VelvtMac

/// The question after a block (attention plan PR-4; proto v38): what the
/// app draws for it, where, and what each tap sends.
///
/// Rust decides everything the question depends on and writes every word
/// (`rust-service/src/self_rating/`, PR-3). These assert the Swift half: it
/// is never drawn during a block, the block's result stays hidden while it
/// is up for the first time, each tap is reported as what it was, and no
/// word on the surface is Swift's.
@MainActor
final class QuestionAfterABlockTests: XCTestCase {
    private let blockID = UUID(uuidString: "7f3c2a10-5b6e-4c1d-9a8f-0e2d4c6b8a03")!
    private let endedAt = Date(timeIntervalSince1970: 1_800_000_000)
    private var deadline: Date { endedAt.addingTimeInterval(7_200) }

    // MARK: When it is drawn

    func testTheQuestionIsUpAboveTheResultAndTheResultIsHeldBack() async throws {
        let (coordinator, _, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }

        XCTAssertEqual(coordinator.questionAfterABlock, .question(prompt(.question), viaLink: false))
        XCTAssertTrue(coordinator.withholdsBlockSummary)
        XCTAssertEqual(
            QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.ask]),
            .question(prompt(.question), viaLink: false))
        XCTAssertNil(QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.link]))
        XCTAssertNil(
            QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.acknowledgement]))
    }

    /// Rust never puts the question on a live block's snapshot. If one
    /// arrived anyway, nothing is drawn and nothing is held back.
    func testNothingIsDrawnDuringABlockWhateverTheSnapshotCarries() async throws {
        for phase in [WorkBlockPhase.active, .paused, .idle] {
            let (coordinator, client, messages) = started()
            messages.send(
                .workBlockState(
                    ended(phase: phase, prompt: prompt(.question), optIn: optIn())))
            try await waitUntil { coordinator.snapshot?.phase == phase }

            XCTAssertNil(coordinator.questionAfterABlock, "\(phase)")
            XCTAssertFalse(coordinator.withholdsBlockSummary, "\(phase)")
            coordinator.reportRatingShown()
            coordinator.answerRating(.most)
            coordinator.declineRatingForNow()
            coordinator.respondToRatingInvitation(turnOn: true)
            try await Task.sleep(for: .milliseconds(50))
            XCTAssertEqual(client.sentMessages, [], "\(phase)")
        }
    }

    func testAQuestionAboutAnotherBlockOrPastItsDeadlineIsNotDrawn() async throws {
        let other = UUID(uuidString: "11111111-2222-4333-8444-555555555555")!
        let (coordinator, _, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question, block: other))))
        try await waitUntil { coordinator.snapshot != nil }
        XCTAssertNil(coordinator.questionAfterABlock)

        let (late, _, lateMessages) = started(now: deadline)
        lateMessages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { late.snapshot != nil }
        XCTAssertNil(late.questionAfterABlock, "an answer at the deadline would be refused")
        XCTAssertFalse(late.withholdsBlockSummary)
    }

    // MARK: What each tap sends

    /// A sighting is a fact, and only a surface someone can see reports one.
    func testTheSightingIsReportedOnlyFromASurfaceOnScreen() async throws {
        let (coordinator, client, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }
        let shown = ClientMessage.workBlockRatingShown(.init(blockID: blockID))

        QuestionAfterABlockView(coordinator: coordinator, parts: [.ask], surfaceIsOnScreen: false)
            .reportSightingIfOnScreen()
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertFalse(client.sentMessages.contains(shown))

        QuestionAfterABlockView(coordinator: coordinator, parts: [.ask], surfaceIsOnScreen: true)
            .reportSightingIfOnScreen()
        try await waitUntil { client.sentMessages.contains(shown) }
    }

    /// One tap is one answer. "Noted." appears only once the service's reply
    /// no longer carries the question, and the result comes back with it.
    func testAnAnswerIsSentOnceAndAcknowledgedFromTheReply() async throws {
        let (coordinator, client, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }

        coordinator.answerRating(.lessThanHalf)
        coordinator.answerRating(.all)
        XCTAssertTrue(coordinator.isSendingRatingAnswer)
        let answer = ClientMessage.rateWorkBlock(
            .init(blockID: blockID, answer: .lessThanHalf, viaLink: false))
        try await waitUntil { client.sentMessages.contains(answer) }
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(client.sentMessages.filter(\.isRatingAnswer), [answer])
        XCTAssertEqual(coordinator.questionAfterABlock, .question(prompt(.question), viaLink: false))

        messages.send(.workBlockState(ended(prompt: nil)))
        try await waitUntil { coordinator.questionAfterABlock == .acknowledged("Noted.") }
        XCTAssertFalse(coordinator.withholdsBlockSummary)
        XCTAssertFalse(coordinator.isSendingRatingAnswer)
        XCTAssertNil(QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.ask]))
        XCTAssertEqual(
            QuestionAfterABlockView.presented(
                coordinator.questionAfterABlock, parts: [.ask, .acknowledgement]),
            .acknowledged("Noted."))
    }

    /// An answer the service never received is not acknowledged.
    func testAnAnswerThatFailsToSendIsNotAcknowledged() async throws {
        let (coordinator, client, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }
        client.shouldThrowOnSend = IPCError.notConnected

        coordinator.answerRating(.half)
        try await waitUntil { !coordinator.isSendingRatingAnswer }

        XCTAssertNotNil(coordinator.commandError)
        XCTAssertEqual(coordinator.questionAfterABlock, .question(prompt(.question), viaLink: false))
    }

    /// A reply that still carries the question means the answer was not
    /// recorded: nothing is acknowledged, and the answers are offered again.
    func testAReplyThatStillAsksOffersTheAnswersAgain() async throws {
        let (coordinator, client, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }

        coordinator.answerRating(.most)
        XCTAssertTrue(coordinator.isSendingRatingAnswer)
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { !coordinator.isSendingRatingAnswer }
        XCTAssertEqual(coordinator.questionAfterABlock, .question(prompt(.question), viaLink: false))

        coordinator.answerRating(.half)
        try await waitUntil {
            client.sentMessages.contains(
                .rateWorkBlock(.init(blockID: self.blockID, answer: .half, viaLink: false)))
        }
    }

    /// Not now is reported once. The service's reply shows the result with
    /// the link under it; the link reopens the question with the result still
    /// on screen, and an answer from there says so.
    func testNotNowThenTheLinkThenAnAnswerViaTheLink() async throws {
        let (coordinator, client, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }

        coordinator.declineRatingForNow()
        let notNow = ClientMessage.workBlockRatingNotNow(.init(blockID: blockID))
        try await waitUntil { client.sentMessages.contains(notNow) }

        let link = prompt(.link)
        messages.send(.workBlockState(ended(prompt: link)))
        try await waitUntil { coordinator.questionAfterABlock == .link(link) }
        XCTAssertFalse(coordinator.withholdsBlockSummary, "the result shows after Not now")
        XCTAssertEqual(
            QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.link]),
            .link(prompt(.link)))
        XCTAssertNil(QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.ask]))

        coordinator.openRatingLink()
        XCTAssertEqual(coordinator.questionAfterABlock, .question(prompt(.link), viaLink: true))
        XCTAssertFalse(coordinator.withholdsBlockSummary, "the result stays once it has been seen")

        // Not now on the reopened question closes it and reports nothing new.
        coordinator.declineRatingForNow()
        XCTAssertEqual(coordinator.questionAfterABlock, .link(prompt(.link)))
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertEqual(client.sentMessages.filter { $0 == notNow }.count, 1)

        coordinator.openRatingLink()
        coordinator.answerRating(.most)
        try await waitUntil {
            client.sentMessages.contains(
                .rateWorkBlock(.init(blockID: self.blockID, answer: .most, viaLink: true)))
        }
    }

    /// The reopened question belongs to its block and its link: a new
    /// snapshot that drops the link closes it.
    func testTheReopenedQuestionClosesWithItsLink() async throws {
        let (coordinator, _, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.link))))
        try await waitUntil { coordinator.snapshot != nil }
        coordinator.openRatingLink()
        XCTAssertEqual(coordinator.questionAfterABlock, .question(prompt(.link), viaLink: true))

        messages.send(.workBlockState(ended(prompt: nil)))
        try await waitUntil { coordinator.snapshot?.ratingPrompt == nil }
        messages.send(.workBlockState(ended(prompt: prompt(.link))))
        try await waitUntil { coordinator.snapshot?.ratingPrompt != nil }
        XCTAssertEqual(coordinator.questionAfterABlock, .link(prompt(.link)))
    }

    // MARK: The one-time invitation and the Settings toggle

    func testTheInvitationReportsWhichButtonAndRereadsTheSetting() async throws {
        let (coordinator, client, messages) = started()
        messages.send(.workBlockState(ended(prompt: nil, optIn: optIn())))
        try await waitUntil { coordinator.snapshot != nil }
        XCTAssertEqual(coordinator.questionAfterABlock, .invitation(optIn()))
        XCTAssertFalse(coordinator.withholdsBlockSummary, "the invitation asks nothing about the block")
        XCTAssertEqual(
            QuestionAfterABlockView.presented(coordinator.questionAfterABlock, parts: [.ask]),
            .invitation(optIn()))

        coordinator.respondToRatingInvitation(turnOn: false)
        try await waitUntil {
            client.sentMessages.suffix(2) == [
                .respondRatingOptIn(.init(turnOn: false)), .requestRatingPromptSettings,
            ]
        }
        coordinator.respondToRatingInvitation(turnOn: true)
        try await waitUntil {
            client.sentMessages.suffix(2) == [
                .respondRatingOptIn(.init(turnOn: true)), .requestRatingPromptSettings,
            ]
        }
    }

    /// Off by default, and not offered at all until Rust says it is
    /// available. The toggle renders what the service reports.
    func testTheSettingsToggleRendersTheServicesSetting() async throws {
        let (coordinator, client, messages) = started()
        let toggle = RatingPromptSettingsToggle(coordinator: coordinator)
        XCTAssertNil(toggle.presentedSettings, "nothing is assumed before the service reports")

        messages.send(.ratingPromptSettings(settings(available: false, enabled: false)))
        try await waitUntil { coordinator.ratingPromptSettings != nil }
        XCTAssertNil(toggle.presentedSettings, "no switch inside the first 14 days")

        messages.send(.ratingPromptSettings(settings(available: true, enabled: false)))
        try await waitUntil { coordinator.ratingPromptSettings?.available == true }
        XCTAssertEqual(toggle.presentedSettings, settings(available: true, enabled: false))

        // On a live block the toggle sends the setting alone.
        messages.send(.workBlockState(ended(phase: .active, prompt: nil)))
        try await waitUntil { coordinator.snapshot?.phase == .active }
        coordinator.setRatingPromptEnabled(true)
        try await waitUntil {
            client.sentMessages.last == .setRatingPromptEnabled(.init(enabled: true))
        }

        // After a block it asks for the block's state again, so a question the
        // change withdrew leaves the result view.
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot?.phase == .completed }
        coordinator.setRatingPromptEnabled(false)
        try await waitUntil {
            client.sentMessages.suffix(2) == [
                .setRatingPromptEnabled(.init(enabled: false)), .requestWorkBlockState,
            ]
        }
    }

    func testConnectingReadsTheSetting() async throws {
        let client = FakeIPCClient()
        let coordinator = WorkBlockCoordinator(ipcClient: client)
        coordinator.start(
            messages: PassthroughSubject<ServerMessage, Never>(),
            connectionStatus: client.connectionStatus)
        client.setConnectionStatus(.connected)
        try await waitUntil { client.sentMessages.contains(.requestRatingPromptSettings) }
    }

    // MARK: State that must not outlive its block

    func testTheAcknowledgementLeavesWithItsBlockAndWithClear() async throws {
        let (coordinator, _, messages) = started()
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot != nil }
        coordinator.answerRating(.all)
        messages.send(.workBlockState(ended(prompt: nil)))
        try await waitUntil { coordinator.questionAfterABlock == .acknowledged("Noted.") }

        coordinator.clearLocalData()
        XCTAssertNil(coordinator.questionAfterABlock)

        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { coordinator.snapshot?.ratingPrompt != nil }
        coordinator.answerRating(.all)
        messages.send(.workBlockState(ended(prompt: nil)))
        try await waitUntil { coordinator.questionAfterABlock == .acknowledged("Noted.") }
        messages.send(.workBlockState(ended(phase: .active, block: UUID(), prompt: nil)))
        try await waitUntil { coordinator.snapshot?.phase == .active }
        XCTAssertNil(coordinator.questionAfterABlock)
    }

    /// The service publishes on commands, never on a timer, so the question's
    /// deadline is the one moment Swift asks again on its own. Once, and not
    /// after a snapshot without the question.
    func testTheDeadlineAsksForTheBlocksStateOnce() async throws {
        let client = FakeIPCClient()
        let sleeps = DelayRecorder()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let now = endedAt.addingTimeInterval(600)
        let coordinator = WorkBlockCoordinator(
            ipcClient: client, now: { now }, sleep: { await sleeps.record($0) })
        coordinator.start(messages: messages, connectionStatus: Empty<ConnectionStatus, Never>())

        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        messages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { client.sentMessages.contains(.requestWorkBlockState) }
        try await Task.sleep(for: .milliseconds(50))
        let delays = await sleeps.delays
        XCTAssertEqual(delays, [6_600])
        XCTAssertEqual(client.sentMessages.filter { $0 == .requestWorkBlockState }.count, 1)

        let gated = FakeIPCClient()
        let gate = Gate()
        let gatedCoordinator = WorkBlockCoordinator(
            ipcClient: gated, now: { now }, sleep: { _ in await gate.wait() })
        let gatedMessages = PassthroughSubject<ServerMessage, Never>()
        gatedCoordinator.start(
            messages: gatedMessages, connectionStatus: Empty<ConnectionStatus, Never>())
        gatedMessages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntilAsync { await gate.isWaiting }
        gatedMessages.send(.workBlockState(ended(prompt: nil)))
        try await waitUntil { gatedCoordinator.snapshot?.ratingPrompt == nil }
        await gate.open()
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertFalse(gated.sentMessages.contains(.requestWorkBlockState))
    }

    // MARK: Where it is drawn, and in whose words

    /// Composition is not observable from a unit test, so these are asserted
    /// against the source, as the proactive cards' move was.
    func testBothResultSurfacesDrawTheQuestionAndHoldBackTheResult() throws {
        let workBlock = try flattened(source("UI/WorkBlockView.swift"))
        let panel = try flattened(source("UI/VelvtPopoverContentView.swift"))
        let menuBar = try flattened(source("UI/MenuBarPopoverView.swift"))

        XCTAssertTrue(workBlock.contains("parts: [.ask, .acknowledgement], surfaceIsOnScreen: true"))
        XCTAssertTrue(workBlock.contains("if !coordinator.withholdsBlockSummary { resultSummary(result)"))
        XCTAssertTrue(panel.contains("if !workBlockCoordinator.withholdsBlockSummary { FocusFragmentationView("))
        XCTAssertTrue(panel.contains("parts: [.ask, .acknowledgement], surfaceIsOnScreen: surfaceIsOnScreen"))
        XCTAssertTrue(panel.contains("if showsLatestInsight { latestInsight"))
        XCTAssertTrue(
            panel.contains(
                "!workBlockCoordinator.withholdsBlockSummary || coordinator.insightAvailability == .available"))
        XCTAssertTrue(
            menuBar.contains("surfaceIsOnScreen: panelIsOnScreen, focusSessionIsOpen: showsFocusSession"))
        XCTAssertTrue(menuBar.contains("RatingPromptSettingsToggle(coordinator: workBlockCoordinator)"))
    }

    /// Never a notification: nothing that posts one reads the question.
    func testNoNotificationPathReadsTheQuestion() throws {
        let delivery = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/VelvtMac/Delivery")
        let files = try FileManager.default.contentsOfDirectory(
            at: delivery, includingPropertiesForKeys: nil)
        XCTAssertFalse(files.isEmpty)
        for file in files where file.pathExtension == "swift" {
            let text = try String(contentsOf: file, encoding: .utf8)
            for token in ["ratingPrompt", "RatingPrompt", "ratingOptIn", "QuestionAfterABlock"] {
                XCTAssertFalse(text.contains(token), "\(file.lastPathComponent) reads \(token)")
            }
        }
    }

    /// Every word on the surface is Rust's: the view holds no string literal
    /// that could reach the screen or VoiceOver.
    func testTheSurfaceHasNoSwiftAuthoredCopy() throws {
        let view = try source("UI/QuestionAfterABlockView.swift")
        let code = view.split(separator: "\n").filter {
            !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//")
        }
        for line in code {
            XCTAssertFalse(line.contains("\""), "a literal in the question's view: \(line)")
        }
    }

    // MARK: Screenshots, on request

    func testRenderTheQuestionWhenRequested() async throws {
        guard let output = ProcessInfo.processInfo.environment["VELVT_QUESTION_SCREENSHOT_DIR"] else {
            throw XCTSkip("Set VELVT_QUESTION_SCREENSHOT_DIR to render the question after a block")
        }
        let cases: [(String, WorkBlockSnapshot)] = [
            ("01-question-result-held-back.png", ended(prompt: prompt(.question))),
            ("02-after-not-now-link.png", ended(prompt: prompt(.link))),
            ("04-invitation.png", ended(prompt: nil, optIn: optIn())),
        ]
        for (name, snapshot) in cases {
            let (coordinator, _, messages) = started()
            messages.send(.workBlockState(snapshot))
            try await waitUntil { coordinator.snapshot != nil }
            try render(WorkBlockView(coordinator: coordinator), named: name, to: output)
        }

        let (answered, _, answeredMessages) = started()
        answeredMessages.send(.workBlockState(ended(prompt: prompt(.question))))
        try await waitUntil { answered.snapshot != nil }
        answered.answerRating(.most)
        answeredMessages.send(.workBlockState(ended(prompt: nil)))
        try await waitUntil { answered.questionAfterABlock == .acknowledged("Noted.") }
        try render(WorkBlockView(coordinator: answered), named: "03-answered.png", to: output)

        let (linked, _, linkedMessages) = started()
        linkedMessages.send(.workBlockState(ended(prompt: prompt(.link))))
        try await waitUntil { linked.snapshot != nil }
        linked.openRatingLink()
        try render(WorkBlockView(coordinator: linked), named: "02b-reopened-from-link.png", to: output)

        let (settingsCoordinator, _, settingsMessages) = started()
        settingsMessages.send(.ratingPromptSettings(settings(available: true, enabled: false)))
        try await waitUntil { settingsCoordinator.ratingPromptSettings != nil }
        try render(
            RatingPromptSettingsToggle(coordinator: settingsCoordinator),
            named: "05-settings-toggle.png", to: output, size: NSSize(width: 340, height: 110))
    }

    // MARK: Fixtures

    private func started(
        now: Date? = nil
    ) -> (WorkBlockCoordinator, FakeIPCClient, PassthroughSubject<ServerMessage, Never>) {
        let client = FakeIPCClient()
        let messages = PassthroughSubject<ServerMessage, Never>()
        let fixedNow = now ?? endedAt.addingTimeInterval(60)
        let coordinator = WorkBlockCoordinator(
            ipcClient: client, now: { fixedNow }, sleep: { _ in try await Task.sleep(for: .seconds(3_600)) })
        coordinator.start(messages: messages, connectionStatus: Empty<ConnectionStatus, Never>())
        return (coordinator, client, messages)
    }

    private func prompt(
        _ presentation: RatingPromptPresentation, block: UUID? = nil
    ) -> RatingPrompt {
        RatingPrompt(
            blockID: block ?? blockID,
            promptVersion: 1,
            presentation: presentation,
            question: "How much of this block was on what you meant to do?",
            options: [
                .init(answer: .all, label: "All or nearly all"),
                .init(answer: .most, label: "Most"),
                .init(answer: .half, label: "About half"),
                .init(answer: .lessThanHalf, label: "Less than half"),
                .init(answer: .little, label: "Little or none"),
            ],
            notNow: "Not now",
            footnote: "Optional. Your answer stays on this Mac unless you send it in an export.",
            acknowledgement: "Noted.",
            link: "Answer the question about this block",
            expiresAt: deadline
        )
    }

    private func optIn() -> RatingOptIn {
        RatingOptIn(
            body:
                "When a block ends, Velvt can ask how much of it was on what you meant to do. One tap, always optional, never during a block. Your answers stay on this Mac unless you send them in an export. You can turn it off in Settings.",
            turnOn: "Turn on",
            notNow: "Not now"
        )
    }

    private func settings(available: Bool, enabled: Bool) -> RatingPromptSettings {
        RatingPromptSettings(
            available: available, enabled: enabled,
            title: "Ask one question after each block",
            subtitle: "Never during a block. Answers stay on this Mac unless you send an export.")
    }

    private func ended(
        phase: WorkBlockPhase = .completed,
        block: UUID? = nil,
        prompt: RatingPrompt?,
        optIn: RatingOptIn? = nil
    ) -> WorkBlockSnapshot {
        let live = phase == .active || phase == .paused
        return WorkBlockSnapshot(
            stateVersion: 1, phase: phase, blockID: block ?? blockID,
            intention: nil, purpose: .deepWork, intensity: .medium,
            plannedDurationSeconds: 1_800, elapsedDurationSeconds: 1_800,
            remainingDurationSeconds: 0,
            startedAt: endedAt.addingTimeInterval(-1_800),
            endsAt: live ? endedAt : nil, pausedAt: nil, recoveredAfterRestart: false,
            currentCategory: nil, anchorCategory: nil,
            classificationStatus: .classified, confidence: .high,
            statusLine: "The planned work block is complete.",
            result: live
                ? nil
                : WorkBlockResult(
                    plannedDurationSeconds: 1_800, elapsedDurationSeconds: 1_800,
                    longestUninterruptedSeconds: 1_140, switchAwayCount: 3, recoveryCount: 2,
                    confidence: .high, coverage: .good, coverageRatio: 0.92,
                    safeEvidenceCategory: "FOCUS_WORK",
                    observation: "You left focus work 3 times and came back 2 of them.",
                    nextAction: WorkBlockNextAction(
                        actionID: "plan_next", label: "Plan another session", durationSeconds: 1_500)),
            activeIntervention: nil,
            ratingPrompt: prompt,
            ratingOptIn: optIn
        )
    }

    /// The source with every run of whitespace made one space, so these
    /// checks follow the code and not its line breaks.
    private func flattened(_ text: String) -> String {
        text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    private func source(_ path: String) throws -> String {
        try String(
            contentsOf: URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
                .appendingPathComponent("Sources/VelvtMac/\(path)"),
            encoding: .utf8)
    }

    private func render<V: View>(
        _ view: V, named name: String, to outputDirectory: String,
        size: NSSize = NSSize(width: 400, height: 560)
    ) throws {
        let root = AnyView(
            view
                .frame(width: size.width, height: size.height, alignment: .topLeading)
                .background(VelvtSurface.ground)
                .preferredColorScheme(.dark))
        let hostingView = NSHostingView(rootView: root)
        hostingView.frame = NSRect(origin: .zero, size: size)
        hostingView.layoutSubtreeIfNeeded()
        guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
            return XCTFail("Unable to create snapshot bitmap")
        }
        hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else {
            return XCTFail("Unable to encode snapshot PNG")
        }
        let directory = URL(fileURLWithPath: outputDirectory, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try data.write(to: directory.appendingPathComponent(name), options: .atomic)
    }

    private func waitUntil(
        timeout: Duration = .seconds(1),
        condition: @escaping @MainActor () -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !condition() {
            if clock.now >= deadline {
                XCTFail("condition timed out")
                return
            }
            await Task.yield()
        }
    }

    private func waitUntilAsync(
        timeout: Duration = .seconds(1),
        condition: @escaping @MainActor () async -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !(await condition()) {
            if clock.now >= deadline {
                XCTFail("condition timed out")
                return
            }
            await Task.yield()
        }
    }
}

extension ClientMessage {
    fileprivate var isRatingAnswer: Bool {
        if case .rateWorkBlock = self { true } else { false }
    }
}

private actor DelayRecorder {
    private(set) var delays: [TimeInterval] = []

    func record(_ delay: TimeInterval) {
        delays.append(delay)
    }
}

/// A sleep that returns only when the test opens it, and never throws, so
/// only the coordinator's own disarming can stop what follows it.
private actor Gate {
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var opened = false

    var isWaiting: Bool { !waiters.isEmpty }

    func wait() async {
        guard !opened else { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func open() {
        opened = true
        for waiter in waiters {
            waiter.resume()
        }
        waiters.removeAll()
    }
}
