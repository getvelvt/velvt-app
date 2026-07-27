use chrono::{NaiveDate, TimeZone, Utc};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use uuid::Uuid;
use velvt_shared_types::*;

fn assert_round_trip<T>(value: T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_vec(&value).unwrap();
    let decoded = serde_json::from_slice::<T>(&encoded).unwrap();
    assert_eq!(decoded, value);
}

fn event_id() -> Uuid {
    Uuid::parse_str("f47ac10b-58cc-4372-a567-0e02b2c3d479").unwrap()
}

fn timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 13, 12, 0, 0).unwrap()
}

#[test]
fn client_message_variants_round_trip() {
    let messages = [
        ClientMessage::ClientHello(ClientHello {
            expected_protocol_version: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        }),
        ClientMessage::RawEvent(RawEvent {
            event_id: event_id(),
            occurred_at: timestamp(),
            app_name: "local-only".into(),
            window_title: "local-only".into(),
            bundle_id: None,
            duration_seconds: 0,
        }),
        ClientMessage::ErrorResponse(ErrorResponse {
            code: "client_error".into(),
            message: "safe".into(),
            related_event_id: None,
        }),
    ];

    for message in messages {
        assert_round_trip(message);
    }
}

#[test]
fn flush_upload_queue_uses_empty_payload_and_round_trips() {
    let message = ClientMessage::FlushUploadQueue(FlushUploadQueue {});

    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        json!({"type": "flush_upload_queue", "payload": {}})
    );
    assert_round_trip(message);
}

#[test]
fn classification_correction_round_trips_without_raw_app_data() {
    let message = ClientMessage::CorrectEventClassification(CorrectEventClassification {
        event_id: event_id(),
        stable_id: "abs_safe".into(),
        category: "COMMUNICATION".into(),
        local_activity_name: None,
    });

    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        json!({
            "type": "correct_event_classification",
            "payload": {
                "event_id": event_id(),
                "stable_id": "abs_safe",
                "category": "COMMUNICATION"
            }
        })
    );
    assert_round_trip(message);
}

#[test]
fn local_activity_name_round_trips_only_on_local_ipc_and_is_redacted_from_debug() {
    let correction = CorrectEventClassification {
        event_id: event_id(),
        stable_id: "abs_private_rule".into(),
        category: "REFERENCE".into(),
        local_activity_name: Some("Research reading".into()),
    };
    let message = ClientMessage::CorrectEventClassification(correction.clone());

    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        json!({
            "type": "correct_event_classification",
            "payload": {
                "event_id": event_id(),
                "stable_id": "abs_private_rule",
                "category": "REFERENCE",
                "local_activity_name": "Research reading"
            }
        })
    );
    let debug = format!("{correction:?}");
    assert!(!debug.contains("Research reading"));
    assert!(!debug.contains("abs_private_rule"));
    assert_round_trip(message);
}

#[test]
fn raw_activity_fields_are_redacted_from_debug_and_error_safe_surfaces() {
    let event = RawEvent {
        event_id: event_id(),
        occurred_at: timestamp(),
        app_name: "PRIVATE_APP_SENTINEL".into(),
        window_title: "PRIVATE_WINDOW_SENTINEL".into(),
        bundle_id: Some("private.bundle.sentinel".into()),
        duration_seconds: 30,
    };
    let debug = format!("{event:?}");

    for forbidden in [
        "PRIVATE_APP_SENTINEL",
        "PRIVATE_WINDOW_SENTINEL",
        "private.bundle.sentinel",
    ] {
        assert!(!debug.contains(forbidden));
    }
}

