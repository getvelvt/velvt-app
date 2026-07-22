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
    pub classification_tier: String,
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
            classification_tier: &'a str,
            payload: ApiEventPayload<'a>,
        }

        ApiBatchEvent {
            event_id: &self.event_id,
            occurred_at: self.occurred_at,
            // Local labels may be specific enough to make the UI useful. The
            // cloud boundary deliberately collapses them to a category-scoped
            // vocabulary so an application name can never be inferred from
            // the uploaded abstraction type.
            abstraction_type: cloud_abstraction_type(&self.category),
            abstraction_type_version: API_ABSTRACTION_TYPE_VERSION,
            classification_tier: &self.classification_tier,
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
            classification_tier: event.classification_tier().as_str().to_owned(),
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
        _supported_abstraction_types: Vec<String>,
        category_taxonomy_version: impl Into<String>,
        events: Vec<BatchEventPayload>,
    ) -> Self {
        let mut supported_abstraction_types = Vec::new();
        for event in &events {
            let safe_type = cloud_abstraction_type(&event.category).to_owned();
            if !supported_abstraction_types.contains(&safe_type) {
                supported_abstraction_types.push(safe_type);
            }
        }
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

fn cloud_abstraction_type(category: &str) -> &'static str {
    match category {
        "FOCUS_WORK" => "document:inferred",
        "PASSIVE_CONSUMPTION" => "video:inferred",
        "SOCIAL_FEED" => "social:inferred",
        "COMMUNICATION" => "communication:inferred",
        "TASK_MANAGEMENT" => "task:inferred",
        "REFERENCE" => "reference:inferred",
        "SYSTEM" => "system:inferred",
        "UNLOGGED" => "unlogged",
        _ => "system:unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchEventPayload, BatchPayload};
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    #[test]
    fn batch_event_collapses_local_label_at_cloud_boundary() {
        let event = BatchEventPayload {
            event_id: "event-1".into(),
            stable_id: "stable-1".into(),
            label: "video:youtube".into(),
            category: "PASSIVE_CONSUMPTION".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            duration_seconds: 120,
        };

        let value = serde_json::to_value(event).unwrap();

        assert_eq!(
            value,
            json!({
                "event_id": "event-1",
                "occurred_at": "2027-01-15T08:00:00Z",
                "abstraction_type": "video:inferred",
                "abstraction_type_version": "1",
                "classification_tier": "exact_match",
                "payload": {
                    "duration_seconds": 120,
                    "category": "PASSIVE_CONSUMPTION"
                }
            })
        );
        assert!(!value.to_string().contains("youtube"));
    }

    #[test]
    fn batch_supported_types_are_derived_from_safe_cloud_labels() {
        let event = BatchEventPayload {
            event_id: "event-1".into(),
            stable_id: "stable-1".into(),
            label: "communication:slack".into(),
            category: "COMMUNICATION".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            duration_seconds: 120,
        };

        let batch = BatchPayload::new(
            "batch-1",
            "1",
            "1.0.0",
            vec!["communication:slack".into()],
            "mvp-1",
            vec![event],
        );
        let value = serde_json::to_value(batch).unwrap();

        assert_eq!(
            value["supported_abstraction_types"],
            json!(["communication:inferred"])
        );
        assert!(!value.to_string().contains("slack"));
    }
}
