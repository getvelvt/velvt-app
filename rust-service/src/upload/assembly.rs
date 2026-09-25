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
        // A taxonomy upgrade closes the open batch. `category_taxonomy_version`
        // is one field for the whole batch, so a batch straddling the upgrade
        // would label some of its events with a version they were not
        // classified under. Carrying the version per event is not the
        // alternative: the per-event DTO is frozen and nothing new crosses the
        // wire, so grouping here is the only honest option.
        if self
            .events
            .first()
            .is_some_and(|open| open.taxonomy_version != event.taxonomy_version)
        {
            let closed = self.take_batch();
            // `get_or_insert`, not an assignment: `take_batch` clears
            // `opened_at` only when it drained the buffer, and a remainder keeps
            // the age it already had rather than being handed a fresh window.
            self.opened_at.get_or_insert(now);
            self.events.push(event);
            // Non-`None`: the buffer held at least the event we just compared
            // against. The new event stays open and leaves on the next push or
            // flush, which is the same treatment any single buffered event gets.
            return closed;
        }
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

    /// Drains the buffer into one batch, or — when the buffer holds more than
    /// one taxonomy version — into the batch for the oldest event's version,
    /// leaving the rest buffered.
    ///
    /// `push` closes a batch at a version boundary, so the buffer is normally
    /// homogeneous already and this splits nothing. `requeue` is the path that
    /// can mix versions again: a batch that failed to persist is prepended to
    /// whatever has arrived since, and across an upgrade that is two versions in
    /// one buffer. Splitting here rather than trusting the caller means the
    /// batch's `category_taxonomy_version` describes every event in it no matter
    /// how the buffer was filled.
    ///
    /// A remainder keeps the original `opened_at`, so it is already past the age
    /// threshold and leaves on the next `flush_due` rather than waiting for a
    /// fresh window. On the drain-everything paths (`flush_sleep`,
    /// `flush_shutdown`) a remainder stays unbatched, which is the state
    /// `recover_unbatched` exists for: retention spares unbatched eligible rows
    /// and the next start re-ingests them.
    fn take_batch(&mut self) -> Option<BatchPayload> {
        let taxonomy = self.events.first()?.taxonomy_version.clone();
        let (events, remainder): (Vec<_>, Vec<_>) = std::mem::take(&mut self.events)
            .into_iter()
            .partition(|event| event.taxonomy_version == taxonomy);
        if remainder.is_empty() {
            self.opened_at = None;
        }
        self.events = remainder;
        let batch_id = deterministic_batch_id(&self.device_id, &events);
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

    fn event(event_id: &str, label: &str, category: &str) -> BatchEventPayload {
        BatchEventPayload {
            event_id: event_id.into(),
            stable_id: format!("stable-{event_id}"),
            label: label.into(),
            category: category.into(),
            taxonomy_version: "mvp-1".into(),
            classification_tier: "exact_match".into(),
            occurred_at: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            duration_seconds: 60,
        }
    }

    #[test]
    fn batch_supported_abstraction_types_are_unique_cloud_labels() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 3, Duration::from_secs(60));

        assert!(assembler
            .push(event("event-1", "document:docs", "FOCUS_WORK"), now)
            .is_none());
        assert!(assembler
            .push(
                event("event-2", "video:youtube", "PASSIVE_CONSUMPTION"),
                now,
            )
            .is_none());
        let batch = assembler
            .push(event("event-3", "document:docs", "FOCUS_WORK"), now)
            .expect("third event should flush the batch");

        assert_eq!(
            batch.supported_abstraction_types,
            vec!["document:inferred".to_owned(), "video:inferred".to_owned()]
        );
    }

    /// A batch straddling a taxonomy upgrade would label one version's events
    /// with the other version's name, and nothing downstream could tell.
    #[test]
    fn a_taxonomy_upgrade_closes_the_open_batch() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 8, Duration::from_secs(60));

        assert!(assembler
            .push(event("event-1", "document:docs", "FOCUS_WORK"), now)
            .is_none());
        let mut upgraded = event("event-2", "document:docs", "FOCUS_WORK");
        upgraded.taxonomy_version = "mvp-2".into();

        let closed = assembler
            .push(upgraded, now)
            .expect("the version change closes the batch below the count threshold");
        assert_eq!(closed.category_taxonomy_version, "mvp-1");
        assert_eq!(closed.events.len(), 1);

        let remaining = assembler
            .flush_shutdown()
            .expect("the upgraded event is still buffered");
        assert_eq!(remaining.category_taxonomy_version, "mvp-2");
        assert_eq!(remaining.events.len(), 1);
    }

    /// `requeue` can mix versions even when `push` never does, so the split has
    /// to hold at the point the batch is minted too.
    #[test]
    fn a_requeued_batch_does_not_relabel_events_of_another_version() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 8, Duration::from_secs(60));

        assembler.push(event("event-1", "document:docs", "FOCUS_WORK"), now);
        let failed = assembler
            .flush_shutdown()
            .expect("one buffered event makes a batch");

        let mut upgraded = event("event-2", "document:docs", "FOCUS_WORK");
        upgraded.taxonomy_version = "mvp-2".into();
        assembler.push(upgraded, now);
        assembler.requeue(failed);

        let first = assembler.flush_shutdown().expect("the older version first");
        assert_eq!(first.category_taxonomy_version, "mvp-1");
        assert_eq!(first.events.len(), 1);
        let second = assembler
            .flush_shutdown()
            .expect("the remainder is still buffered, not discarded");
        assert_eq!(second.category_taxonomy_version, "mvp-2");
        assert_eq!(second.events.len(), 1);
    }
}