#[test]
fn correction_history_messages_are_bounded_typed_and_redacted() {
    let request = RequestCorrectionHistory {
        query: Some("PRIVATE_ALIAS_QUERY".into()),
        offset: 20,
        page_size: 20,
    };
    let update = UpdateClassificationOverride {
        stable_id: "abs_PRIVATE_LOCAL_ID".into(),
        category: "REFERENCE".into(),
        local_activity_name: Some("PRIVATE_ALIAS_VALUE".into()),
    };
    let page = CorrectionHistoryPage {
        items: vec![ClassificationCorrectionSummary {
            stable_id: "abs_PRIVATE_LOCAL_ID".into(),
            label: "reference:inferred".into(),
            local_label: Some("PRIVATE_ALIAS_VALUE".into()),
            category: "REFERENCE".into(),
            updated_at: timestamp(),
        }],
        offset: 0,
        page_size: 20,
        total_count: 45,
        has_more: true,
    };

    assert_round_trip(ClientMessage::RequestCorrectionHistory(request.clone()));
    assert_round_trip(ClientMessage::UpdateClassificationOverride(update.clone()));
    assert_round_trip(ServerMessage::CorrectionHistoryPage(page.clone()));
    for debug in [
        format!("{request:?}"),
        format!("{update:?}"),
        format!("{page:?}"),
    ] {
        for forbidden in [
            "PRIVATE_ALIAS_QUERY",
            "PRIVATE_ALIAS_VALUE",
            "abs_PRIVATE_LOCAL_ID",
        ] {
            assert!(!debug.contains(forbidden));
        }
    }
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.page_size, 20);
}

#[test]
fn classification_override_management_is_local_and_typed() {
    let remove = ClientMessage::RemoveClassificationOverride(RemoveClassificationOverride {
        stable_id: "abs_safe".into(),
    });
    let reset = ClientMessage::ResetClassificationOverrides(ResetClassificationOverrides {});

    assert_eq!(
        serde_json::to_value(&remove).unwrap(),
        json!({
            "type": "remove_classification_override",
            "payload": { "stable_id": "abs_safe" }
        })
    );
    assert_eq!(
        serde_json::to_value(&reset).unwrap(),
        json!({"type": "reset_classification_overrides", "payload": {}})
    );
    assert_round_trip(remove);
    assert_round_trip(reset);
}

#[test]
fn work_block_contract_round_trips_and_redacts_intention_from_debug() {
    let sentinel = "PRIVATE_INTENTION_SENTINEL";
    let start = ClientMessage::StartWorkBlock(StartWorkBlock {
        intention: Some(sentinel.into()),
        planned_duration_seconds: 1_500,
        purpose: Some(WorkBlockPurpose::DeepWork),
        intensity: WorkBlockIntensity::Medium,
    });
    assert_round_trip(start.clone());
    assert!(!format!("{start:?}").contains(sentinel));

    let snapshot = WorkBlockSnapshot {
        state_version: WORK_BLOCK_STATE_VERSION,
        phase: WorkBlockPhase::Active,
        block_id: Some(event_id()),
        intention: Some(sentinel.into()),
        purpose: Some(WorkBlockPurpose::DeepWork),
        intensity: Some(WorkBlockIntensity::Medium),
        planned_duration_seconds: 1_500,
        elapsed_duration_seconds: 60,
        remaining_duration_seconds: 1_440,
        started_at: Some(timestamp()),
        analysis_ended_at: None,
        ends_at: Some(timestamp() + chrono::Duration::seconds(1_500)),
        paused_at: None,
        recovered_after_restart: false,
        current_category: Some("FOCUS_WORK".into()),
        classification_status: ClassificationStatus::Classified,
        confidence: ClassificationConfidence::High,
        status_line: "Current category: Focus work.".into(),
        result: None,
    };
    assert_round_trip(ServerMessage::WorkBlockState(snapshot.clone()));
    assert!(!format!("{snapshot:?}").contains(sentinel));
}

