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

/// Hand-written, and deliberately not derived.
///
/// The struct above holds more than the cloud is given — a local label, a stable
/// id, a taxonomy version — and a derive would send all of it the moment someone
/// added a field for a local purpose. Writing the outbound shape out by hand
/// makes the wire a decision rather than a consequence: a new field is invisible
/// here until a person types it into `ApiBatchEvent`.
///
/// That is what keeps the device-local facts device-local. The bundle
/// identifier, the declared `LSApplicationCategoryType` and the declared document
/// types are recorded beside the event on disk (migration 0033) and never reach a
/// DTO; there is no field here they could occupy. Two tests hold that shut:
/// `serialized_batch_holds_exactly_the_documented_keys` below closes this key set,
/// and `published_claims::no_declared_fact_reaches_an_upload_payload` drives a
/// real event carrying all three through the router and asserts their *values*
/// appear nowhere in the batch this device would have POSTed.
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
    use std::collections::BTreeSet;

    /// Every key the upload body is allowed to carry, as a dotted path with
    /// array elements flattened onto their field.
    ///
    /// Invariant 1 of the Classification v2 implementation contract fixes this
    /// list: `event_id, occurred_at, abstraction_type, abstraction_type_version,
    /// classification_tier, payload{duration_seconds, category}`, inside the
    /// batch envelope the API requires. Adding a line here is the deliberate act
    /// of deciding a new fact may leave the device, and it is the only place that
    /// decision can be made quietly enough to matter.
    const DOCUMENTED_WIRE_KEYS: &[&str] = &[
        "batch_id",
        "category_taxonomy_version",
        "client_version",
        "events",
        "events.abstraction_type",
        "events.abstraction_type_version",
        "events.classification_tier",
        "events.event_id",
        "events.occurred_at",
        "events.payload",
        "events.payload.category",
        "events.payload.duration_seconds",
        "schema_version",
        "supported_abstraction_types",
    ];

    fn collect_key_paths(value: &serde_json::Value, prefix: &str, found: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, child) in fields {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    found.insert(path.clone());
                    collect_key_paths(child, &path, found);
                }
            }
            // An array element is the same shape repeated, so its keys belong to
            // the field, not to an index: `events[0].event_id` and
            // `events[1].event_id` are one key on the wire.
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_key_paths(item, prefix, found);
                }
            }
            _ => {}
        }
    }

    fn sample_event(label: &str, category: &str) -> BatchEventPayload {
        BatchEventPayload {
            event_id: "event-1".into(),
            stable_id: "stable-1".into(),
            label: label.into(),
            category: category.into(),
            taxonomy_version: "mvp-2".into(),
            classification_tier: "exact_match".into(),
            occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            duration_seconds: 120,
        }
    }

    /// The serialized batch holds exactly the documented keys, and no others.
    ///
    /// The other tests in this file assert one whole value each, which catches a
    /// field added to `ApiBatchEvent` and nothing else. This one walks the
    /// serialized tree, so a field added to the envelope, to the event, or to the
    /// nested payload all fail it, and it fails in the file that would have to
    /// allow the field.
    ///
    /// It is a check on key names, and therefore only half the guard. A new field
    /// fails it whatever it is called, but a device-local value folded into a
    /// field that already exists -- a bundle digest appended to
    /// `classification_tier`, say -- adds no key and passes. That half is
    /// `published_claims::no_declared_fact_reaches_an_upload_payload`, which
    /// asserts against the values.
    #[test]
    fn serialized_batch_holds_exactly_the_documented_keys() {
        let batch = BatchPayload::new(
            "batch-1",
            "1",
            "1.0.0",
            Vec::new(),
            "mvp-2",
            vec![
                sample_event("document:code", "FOCUS_WORK"),
                sample_event("communication:slack", "COMMUNICATION"),
            ],
        );

        let value = serde_json::to_value(&batch).unwrap();
        let mut found = BTreeSet::new();
        collect_key_paths(&value, "", &mut found);

        let documented: BTreeSet<String> = DOCUMENTED_WIRE_KEYS
            .iter()
            .map(|key| (*key).to_owned())
            .collect();
        assert_eq!(
            found, documented,
            "the upload body's key set changed. A key on the left and not the right \
             is a new fact crossing the wire -- Invariant 1 of the Classification v2 \
             contract admits none, and the bundle identifier, the declared category \
             and the declared document types are device-local by that rule. A key on \
             the right and not the left is a field that stopped being sent, which the \
             API contract has to agree to"
        );
    }

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
