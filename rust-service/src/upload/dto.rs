use chrono::{DateTime, Utc};
use serde::{Serialize, Serializer};

pub const API_ABSTRACTION_TYPE_VERSION: &str = "1";

/// Auditable outbound event DTO. It deliberately has no raw-content fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEventPayload {
    pub event_id: String,
    pub stable_id: String,
    pub label: String,
    pub category: String,
    pub taxonomy_version: String,
    pub occurred_at: DateTime<Utc>,
    pub duration_seconds: u64,
}

impl Serialize for BatchEventPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct ApiEventPayload<'a> {
            duration_seconds: u64,
            category: &'a str,
        }

        #[derive(Serialize)]
        struct ApiBatchEvent<'a> {
            event_id: &'a str,
            occurred_at: DateTime<Utc>,
            abstraction_type: &'a str,
            abstraction_type_version: &'a str,
            payload: ApiEventPayload<'a>,
        }

        ApiBatchEvent {
            event_id: &self.event_id,
            occurred_at: self.occurred_at,
            abstraction_type: &self.label,
            abstraction_type_version: API_ABSTRACTION_TYPE_VERSION,
            payload: ApiEventPayload {
                duration_seconds: self.duration_seconds,
                category: &self.category,
            },
        }
        .serialize(serializer)
    }
}

impl BatchEventPayload {
    pub fn from_abstracted(
        event_id: impl Into<String>,
        event: &crate::abstraction::AbstractedEvent,
        duration_seconds: u64,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            stable_id: event.stable_id().to_owned(),
            label: event.label().to_owned(),
            category: event.category().to_owned(),
            taxonomy_version: event.taxonomy_version().to_owned(),
            occurred_at: event.occurred_at(),
            duration_seconds,
        }
    }
}

/// Exact privacy-safe body sent to `POST /v1/events/batches`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatchPayload {
    pub batch_id: String,
    pub schema_version: String,
    pub client_version: String,
    pub supported_abstraction_types: Vec<String>,
    pub category_taxonomy_version: String,
    pub events: Vec<BatchEventPayload>,
}

impl BatchPayload {
    pub fn new(
        batch_id: impl Into<String>,
        schema_version: impl Into<String>,
        client_version: impl Into<String>,
        supported_abstraction_types: Vec<String>,
        category_taxonomy_version: impl Into<String>,
        events: Vec<BatchEventPayload>,
    ) -> Self {
        Self {
            batch_id: batch_id.into(),
            schema_version: schema_version.into(),
            client_version: client_version.into(),
            supported_abstraction_types,
            category_taxonomy_version: category_taxonomy_version.into(),
            events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BatchEventPayload;
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    #[test]
    fn batch_event_serializes_safe_label_as_abstraction_type() {
        let event = BatchEventPayload {
            event_id: "event-1".into(),
            stable_id: "stable-1".into(),
            label: "video:youtube".into(),
            category: "PASSIVE_CONSUMPTION".into(),
            taxonomy_version: "mvp-1".into(),
            occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            duration_seconds: 120,
        };

        let value = serde_json::to_value(event).unwrap();

        assert_eq!(
            value,
            json!({
                "event_id": "event-1",
                "occurred_at": "2027-01-15T08:00:00Z",
                "abstraction_type": "video:youtube",
                "abstraction_type_version": "1",
                "payload": {
                    "duration_seconds": 120,
                    "category": "PASSIVE_CONSUMPTION"
                }
            })
        );
    }
}
