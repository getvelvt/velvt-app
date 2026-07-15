use super::{BatchEventPayload, BatchPayload};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub struct BatchAssembler {
    device_id: String,
    count_threshold: usize,
    age_threshold: Duration,
    opened_at: Option<DateTime<Utc>>,
    events: Vec<BatchEventPayload>,
}

impl BatchAssembler {
    pub fn from_config(
        device_id: impl Into<String>,
        config: &crate::config::ServiceConfig,
    ) -> Self {
        Self::new(
            device_id,
            config.upload_batch_event_limit,
            config.upload_flush_interval,
        )
    }

    pub fn new(
        device_id: impl Into<String>,
        count_threshold: usize,
        age_threshold: Duration,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            count_threshold,
            age_threshold,
            opened_at: None,
            events: Vec::new(),
        }
    }

    pub fn push(&mut self, event: BatchEventPayload, now: DateTime<Utc>) -> Option<BatchPayload> {
        self.opened_at.get_or_insert(now);
        self.events.push(event);
        (self.events.len() >= self.count_threshold)
            .then(|| self.take_batch())
            .flatten()
    }

    pub fn flush_due(&mut self, now: DateTime<Utc>) -> Option<BatchPayload> {
        let opened_at = self.opened_at?;
        let elapsed = now.signed_duration_since(opened_at).num_seconds();
        (elapsed >= self.age_threshold.as_secs() as i64)
            .then(|| self.take_batch())
            .flatten()
    }

    pub fn flush_shutdown(&mut self) -> Option<BatchPayload> {
        self.take_batch()
    }

    pub fn flush_sleep(&mut self) -> Option<BatchPayload> {
        self.take_batch()
    }

    pub fn requeue(&mut self, mut batch: BatchPayload) {
        if batch.events.is_empty() {
            return;
        }
        let reopened_at = batch.events.iter().map(|event| event.occurred_at).min();
        batch.events.append(&mut self.events);
        self.events = batch.events;
        self.opened_at = match (self.opened_at, reopened_at) {
            (Some(current), Some(reopened)) => Some(current.min(reopened)),
            (current, reopened) => current.or(reopened),
        };
    }

    fn take_batch(&mut self) -> Option<BatchPayload> {
        if self.events.is_empty() {
            return None;
        }
        let events = std::mem::take(&mut self.events);
        self.opened_at = None;
        let batch_id = deterministic_batch_id(&self.device_id, &events);
        let taxonomy = events
            .first()
            .map(|event| event.taxonomy_version.clone())
            .unwrap_or_default();
        let mut supported_abstraction_types = Vec::new();
        for event in &events {
            if !supported_abstraction_types.contains(&event.label) {
                supported_abstraction_types.push(event.label.clone());
            }
        }
        Some(BatchPayload::new(
            batch_id,
            "1",
            env!("CARGO_PKG_VERSION"),
            supported_abstraction_types,
            taxonomy,
            events,
        ))
    }
}

fn deterministic_batch_id(device_id: &str, events: &[BatchEventPayload]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"velvt:upload-batch:v1");
    hasher.update((device_id.len() as u64).to_be_bytes());
    hasher.update(device_id.as_bytes());
    for event in events {
        hasher.update((event.event_id.len() as u64).to_be_bytes());
        hasher.update(event.event_id.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{BatchAssembler, BatchEventPayload};
    use chrono::{TimeZone, Utc};
    use std::time::Duration;

    fn event(event_id: &str, label: &str) -> BatchEventPayload {
        BatchEventPayload {
            event_id: event_id.into(),
            stable_id: format!("stable-{event_id}"),
            label: label.into(),
            category: "FOCUS_WORK".into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            duration_seconds: 60,
        }
    }

    #[test]
    fn batch_supported_abstraction_types_are_unique_event_labels() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 3, Duration::from_secs(60));

        assert!(assembler
            .push(event("event-1", "document:docs"), now)
            .is_none());
        assert!(assembler
            .push(event("event-2", "video:youtube"), now)
            .is_none());
        let batch = assembler
            .push(event("event-3", "document:docs"), now)
            .expect("third event should flush the batch");

        assert_eq!(
            batch.supported_abstraction_types,
            vec!["document:docs".to_owned(), "video:youtube".to_owned()]
        );
    }
}
