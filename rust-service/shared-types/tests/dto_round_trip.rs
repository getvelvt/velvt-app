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
