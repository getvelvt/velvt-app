//! The needs-a-category list (protocol 33).
//!
//! One list of the applications and browser sites Velvt could not
//! categorize, ranked together, which the Settings list shows and answers
//! entry by entry.

use velvt_shared_types::{TriageEntryKind, UnclassifiedTriageEntry};

use crate::persistence::{PersistenceError, RawEventRepo, TRIAGE_MAX_ENTRIES, TRIAGE_MIN_SECONDS};

/// The needs-a-category list: the applications and the browser sites Velvt
/// could not categorize in the last `lookback_days`, ranked together.
///
/// Each half is bounded by its own query exactly as before (at least
/// [`TRIAGE_MIN_SECONDS`] observed, at most [`TRIAGE_MAX_ENTRIES`], the window
/// clamped to the raw-event horizon). The merge ranks by observed time,
/// longest first, then applications ahead of sites, then by key, and keeps the
/// first [`TRIAGE_MAX_ENTRIES`]: a list of eight is a task, and a list of
/// sixteen is an inventory. The top eight of the union are always inside the
/// top eight of each half, so the per-half caps lose nothing.
///
/// An application Velvt holds no local name for keeps its row, with no name,
/// so the client can say "Unnamed application" without that placeholder ever
/// coming back as the name of a rule.
pub fn needs_a_category(
    raw_events: &dyn RawEventRepo,
    lookback_days: u32,
) -> Result<Vec<UnclassifiedTriageEntry>, PersistenceError> {
    let applications =
        raw_events.unclassified_triage(lookback_days, TRIAGE_MIN_SECONDS, TRIAGE_MAX_ENTRIES)?;
    let sites = raw_events.unclassified_site_triage(
        lookback_days,
        TRIAGE_MIN_SECONDS,
        TRIAGE_MAX_ENTRIES,
    )?;
    let mut entries: Vec<UnclassifiedTriageEntry> = applications
        .into_iter()
        .map(|application| UnclassifiedTriageEntry {
            kind: TriageEntryKind::Application,
            stable_id: application.app_stable_id,
            display_name: application.display_name,
            seconds_observed: application.seconds_observed,
            event_count: application.event_count,
        })
        .chain(sites.into_iter().map(|site| UnclassifiedTriageEntry {
            kind: TriageEntryKind::Site,
            stable_id: site.site_stable_id,
            display_name: Some(site.display_name),
            seconds_observed: site.seconds_observed,
            event_count: site.event_count,
        }))
        .collect();
    entries.sort_by(|left, right| {
        right
            .seconds_observed
            .cmp(&left.seconds_observed)
            .then(left.kind.cmp(&right.kind))
            .then_with(|| left.stable_id.cmp(&right.stable_id))
    });
    entries.truncate(TRIAGE_MAX_ENTRIES);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{RawEventEntry, SqlitePersistence};
    use chrono::{Duration, Utc};

    fn unclassified_event(
        id: &str,
        seconds: u64,
        app_key: Option<String>,
        site_key: Option<String>,
    ) -> RawEventEntry {
        RawEventEntry {
            event_id: id.to_owned(),
            stable_id: format!("abs_{id}"),
            label: "unlogged".into(),
            local_display_label: None,
            local_name_suggestion: app_key.as_ref().map(|_| format!("Qwybex {id}")),
            category: "UNLOGGED".into(),
            taxonomy_version: "mvp-2".into(),
            classification_tier: "fallback".into(),
            classification_status: "unclassified".into(),
            classification_confidence: "none".into(),
            classification_source: "fallback".into(),
            occurred_at: Utc::now() - Duration::hours(1),
            duration_seconds: seconds,
            upload_eligible: false,
            app_scope_eligible: site_key.is_none(),
            app_stable_id: app_key,
            site_stable_id: site_key,
        }
    }

    /// Applications and sites share one list: longest first, applications
    /// ahead of sites on a tie, then by key, eight at most, and a site's name
    /// is its host.
    #[test]
    fn the_list_ranks_applications_and_sites_together_and_keeps_eight() {
        let db = SqlitePersistence::open_in_memory().unwrap();
        let events = db.raw_event_repo();
        for index in 0..6_u8 {
            let seconds = 600 + u64::from(index) * 60;
            let app = unclassified_event(
                &format!("app-{index}"),
                seconds,
                Some(format!("{:02x}", 0x10 + index).repeat(32)),
                None,
            );
            events.insert(&app).unwrap();
            let visit = unclassified_event(
                &format!("site-{index}"),
                seconds,
                Some("ee".repeat(32)),
                Some(format!("{:02x}", 0x20 + index).repeat(32)),
            );
            events.insert(&visit).unwrap();
            assert!(events
                .record_local_site_name(
                    &visit.event_id,
                    &format!("site-{index}.example.org"),
                    Utc::now()
                )
                .unwrap());
        }

        let list = needs_a_category(&*events, 7).unwrap();

        assert_eq!(list.len(), TRIAGE_MAX_ENTRIES);
        let order: Vec<(TriageEntryKind, u64)> = list
            .iter()
            .map(|entry| (entry.kind, entry.seconds_observed))
            .collect();
        assert_eq!(
            order,
            vec![
                (TriageEntryKind::Application, 900),
                (TriageEntryKind::Site, 900),
                (TriageEntryKind::Application, 840),
                (TriageEntryKind::Site, 840),
                (TriageEntryKind::Application, 780),
                (TriageEntryKind::Site, 780),
                (TriageEntryKind::Application, 720),
                (TriageEntryKind::Site, 720),
            ]
        );
        assert_eq!(list[1].display_name.as_deref(), Some("site-5.example.org"));
        assert_eq!(list[0].display_name.as_deref(), Some("Qwybex app-5"));
    }
}
