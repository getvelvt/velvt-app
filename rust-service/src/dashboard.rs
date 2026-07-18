//! Bounded, local-only dashboard aggregation.
//!
//! This module consumes already abstracted local event rows. It never sees raw
//! application names or window titles, and it returns only safe categories and
//! bounded timing aggregates for the menu-bar UI.

use chrono::{DateTime, Duration, Utc};
use velvt_shared_types::{
    ClassificationConfidence, LocalDashboardCoverage, LocalDashboardSnapshot, LocalTimelineSegment,
};

use crate::persistence::{PersistenceError, RawEventEntry, RawEventRepo};

const MIN_WINDOW_SECONDS: u32 = 60;
const MAX_WINDOW_SECONDS: u32 = 60 * 60;
const MAX_EVENTS: usize = 512;

pub fn snapshot(
    repo: &dyn RawEventRepo,
    now: DateTime<Utc>,
    requested_window_seconds: u32,
) -> Result<LocalDashboardSnapshot, PersistenceError> {
    let window_seconds = requested_window_seconds.clamp(MIN_WINDOW_SECONDS, MAX_WINDOW_SECONDS);
    let window_start = now - Duration::seconds(i64::from(window_seconds));
    let events = repo.events_between(window_start, now, MAX_EVENTS)?;
    Ok(aggregate(events, window_start, now))
}

fn aggregate(
    mut events: Vec<RawEventEntry>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> LocalDashboardSnapshot {
    events.sort_by_key(|event| event.occurred_at);

    let mut segments: Vec<LocalTimelineSegment> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let started_at = event.occurred_at.max(window_start);
        let next_at = events
            .get(index + 1)
            .map(|next| next.occurred_at)
            .unwrap_or(window_end);
        let ended_at = next_at.min(window_end);
        if ended_at <= started_at {
            continue;
        }

        let category = safe_category(event);
        let confidence = parse_confidence(&event.classification_confidence);
        if let Some(previous) = segments.last_mut() {
            if previous.category == category
                && previous.confidence == confidence
                && previous.ended_at >= started_at
            {
                previous.ended_at = previous.ended_at.max(ended_at);
                continue;
            }
        }
        segments.push(LocalTimelineSegment {
            started_at,
            ended_at,
            category,
            confidence,
        });
    }

    let switch_count = segments
        .windows(2)
        .filter(|pair| pair[0].category != pair[1].category)
        .count() as u32;
    let observed_seconds: u64 = segments
        .iter()
        .map(|segment| (segment.ended_at - segment.started_at).num_seconds().max(0) as u64)
        .sum();
    let switches_per_hour = if observed_seconds == 0 {
        0.0
    } else {
        f64::from(switch_count) * 3600.0 / observed_seconds as f64
    };
    let coverage = if observed_seconds == 0 {
        LocalDashboardCoverage::NoData
    } else if observed_seconds * 2 < (window_end - window_start).num_seconds() as u64 {
        LocalDashboardCoverage::Partial
    } else {
        LocalDashboardCoverage::Good
    };

    LocalDashboardSnapshot {
        generated_at: window_end,
        window_start,
        window_end,
        switch_count,
        switches_per_hour,
        coverage,
        segments,
    }
}

fn safe_category(event: &RawEventEntry) -> String {
    if event.classification_status == "classified"
        && matches!(event.classification_confidence.as_str(), "high" | "medium")
    {
        event.category.clone()
    } else {
        "UNCLASSIFIED".to_owned()
    }
}

fn parse_confidence(value: &str) -> ClassificationConfidence {
    match value {
        "high" => ClassificationConfidence::High,
        "medium" => ClassificationConfidence::Medium,
        "low" => ClassificationConfidence::Low,
        _ => ClassificationConfidence::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at: i64, category: &str, status: &str, confidence: &str) -> RawEventEntry {
        RawEventEntry {
            event_id: at.to_string(),
            stable_id: format!("stable-{at}"),
            label: category.to_owned(),
            local_display_label: None,
            category: category.to_owned(),
            taxonomy_version: "test".to_owned(),
            classification_tier: "exact_match".to_owned(),
            classification_status: status.to_owned(),
            classification_confidence: confidence.to_owned(),
            classification_source: "seed".to_owned(),
            occurred_at: DateTime::from_timestamp(at, 0).unwrap(),
            duration_seconds: 0,
        }
    }

    #[test]
    fn aggregates_safe_segments_and_switch_rate() {
        let start = DateTime::from_timestamp(0, 0).unwrap();
        let end = DateTime::from_timestamp(3600, 0).unwrap();
        let snapshot = aggregate(
            vec![
                event(0, "FOCUS_WORK", "classified", "high"),
                event(900, "COMMUNICATION", "classified", "medium"),
                event(1800, "COMMUNICATION", "classified", "medium"),
            ],
            start,
            end,
        );

        assert_eq!(snapshot.segments.len(), 2);
        assert_eq!(snapshot.switch_count, 1);
        assert!((snapshot.switches_per_hour - 1.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.coverage, LocalDashboardCoverage::Good);
    }

    #[test]
    fn low_confidence_events_are_safe_unclassified_segments() {
        let start = DateTime::from_timestamp(0, 0).unwrap();
        let end = DateTime::from_timestamp(600, 0).unwrap();
        let snapshot = aggregate(
            vec![event(0, "COMMUNICATION", "ambiguous", "low")],
            start,
            end,
        );

        assert_eq!(snapshot.segments[0].category, "UNCLASSIFIED");
        assert_eq!(
            snapshot.segments[0].confidence,
            ClassificationConfidence::Low
        );
    }
}
