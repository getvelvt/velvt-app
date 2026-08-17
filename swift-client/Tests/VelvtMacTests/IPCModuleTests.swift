import XCTest

@testable import VelvtMac

final class IPCModuleTests: XCTestCase {
    private let encoder = IPCMessageCodec.makeEncoder()
    private let decoder = IPCMessageCodec.makeDecoder()

    func testEveryClientMessageRoundTrips() throws {
        let messages: [ClientMessage] = [
            .clientHello(ClientHello(expectedProtocolVersion: 3, clientVersion: "1.2.3")),
            .rawEvent(
                RawEventMessage(
                    eventID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
                    occurredAt: Date(timeIntervalSince1970: 1_700_000_000),
                    durationSeconds: 120,
                    appName: "local-only",
                    windowTitle: "local-only",
                    bundleID: nil,
                    focusedDocumentURL: "https://local-only.example/private"
                )
            ),
            .correctEventClassification(
                CorrectEventClassification(
                    eventID: UUID(uuidString: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")!,
                    stableID: "abs_safe",
                    category: "REFERENCE",
                    localActivityName: "Research reading"
                )
            ),
            .updateClassificationOverride(
                UpdateClassificationOverride(
                    stableID: "abs_safe",
                    category: "REFERENCE",
                    localActivityName: "Research reading"
                )
            ),
            .requestCorrectionHistory(
                RequestCorrectionHistory(query: "Research", offset: 20, pageSize: 20)
            ),
      .errorResponse(ErrorResponse(code: "safe_error", message: "safe", relatedEventID: nil)),
        ]

        for message in messages {
            let data = try encoder.encode(message)
            XCTAssertEqual(try decoder.decode(ClientMessage.self, from: data), message)
        }
    }

    func testEveryServerMessageRoundTrips() throws {
        let eventID = UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!
        let messages: [ServerMessage] = [
            .serverHello(ServerHello(protocolVersion: 3)),
            .acknowledged(Acknowledged()),
            .versionMismatch(VersionMismatch(serverProtocolVersion: 2, clientProtocolVersion: 1)),
            .malformedMessage(MalformedMessage(code: .invalidMessage)),
            .rawEventAck(RawEventAcknowledgement(eventID: eventID, status: .accepted, dropReason: nil)),
            .insightPayload(
                InsightPayload(
                    date: "2026-06-13",
                    text: "ready-to-display",
                    evidence: InsightEvidence(
                        observation: "You made 8 meaningful switches.",
                        comparison: "That stayed within your usual range.",
                        suggestedAction: "Try protecting your next 20 minutes for one work lane.",
                        toneStage: .stable,
                        observationType: "repeated_switching_loop",
                        templateID: "repeated_switching_loop",
                        metricValue: 8,
                        metricUnit: "transitions",
                        timeWindow: ["kind": "current_day", "date": "2026-06-13"],
                        safeCategories: ["document"],
                        confidence: "high",
                        coverage: 0.91,
                        baselineStatus: "mature",
                        actionMinutes: 20,
                        repetitionDays: 1,
                        nextActionID: "protect_next_20",
                        direction: "stable",
                        magnitude: 2.5
                    ),
                    confidenceLevel: .high,
                    lowConfidence: false,
                    generatedAt: Date(timeIntervalSince1970: 1_700_000_000)
                )
            ),
            .historyPayload(
                HistoryPayload(
                    days: 1,
                    summaries: [
                        DailySummary(
                            date: "2026-06-13",
                            status: .ready,
                            eventCount: 1,
                            focusScore: 0.8,
                            fragmentationScore: nil,
                            confidenceLevel: .medium,
                            activeSeconds: 60
                        )
                    ]
                )
            ),
            .serviceStatus(ServiceStatus(state: .ready, reason: nil)),
            .menuStatus(
                MenuStatus(
                    deviceID: "device-1",
                    cloudReady: true,
                    uploadStatus: "ready",
                    lastUploadErrorCode: nil,
                    nextUploadAttemptAt: nil,
                    pendingUploadBatchCount: 0,
                    failedUploadBatchCount: 0,
                    rejectedUploadBatchCount: 0,
                    queuedEventCount: 0,
                    queuedEvents: [],
                    correctionHistory: [
                        ClassificationCorrectionSummary(
                            stableID: "abs_safe",
                            label: "reference:inferred",
                            localLabel: "Research reading",
                            category: "REFERENCE",
                            updatedAt: Date(timeIntervalSince1970: 1_700_000_000)
                        )
                    ]
                )
            ),
            .correctionHistoryPage(
                CorrectionHistoryPage(
                    items: [
                        ClassificationCorrectionSummary(
                            stableID: "abs_safe",
                            label: "reference:inferred",
                            localLabel: "Research reading",
                            category: "REFERENCE",
                            updatedAt: Date(timeIntervalSince1970: 1_700_000_000)
                        )
                    ],
                    offset: 20,
                    pageSize: 20,
                    totalCount: 45,
                    hasMore: true
                )
            ),
      .privacyViolationAlert(
        PrivacyViolationAlert(code: "raw_field_rejected", message: "safe rejection")),
            .errorResponse(ErrorResponse(code: "safe_error", message: "safe", relatedEventID: nil)),
      .unknown(type: "future_message"),
        ]

        for message in messages {
            let data = try encoder.encode(message)
            XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), message)
        }
    }

    func testTaggedEnumUsesTypeDiscriminator() throws {
        let data = try encoder.encode(
            ClientMessage.clientHello(ClientHello(expectedProtocolVersion: 3, clientVersion: "1.2.3"))
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let payload = try XCTUnwrap(object["payload"] as? [String: Any])

        XCTAssertEqual(object["type"] as? String, "client_hello")
        XCTAssertEqual(payload["expected_protocol_version"] as? Int, 3)
    }

    func testLatestInsightRequestUsesProtocolWireShape() throws {
        let message = ClientMessage.requestLatestInsight(RequestLatestInsight(date: "2026-06-20"))
        let data = try encoder.encode(message)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let payload = try XCTUnwrap(object["payload"] as? [String: Any])

        XCTAssertEqual(object["type"] as? String, "request_latest_insight")
        XCTAssertEqual(payload["date"] as? String, "2026-06-20")
    }

    func testFlushUploadQueueUsesEmptyProtocolWireShape() throws {
        let data = try encoder.encode(ClientMessage.flushUploadQueue)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let payload = try XCTUnwrap(object["payload"] as? [String: Any])

        XCTAssertEqual(object["type"] as? String, "flush_upload_queue")
        XCTAssertTrue(payload.isEmpty)
    }

    func testCorrectionCarriesLocalActivityNameAcrossLocalIPC() throws {
        let data = try encoder.encode(
            ClientMessage.correctEventClassification(
                CorrectEventClassification(
                    eventID: UUID(uuidString: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")!,
                    stableID: "abs_safe",
                    category: "REFERENCE",
                    localActivityName: "Research reading"
                )
            )
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let payload = try XCTUnwrap(object["payload"] as? [String: Any])

        XCTAssertEqual(object["type"] as? String, "correct_event_classification")
        XCTAssertEqual(payload["category"] as? String, "REFERENCE")
        XCTAssertEqual(payload["local_activity_name"] as? String, "Research reading")
    }

    func testCorrectionHistoryWireContractIsBoundedAndLocalOnly() throws {
        let request = ClientMessage.requestCorrectionHistory(
            RequestCorrectionHistory(query: "Private alias", offset: 20, pageSize: 500)
        )
        let requestData = try encoder.encode(request)
        let requestObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: requestData) as? [String: Any]
        )
        let requestPayload = try XCTUnwrap(requestObject["payload"] as? [String: Any])
        XCTAssertEqual(requestObject["type"] as? String, "request_correction_history")
        XCTAssertEqual(requestPayload["page_size"] as? Int, 20)
        XCTAssertEqual(requestPayload["offset"] as? Int, 20)

        let update = ClientMessage.updateClassificationOverride(
            UpdateClassificationOverride(
                stableID: "abs_local",
                category: "COMMUNICATION",
                localActivityName: "Private alias"
            )
        )
        let updateObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: encoder.encode(update)) as? [String: Any]
        )
        XCTAssertEqual(updateObject["type"] as? String, "update_classification_override")
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: encoder.encode(update)), update)
    }

    func testFlushUploadQueueRoundTrips() throws {
        let message = ClientMessage.flushUploadQueue

        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: encoder.encode(message)), message)
    }

    func testWorkBlockCommandsUseTypedLocalProtocolShapes() throws {
        let blockID = UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!
        let messages: [ClientMessage] = [
      .startWorkBlock(
        .init(
                intention: "Private local intention",
                plannedDurationSeconds: 1_500,
                purpose: .deepWork,
                intensity: .medium
            )),
            .pauseWorkBlock(.init(blockID: blockID)),
            .resumeWorkBlock(.init(blockID: blockID)),
            .endWorkBlock(.init(blockID: blockID)),
            .requestWorkBlockState,
            .acceptWorkBlockRecovery(.init(blockID: blockID, actionID: "protect_next_10")),
            .workBlockLifecycle(.init(event: .sleep)),
            .clearWorkBlockData,
        ]

        for message in messages {
            XCTAssertEqual(try decoder.decode(ClientMessage.self, from: encoder.encode(message)), message)
        }
    let start = try XCTUnwrap(
      JSONSerialization.jsonObject(with: encoder.encode(messages[0])) as? [String: Any])
        XCTAssertEqual(start["type"] as? String, "start_work_block")
    }

  func testLocalDashboardRequestCarriesOnlyBoundedWindowAndTimeZoneOffset() throws {
    let message = ClientMessage.requestLocalDashboard(
      .init(windowSeconds: 3_600, utcOffsetSeconds: -14_400)
    )
    let data = try encoder.encode(message)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    let payload = try XCTUnwrap(object["payload"] as? [String: Any])

    XCTAssertEqual(object["type"] as? String, "request_local_dashboard")
    XCTAssertEqual(payload["window_seconds"] as? Int, 3_600)
    XCTAssertEqual(payload["utc_offset_seconds"] as? Int, -14_400)
    XCTAssertEqual(Set(payload.keys), ["window_seconds", "utc_offset_seconds"])
  }

  func testFocusFragmentationPreservesExactWindowBoundsOnTheWire() throws {
    let startedAt = Date(timeIntervalSince1970: 1_800_000_000)
    let endedAt = startedAt.addingTimeInterval(3_600)
    let focus = LocalFocusFragmentation(
      blockID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
      phase: .active,
      windowLabel: "Most recent 60 work-block minutes",
      windowStartedAt: startedAt,
      windowEndedAt: endedAt,
      plannedDurationSeconds: 3_600,
      elapsedDurationSeconds: 3_600,
      longestUninterruptedSeconds: 3_600,
      observedSwitchCount: 0,
      recoveryCount: 0,
      coverage: .good,
      coverageRatio: 1,
      comparison: nil,
      observation: "No category switches were observed.",
      nextAction: "Protect the next 10 minutes.",
      segments: [],
      transitions: [],
      clusters: []
    )

    let data = try encoder.encode(focus)
    let payload = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

    XCTAssertNotNil(payload["window_started_at"])
    XCTAssertNotNil(payload["window_ended_at"])
    XCTAssertEqual(try decoder.decode(LocalFocusFragmentation.self, from: data), focus)
  }

    func testWorkBlockResultHasExactlyOneActionAndNoIntention() throws {
        let result = WorkBlockResult(
            plannedDurationSeconds: 1_500,
            elapsedDurationSeconds: 1_500,
            longestUninterruptedSeconds: 900,
            switchAwayCount: 2,
            recoveryCount: 1,
            confidence: .high,
            coverage: .good,
            coverageRatio: 0.9,
            safeEvidenceCategory: "FOCUS_WORK",
            observation: "Velvt observed two category changes.",
            nextAction: .init(
                actionID: "protect_next_10",
                label: "Protect the next 10 minutes.",
                durationSeconds: 600
            )
        )
    let value = try XCTUnwrap(
      JSONSerialization.jsonObject(with: encoder.encode(result)) as? [String: Any])
        XCTAssertNotNil(value["next_action"] as? [String: Any])
        XCTAssertNil(value["next_actions"])
        XCTAssertNil(value["intention"])
    }

    func testFocusAndQuietHoursMessagesRoundTripOnTheWire() throws {
        let focus = ClientMessage.focusStateChanged(
            .init(
                active: true,
                occurredAt: Date(timeIntervalSince1970: 1_800_000_000),
                utcOffsetSeconds: 3_600
            )
        )
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: encoder.encode(focus)), focus)

        let reply = ClientMessage.respondQuietHoursOffer(.init(accepted: true))
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: encoder.encode(reply)), reply)

        let offer = ServerMessage.quietHoursOffer(
            QuietHoursOffer(
                ruleVersion: 1,
                lateNightDays: 3,
                startLocalMinutes: 1_320,
                endLocalMinutes: 420,
                body: "Velvt can hold its own notifications overnight."
            )
        )
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: encoder.encode(offer)), offer)
        XCTAssertEqual(offer.safeLogDescription, "quiet_hours_offer")
    }

    /// Compatibility across the v26 bump: a result without the new DND
    /// fields decodes to an empty outcome list, and DND outcomes round-trip
    /// with the exact Rust wire vocabulary.
    func testWorkBlockResultDndFieldsAreOptionalAndRoundTrip() throws {
        let v25Payload = Data(
            """
            {
                "planned_duration_seconds": 1500,
                "elapsed_duration_seconds": 1500,
                "longest_uninterrupted_seconds": 900,
                "switch_away_count": 1,
                "recovery_count": 1,
                "confidence": "high",
                "coverage": "good",
                "coverage_ratio": 0.9,
                "safe_evidence_category": null,
                "observation": "Velvt observed one sustained category pattern.",
                "next_action": {
                    "action_id": "protect_next_10",
                    "label": "Protect the next 10 minutes.",
                    "duration_seconds": 600
                }
            }
            """.utf8)
        let decoded = try decoder.decode(WorkBlockResult.self, from: v25Payload)
        XCTAssertTrue(decoded.dndOutcomes.isEmpty)
        XCTAssertNil(decoded.reconciliation)

        let withOutcomes = WorkBlockResult(
            plannedDurationSeconds: 1_500,
            elapsedDurationSeconds: 1_500,
            longestUninterruptedSeconds: 900,
            switchAwayCount: 1,
            recoveryCount: 1,
            confidence: .high,
            coverage: .good,
            coverageRatio: 0.9,
            safeEvidenceCategory: nil,
            observation: "Velvt observed one sustained category pattern.",
            nextAction: .init(
                actionID: "protect_next_10",
                label: "Protect the next 10 minutes.",
                durationSeconds: 600
            ),
            dndOutcomes: [.completedUnderDnd, .deliverySuppressedDnd],
            reconciliation: "Do Not Disturb was on and the block completed as planned."
        )
        let data = try encoder.encode(withOutcomes)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(
            object["dnd_outcomes"] as? [String],
            ["completed_under_dnd", "delivery_suppressed_dnd"]
        )
        XCTAssertEqual(try decoder.decode(WorkBlockResult.self, from: data), withOutcomes)
    }

    func testEarlyLocalSignalContainsNoRawActivityFields() throws {
        let signal = LocalEarlySignal(
            status: .ready,
            observedFrom: Date(timeIntervalSince1970: 0),
            observedThrough: Date(timeIntervalSince1970: 60),
            observedSeconds: 60,
            requiredSeconds: 0,
            evidenceEventCount: 1,
            focusedSeconds: 60,
            meaningfulSwitchCount: 0,
            longestUninterruptedSeconds: 60,
            observation: "Your activity is still settling into a steady context.",
            suggestedAction: "Protect a short block for the context you want to stay with.",
            actionMinutes: 10
        )
        let encoded = String(data: try JSONEncoder().encode(signal), encoding: .utf8) ?? ""

        for forbidden in [
            "app_name", "window_title", "url", "filename", "path",
            "contact", "intention", "local_label", "stable_id",
        ] {
            XCTAssertFalse(encoded.contains(forbidden), "Found forbidden field \(forbidden)")
        }
    }

    func testCacheEmptyRoundTrips() throws {
        let message = ServerMessage.cacheEmpty(CacheEmpty(payloadType: "insight_payload"))
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: encoder.encode(message)), message)
    }

    func testDecoderRejectsMissingPayload() {
        let data = Data(#"{"type":"server_hello"}"#.utf8)

        XCTAssertThrowsError(try decoder.decode(ServerMessage.self, from: data))
    }

    func testUnknownServerMessageDecodesWithoutPayloadValues() throws {
    let data = Data(
      #"{"type":"future_message","payload":{"raw_title":"must-not-be-retained"}}"#.utf8)

    XCTAssertEqual(
      try decoder.decode(ServerMessage.self, from: data), .unknown(type: "future_message"))
    }

    func testFakeClientDispatchesInjectedMessage() async throws {
        let client = FakeIPCClient()
        let message = ServerMessage.serviceStatus(ServiceStatus(state: .ready, reason: nil))
        let received = expectation(description: "received message")

        Task {
            for await incoming in client.incomingMessages {
                XCTAssertEqual(incoming, message)
                received.fulfill()
                break
            }
        }

        client.inject(message)
        await fulfillment(of: [received], timeout: 1)
    }

    func testBackoffDoublesAndCaps() {
        let backoff = ReconnectBackoff(maximumDelay: 4, jitter: { 1 })

        XCTAssertEqual(backoff.delay(forAttempt: 1), 1)
        XCTAssertEqual(backoff.delay(forAttempt: 2), 2)
        XCTAssertEqual(backoff.delay(forAttempt: 3), 4)
        XCTAssertEqual(backoff.delay(forAttempt: 4), 4)
    }

    func testBackoffJitterStaysWithinTwentyPercent() {
        XCTAssertEqual(ReconnectBackoff(jitter: { 0.8 }).delay(forAttempt: 3), 3.2, accuracy: 0.0001)
        XCTAssertEqual(ReconnectBackoff(jitter: { 1.2 }).delay(forAttempt: 3), 4.8, accuracy: 0.0001)
    }

    func testBackoffBaseDelayScalesEveryAttemptProportionally() {
        let backoff = ReconnectBackoff(baseDelay: 0.25, maximumDelay: 10, jitter: { 1 })

        XCTAssertEqual(backoff.delay(forAttempt: 1), 0.25, accuracy: 0.0001)
        XCTAssertEqual(backoff.delay(forAttempt: 2), 0.5, accuracy: 0.0001)
        XCTAssertEqual(backoff.delay(forAttempt: 3), 1, accuracy: 0.0001)
        XCTAssertEqual(backoff.delay(forAttempt: 6), 8, accuracy: 0.0001)
        XCTAssertEqual(backoff.delay(forAttempt: 7), 10, accuracy: 0.0001, "capped at maximumDelay")
    }
}
// MARK: - Auth IPC DTO contract tests (proto v6)

final class AuthIPCContractTests: XCTestCase {
    private let encoder = IPCMessageCodec.makeEncoder()
    private let decoder = IPCMessageCodec.makeDecoder()

    // MARK: ClientMessage auth variants

    func testSignUpRoundTrip() throws {
        let msg = ClientMessage.signUp(SignUpRequest(email: "a@b.com", password: "pw"))
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: data), msg)
    }

    func testSignUpDiscriminator() throws {
    let data = try encoder.encode(
      ClientMessage.signUp(SignUpRequest(email: "a@b.com", password: "pw")))
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "sign_up")
        let payload = try XCTUnwrap(obj["payload"] as? [String: Any])
        XCTAssertEqual(payload["email"] as? String, "a@b.com")
        // Password must be in the payload (not echoed in logs but encoded correctly)
        XCTAssertNotNil(payload["password"])
    }

    func testLogInRoundTrip() throws {
        let msg = ClientMessage.logIn(LogInRequest(email: "x@y.com", password: "secret"))
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: data), msg)
    }

    func testLogInDiscriminator() throws {
    let data = try encoder.encode(
      ClientMessage.logIn(LogInRequest(email: "x@y.com", password: "s")))
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "log_in")
    }

    func testLogOutRoundTrip() throws {
        let data = try encoder.encode(ClientMessage.logOut)
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: data), .logOut)
    }

    func testLogOutDiscriminator() throws {
        let data = try encoder.encode(ClientMessage.logOut)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "log_out")
    }

    func testDeleteAccountRoundTrip() throws {
        let data = try encoder.encode(ClientMessage.deleteAccount)
        XCTAssertEqual(try decoder.decode(ClientMessage.self, from: data), .deleteAccount)
    }

    func testDeleteAccountDiscriminator() throws {
        let data = try encoder.encode(ClientMessage.deleteAccount)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "delete_account")
    }

    // MARK: ServerMessage auth variants

    func testAuthSuccessRoundTrip() throws {
        let expires = Date(timeIntervalSince1970: 1_750_000_000)
        let msg = ServerMessage.authSuccess(
      AuthSuccess(
        userId: "u1", deviceId: "device-1", accessToken: "at", refreshToken: "rt",
        expiresAt: expires)
        )
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), msg)
    }

    /// Regression test for a real bug: the Rust service serializes
    /// `chrono::DateTime<Utc>` with fractional seconds (captured verbatim
    /// from a live `/v1/auth/signup` response relayed over IPC), but
    /// Foundation's `.iso8601` decoding strategy cannot parse fractional
    /// seconds. Every `AuthSuccess` was silently failing to decode, which
    /// `UnixSocketIPCClient` treated as a dropped connection and
    /// reconnected on — so login/signup spun forever even though the
    /// server had already responded 200/201. This must decode without
    /// throwing and without going through `encoder` (a round-trip through
    /// Swift's own encoder never reproduces the bug, since Swift never
    /// emits fractional seconds itself).
    func testAuthSuccessWithFractionalSecondsFromRealServerDecodesSuccessfully() throws {
        let wire = """
        {"type":"auth_success","payload":{"user_id":"f14e0762-cc11-44c3-92f3-302e1762719f",\
        "device_id":"device-1","access_token":"at","refresh_token":"rt","expires_at":"2026-06-19T21:36:13.182093Z"}}
        """.data(using: .utf8)!

        let message = try decoder.decode(ServerMessage.self, from: wire)

        guard case .authSuccess(let success) = message else {
            XCTFail("Expected authSuccess, got \(message)")
            return
        }
        XCTAssertEqual(success.userId, "f14e0762-cc11-44c3-92f3-302e1762719f")
        XCTAssertEqual(
            success.expiresAt.timeIntervalSince1970,
            1_781_904_973.182093,
            accuracy: 0.001
        )
    }

    func testAuthSuccessPayloadUsesSnakeCaseKeys() throws {
        let msg = ServerMessage.authSuccess(
      AuthSuccess(
        userId: "u1", deviceId: "device-1", accessToken: "at", refreshToken: "rt",
                        expiresAt: Date(timeIntervalSince1970: 1_750_000_000))
        )
        let data = try encoder.encode(msg)
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "auth_success")
        let payload = try XCTUnwrap(obj["payload"] as? [String: Any])
        XCTAssertNotNil(payload["user_id"])
        XCTAssertNotNil(payload["device_id"])
        XCTAssertNotNil(payload["access_token"])
        XCTAssertNotNil(payload["refresh_token"])
        XCTAssertNotNil(payload["expires_at"])
        // Must not leak tokens via raw key names
        XCTAssertNil(payload["userId"])
        XCTAssertNil(payload["accessToken"])
    }

    func testAuthFailureRoundTrip() throws {
    let msg = ServerMessage.authFailure(
      AuthFailure(code: .invalidCredentials, message: "Bad creds"))
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), msg)
    }

    func testAuthFailureAllCodes() throws {
        let codes: [AuthFailureCode] = [.invalidCredentials, .networkError, .serverError]
        for code in codes {
            let msg = ServerMessage.authFailure(AuthFailure(code: code, message: "m"))
            let data = try encoder.encode(msg)
            let decoded = try decoder.decode(ServerMessage.self, from: data)
            XCTAssertEqual(decoded, msg, "Round-trip failed for code \(code)")
        }
    }

    func testAccountDeletionAcceptedRoundTrip() throws {
        let data = try encoder.encode(ServerMessage.accountDeletionAccepted)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), .accountDeletionAccepted)
    }

    func testNeedsReauthRoundTrip() throws {
        let msg = ServerMessage.needsReauth(NeedsReauth(reason: "token_expired"))
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), msg)
    }

    func testDeviceRevokedRoundTrip() throws {
        let msg = ServerMessage.deviceRevoked(DeviceRevoked(message: "Revoked by admin"))
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), msg)
    }

    func testDeviceRevokedDiscriminator() throws {
        let data = try encoder.encode(ServerMessage.deviceRevoked(DeviceRevoked(message: "x")))
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "device_revoked")
    }

    func testNotificationPayloadRoundTrips() throws {
        let msg = ServerMessage.notificationPayload(
            NotificationPayload(
                notificationID: UUID(uuidString: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")!,
                title: "Daily insight",
                body: "ready-to-display",
                insightDate: "2026-06-15",
                doNotDisturbUntil: Date(timeIntervalSince1970: 1_700_000_000)
            )
        )
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), msg)
    }

    func testNotificationPayloadRoundTripsWithoutDoNotDisturb() throws {
        let msg = ServerMessage.notificationPayload(
            NotificationPayload(
                notificationID: UUID(),
                title: "Daily insight",
                body: "ready-to-display",
                insightDate: "2026-06-15",
                doNotDisturbUntil: nil
            )
        )
        let data = try encoder.encode(msg)
        XCTAssertEqual(try decoder.decode(ServerMessage.self, from: data), msg)
    }

    func testNotificationPayloadDiscriminator() throws {
        let data = try encoder.encode(
            ServerMessage.notificationPayload(
        NotificationPayload(
          notificationID: UUID(), title: "t", body: "b", insightDate: "2026-06-15",
          doNotDisturbUntil: nil)
            )
        )
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(obj["type"] as? String, "notification_payload")
    }

    func testAuthSuccessDoesNotDecodeAsUnknown() throws {
    let raw =
      #"{"type":"auth_success","payload":{"user_id":"u","device_id":"device-1","access_token":"a","refresh_token":"r","expires_at":"2026-06-15T00:00:00Z"}}"#
        let decoded = try decoder.decode(ServerMessage.self, from: Data(raw.utf8))
        guard case .authSuccess(let s) = decoded else {
            XCTFail("Expected .authSuccess, got \(decoded)")
            return
        }
        XCTAssertEqual(s.userId, "u")
    }

    func testAllNewClientMessagesRoundTripTogether() throws {
        let messages: [ClientMessage] = [
            .signUp(SignUpRequest(email: "a@b.com", password: "pw")),
            .logIn(LogInRequest(email: "x@y.com", password: "s")),
      .authSession(
        AuthSession(
                deviceId: "device-1",
                accessToken: "a",
                refreshToken: "r",
                expiresAt: Date(timeIntervalSince1970: 1_750_000_000)
            )),
            .logOut,
            .deleteAccount,
            .removeClassificationOverride(.init(stableID: "abs_safe")),
            .resetClassificationOverrides,
        ]
        for msg in messages {
            let data = try encoder.encode(msg)
      XCTAssertEqual(
        try decoder.decode(ClientMessage.self, from: data), msg,
                           "Round-trip failed for \(msg)")
        }
    }

    // MARK: DTO extensibility

    func testUnknownServerMessageTypeIsForwardCompatible() throws {
        // Any future server message type the Swift client doesn't know about must
        // produce .unknown(type:) and must not crash or corrupt state. This is the
        // forward-compatibility guarantee for proto extensibility.
    let raw =
      #"{"type":"future_server_feature","payload":{"sensitive_field":"must-not-be-retained"}}"#
        let decoded = try decoder.decode(ServerMessage.self, from: Data(raw.utf8))
        guard case .unknown(let t) = decoded else {
            XCTFail("Expected .unknown; got \(decoded)")
            return
        }
        XCTAssertEqual(t, "future_server_feature")
    }

    func testUnknownClientMessageTypeThrowsOnDecode() {
        // ClientMessage decode is intentionally strict: the Swift client is the
        // authoritative source of client messages and should never receive unknown
        // ones. Decoding an unknown type must throw rather than silently succeed.
        let raw = #"{"type":"unknown_client_cmd","payload":{}}"#
    XCTAssertThrowsError(
      try decoder.decode(ClientMessage.self, from: Data(raw.utf8)),
                             "Unknown ClientMessage types must throw DecodingError")
    }

    func testAllNewServerMessagesRoundTripTogether() throws {
        let expires = Date(timeIntervalSince1970: 1_750_000_000)
        let messages: [ServerMessage] = [
      .authSuccess(
        AuthSuccess(
          userId: "u", deviceId: "device-1", accessToken: "a", refreshToken: "r", expiresAt: expires
        )),
      .authSessionUpdated(
        AuthSession(deviceId: "device-1", accessToken: "a", refreshToken: "r", expiresAt: expires)),
            .authFailure(AuthFailure(code: .serverError, message: "oops")),
            .accountDeletionAccepted,
            .needsReauth(NeedsReauth(reason: "expired")),
            .deviceRevoked(DeviceRevoked(message: "revoked")),
            .notificationPayload(
        NotificationPayload(
          notificationID: UUID(), title: "t", body: "b", insightDate: "2026-06-15",
          doNotDisturbUntil: nil)
            ),
        ]
        for msg in messages {
            let data = try encoder.encode(msg)
      XCTAssertEqual(
        try decoder.decode(ServerMessage.self, from: data), msg,
                           "Round-trip failed for \(msg)")
        }
    }
}
