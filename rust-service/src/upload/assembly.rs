use super::{BatchEventPayload, BatchPayload};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub struct BatchAssembler {
    device_id: String,
    count_threshold: usize,
    age_threshold: Duration,
    opened_at: Option<DateTime<Utc>>,
    events: Vec<Buffered>,
}

/// One buffered event and the account it was recorded under.
struct Buffered {
    owner_user_id: Option<String>,
    event: BatchEventPayload,
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

    /// Buffers an event recorded with no known owner. See [`Self::push_owned`].
    pub fn push(&mut self, event: BatchEventPayload, now: DateTime<Utc>) -> Option<BatchPayload> {
        self.push_owned(event, None, now)
    }

    /// Buffers `event`, recorded while `owner_user_id` was signed in, and
    /// returns a batch when one closes.
    pub fn push_owned(
        &mut self,
        event: BatchEventPayload,
        owner_user_id: Option<String>,
        now: DateTime<Utc>,
    ) -> Option<BatchPayload> {
        // A taxonomy upgrade closes the open batch. `category_taxonomy_version`
        // is one field for the whole batch, so a batch straddling the upgrade
        // would label some of its events with a version they were not
        // classified under. Carrying the version per event is not the
        // alternative: the per-event DTO is frozen and nothing new crosses the
        // wire, so grouping here is the only honest option.
        //
        // A change of account closes it for the same reason: a batch is sent
        // under one account's token, so it may hold only that account's
        // events.
        if self.events.first().is_some_and(|open| {
            open.event.taxonomy_version != event.taxonomy_version
                || open.owner_user_id != owner_user_id
        }) {
            let closed = self.take_batch();
            // `get_or_insert`, not an assignment: `take_batch` clears
            // `opened_at` only when it drained the buffer, and a remainder keeps
            // the age it already had rather than being handed a fresh window.
            self.opened_at.get_or_insert(now);
            self.events.push(Buffered {
                owner_user_id,
                event,
            });
            // Non-`None`: the buffer held at least the event we just compared
            // against. The new event stays open and leaves on the next push or
            // flush, which is the same treatment any single buffered event gets.
            return closed;
        }
        self.opened_at.get_or_insert(now);
        self.events.push(Buffered {
            owner_user_id,
            event,
        });
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

    pub fn requeue(&mut self, batch: BatchPayload) {
        if batch.events.is_empty() {
            return;
        }
        let reopened_at = batch.events.iter().map(|event| event.occurred_at).min();
        let owner_user_id = batch.owner_user_id;
        let mut events = batch
            .events
            .into_iter()
            .map(|event| Buffered {
                owner_user_id: owner_user_id.clone(),
                event,
            })
            .collect::<Vec<_>>();
        events.append(&mut self.events);
        self.events = events;
        self.opened_at = match (self.opened_at, reopened_at) {
            (Some(current), Some(reopened)) => Some(current.min(reopened)),
            (current, reopened) => current.or(reopened),
        };
    }

    /// Drops every buffered event and says how many there were.
    ///
    /// For an account that was deleted: what it recorded and has not yet
    /// queued is sent nowhere.
    pub fn discard_all(&mut self) -> usize {
        let discarded = self.events.len();
        self.events.clear();
        self.opened_at = None;
        discarded
    }

    /// Drains the buffer into one batch, or — when the buffer holds more than
    /// one taxonomy version or account — into the batch for the oldest
    /// event's version and account, leaving the rest buffered.
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
        let first = self.events.first()?;
        let taxonomy = first.event.taxonomy_version.clone();
        let owner_user_id = first.owner_user_id.clone();
        let (events, remainder): (Vec<_>, Vec<_>) = std::mem::take(&mut self.events)
            .into_iter()
            .partition(|buffered| {
                buffered.event.taxonomy_version == taxonomy
                    && buffered.owner_user_id == owner_user_id
            });
        if remainder.is_empty() {
            self.opened_at = None;
        }
        self.events = remainder;
        let events = events
            .into_iter()
            .map(|buffered| buffered.event)
            .collect::<Vec<_>>();
        let batch_id = deterministic_batch_id(&self.device_id, &events);
        let mut supported_abstraction_types = Vec::new();
        for event in &events {
            if !supported_abstraction_types.contains(&event.label) {
                supported_abstraction_types.push(event.label.clone());
            }
        }
        Some(
            BatchPayload::new(
                batch_id,
                "1",
                crate::build_info::SERVICE_VERSION,
                supported_abstraction_types,
                taxonomy,
                events,
            )
            .owned_by(owner_user_id),
        )
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

    /// The backend stores `client_version` on every batch row. Cargo's fixed
    /// 1.0.0 made every release since 1.0.0 look the same there.
    #[test]
    fn batches_carry_the_release_version() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 8, Duration::from_secs(60));
        assembler.push(event("event-1", "document:docs", "FOCUS_WORK"), now);

        let batch = assembler
            .flush_shutdown()
            .expect("one buffered event makes a batch");

        assert_eq!(batch.client_version, crate::build_info::SERVICE_VERSION);
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

    /// A batch is sent under one account's token, so a sign-in to another
    /// account closes the open batch rather than adding to it.
    #[test]
    fn a_change_of_account_closes_the_open_batch() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 8, Duration::from_secs(60));

        assert!(assembler
            .push_owned(
                event("event-a", "document:docs", "FOCUS_WORK"),
                Some("account-a".into()),
                now,
            )
            .is_none());
        let closed = assembler
            .push_owned(
                event("event-b", "document:docs", "FOCUS_WORK"),
                Some("account-b".into()),
                now,
            )
            .expect("another account's event closes the batch below the count threshold");

        assert_eq!(closed.owner_user_id.as_deref(), Some("account-a"));
        assert_eq!(closed.events.len(), 1);
        assert_eq!(closed.events[0].event_id, "event-a");
        let open = assembler.flush_shutdown().expect("B's event is still open");
        assert_eq!(open.owner_user_id.as_deref(), Some("account-b"));
        assert_eq!(open.events[0].event_id, "event-b");
    }

    /// A batch that failed to persist goes back under the account it was
    /// recorded for, even when another account's events arrived since.
    #[test]
    fn a_requeued_batch_keeps_its_owner() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 8, Duration::from_secs(60));

        assembler.push_owned(
            event("event-a", "document:docs", "FOCUS_WORK"),
            Some("account-a".into()),
            now,
        );
        let failed = assembler.flush_shutdown().expect("A's batch");
        assembler.push_owned(
            event("event-b", "document:docs", "FOCUS_WORK"),
            Some("account-b".into()),
            now,
        );
        assembler.requeue(failed);

        let first = assembler.flush_shutdown().expect("A's events first");
        assert_eq!(first.owner_user_id.as_deref(), Some("account-a"));
        assert_eq!(first.events.len(), 1);
        let second = assembler.flush_shutdown().expect("then B's");
        assert_eq!(second.owner_user_id.as_deref(), Some("account-b"));
        assert_eq!(second.events.len(), 1);
        assert!(assembler.flush_shutdown().is_none());
    }

    #[test]
    fn discard_all_empties_the_buffer() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let mut assembler = BatchAssembler::new("device-1", 8, Duration::from_secs(60));
        assembler.push_owned(
            event("event-a", "document:docs", "FOCUS_WORK"),
            Some("account-a".into()),
            now,
        );

        assert_eq!(assembler.discard_all(), 1);
        assert!(assembler.flush_shutdown().is_none());
        assert!(assembler
            .flush_due(now + chrono::Duration::hours(1))
            .is_none());
    }
}
