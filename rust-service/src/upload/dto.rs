use chrono::{DateTime, Utc};
use serde::{Serialize, Serializer};

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
            abstraction_type: &'static str,
            abstraction_type_version: &'static str,
            payload: ApiEventPayload<'a>,
        }

        ApiBatchEvent {
            event_id: &self.event_id,
            occurred_at: self.occurred_at,
            abstraction_type: "document:edit",
            abstraction_type_version: "1",
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