#[test]
fn safe_work_block_result_has_one_action_and_no_intention_field() {
    let result = WorkBlockResult {
        planned_duration_seconds: 1_500,
        elapsed_duration_seconds: 1_500,
        longest_uninterrupted_seconds: 900,
        switch_away_count: 2,
        recovery_count: 1,
        confidence: ConfidenceLevel::High,
        coverage: WorkBlockCoverage::Good,
        coverage_ratio: 0.9,
        safe_evidence_category: Some("FOCUS_WORK".into()),
        observation: "Velvt observed two category changes.".into(),
        next_action: WorkBlockNextAction {
            action_id: "protect_next_10".into(),
            label: "Protect the next 10 minutes.".into(),
            duration_seconds: 600,
        },
    };
    let value = serde_json::to_value(&result).unwrap();
    assert!(value.get("next_action").unwrap().is_object());
    assert!(value.get("next_actions").is_none());
    assert!(value.get("intention").is_none());
    assert_round_trip(result);
}

#[test]
fn queued_event_quality_metadata_round_trips_without_raw_fields() {
    let event = QueuedEventSummary {
        event_id: event_id(),
        stable_id: "abs_safe".into(),
        label: "reference:browser".into(),
        local_label: Some("Browser".into()),
        category: "REFERENCE".into(),
        classification_tier: "fallback".into(),
        classification_status: ClassificationStatus::Ambiguous,
        classification_confidence: ClassificationConfidence::Low,
        classification_source: ClassificationSource::Fallback,
        occurred_at: timestamp(),
    };

    let encoded = serde_json::to_value(&event).unwrap();
    for forbidden in ["app_name", "window_title", "bundle_id", "url", "key_hash"] {
        assert!(encoded.get(forbidden).is_none(), "{forbidden}");
    }
    assert_round_trip(event);
}

#[test]
fn server_message_variants_round_trip() {
    let messages = [
        ServerMessage::ServerHello(ServerHello {
            protocol_version: PROTOCOL_VERSION,
        }),
        ServerMessage::Acknowledged(Acknowledged),
        ServerMessage::VersionMismatch(VersionMismatch {
            server_protocol_version: PROTOCOL_VERSION,
            client_protocol_version: 99,
        }),
        ServerMessage::MalformedMessage(MalformedMessage {
            code: MalformedMessageCode::InvalidMessage,
        }),
        ServerMessage::RawEventAck(RawEventAck {
            event_id: event_id(),
            status: RawEventStatus::Accepted,
            drop_reason: None,
        }),
        ServerMessage::InsightPayload(InsightPayload {
            date: NaiveDate::from_ymd_opt(2026, 6, 13).unwrap(),
            text: "ready-to-display".into(),
            evidence: Default::default(),
            confidence_level: ConfidenceLevel::High,
            low_confidence: false,
            generated_at: timestamp(),
        }),
        ServerMessage::HistoryPayload(HistoryPayload {
            days: 0,
            summaries: Vec::new(),
        }),
        ServerMessage::ServiceStatus(ServiceStatus {
            state: ServiceState::Ready,
            reason: None,
        }),
        ServerMessage::PrivacyViolationAlert(PrivacyViolationAlert {
            code: "raw_field_rejected".into(),
            message: "safe rejection".into(),
        }),
        ServerMessage::ErrorResponse(ErrorResponse {
            code: "server_error".into(),
            message: "safe".into(),
            related_event_id: None,
        }),
    ];

    for message in messages {
        assert_round_trip(message);
    }
}

#[test]
fn messages_use_tagged_payload_envelope() {
    let value = serde_json::to_value(ServerMessage::ServerHello(ServerHello {
        protocol_version: PROTOCOL_VERSION,
    }))
    .unwrap();

    assert_eq!(
        value,
        json!({"type": "server_hello", "payload": {"protocol_version": PROTOCOL_VERSION}})
    );
}

#[test]
fn payloads_reject_unknown_fields() {
    let value = json!({
        "type": "client_hello",
        "payload": {
            "expected_protocol_version": 2,
            "client_version": "0.1.0",
            "unexpected": "rejected"
        }
    });

    assert!(serde_json::from_value::<ClientMessage>(value).is_err());
}
