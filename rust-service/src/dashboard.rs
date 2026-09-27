//! Bounded, local-only Focus Fragmentation and Daily Activity aggregation,
//! and the daily summaries built on this Mac when the cloud's are unavailable
//! ([`local_daily_history`]).
//!
//! Rust owns every analytical derivation. Swift receives ready-to-render DTOs
//! and never scans event history. Local display labels appear only in the
//! `daily_activity` branch of this local IPC payload.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone, Timelike, Utc};
use velvt_shared_types::{
    ClassificationConfidence, ClassificationStatus, ConfidenceLevel, DailySummary, HistoryPayload,
    HistorySource, HistoryStatus, LocalComparisonKind, LocalDailyActivityDay,
    LocalDailyActivitySegment, LocalDailyActivityState, LocalDashboardCoverage,
    LocalDashboardSnapshot, LocalEarlySignal, LocalEarlySignalStatus, LocalFocusComparison,
    LocalFocusFragmentation, LocalSwitchingCluster, LocalTimelineSegment, LocalTransitionMarker,
    WorkBlockPhase, WorkBlockSnapshot,
};

use crate::persistence::{PersistenceError, RawEventEntry, RawEventRepo};

const MIN_WINDOW_SECONDS: u32 = 60;
const MAX_WINDOW_SECONDS: u32 = 60 * 60;
const MAX_WINDOW_EVENTS: usize = 512;
/// Per-day read cap for the seven-day activity rows.
///
/// `events_between` is `ORDER BY occurred_at ASC LIMIT`, so a day over this
/// cap is not sampled — it is cut off at its earliest N events, and the rows
/// silently describe the first part of the day as if it were the whole of it.
/// At 2,048 that truncated three of eight days on a real machine, one of them
/// reporting roughly a third of its observed hours. A day is loaded, folded
/// into buckets and dropped one at a time, so the cost of raising this is one
/// day's rows in memory, and the query is single-index and sub-millisecond at
/// every value measured. `day_was_truncated` still reports when the cap binds,
/// because a number that quietly means "some of this day" is worse than a
/// smaller one that says so.
const MAX_DAY_EVENTS: usize = 16_384;
const MAX_EVENT_DURATION_SECONDS: i64 = 30 * 60;
/// Days rendered by the local daily-activity chart.
///
/// Read from `raw_event_buffer`, so raw-event retention must cover at least
/// this window: a shorter TTL renders the oldest days as permanent zeroes
/// rather than as missing data. `raw_event_retention_covers_daily_activity`
/// in `config` pins the relationship.
/// How many local days the Patterns chart covers.
///
/// This number appeared in three places that had to agree and could not be
/// changed in one: here, the outbound validator's `!= 7`, and the retention TTL
/// that decides whether the oldest days still have evidence to draw. The
/// validator now reads this constant, and `config::raw_event_retention_covers_daily_activity`
/// fails the build if the TTL stops covering the window — so raising it is one
/// edit here plus one to `VELVT_RAW_EVENT_TTL_HOURS`, and forgetting the second
/// is a red test rather than seven days silently drawn as zeroes.
///
/// Fourteen rather than seven: these rows are abstracted, local-only metadata,
/// and the same events already persist thirty days in `upload_batch`, so the
/// window was the shorter of the two bounds for no reason a user benefits from.
/// A person who wants the original seven sets `VELVT_RAW_EVENT_TTL_HOURS=168`
/// and changes this to 7.
pub const DAILY_ACTIVITY_DAYS: i64 = 14;
const EARLY_SIGNAL_REQUIRED_SECONDS: u64 = 60;
const EARLY_SIGNAL_ACTION_MINUTES: u32 = 10;
pub const SWITCHING_CLUSTER_RULE_VERSION: u32 = 1;
pub const SWITCHING_CLUSTER_MIN_TRANSITIONS: usize = 3;
pub const SWITCHING_CLUSTER_WINDOW_SECONDS: i64 = 5 * 60;
const SUFFICIENT_COVERAGE_RATIO: f64 = 0.75;
/// Below this, a display bucket is noise rather than a place the day went.
const TINY_SEGMENT_SECONDS: u64 = 30;

/// A bucket under this share of the day folds into `Other`.
///
/// Buckets are per `(stable_id, category)` — per app — so a day spent across
/// many apps of the *same* category had each app individually fall under the
/// old 5% and vanish into `Other` before anything downstream could group them
/// by category. On real data that made `Other` 51% of the busiest day and 28%
/// of the average one: a bar that is mostly one grey slice saying nothing.
/// `Other` also carries no `stable_id`, so every second swept into it is a
/// second the correction workbench cannot offer to teach.
const MINOR_SEGMENT_PERCENT: u64 = 1;

/// How many buckets a day names before the rest folds into `Other`. Twelve
/// keeps `Other` near a tenth of a typical day while leaving the workbench a
/// list a person can still read down.
const MAX_DISPLAY_BUCKETS: usize = 12;

/// The most segments a single day can carry: the named buckets plus `Other`.
///
/// Exported because the outbound validator has to agree with what this module
/// produces, and when the two were independent numbers they drifted the moment
/// one changed — the shaper rejected every snapshot, and the whole local
/// dashboard came back as an error with nothing naming the day that caused it.
/// Deriving the bound from the cap makes disagreement unrepresentable.
pub const MAX_DAILY_ACTIVITY_SEGMENTS: usize = MAX_DISPLAY_BUCKETS + 1;

pub fn snapshot(
    repo: &dyn RawEventRepo,
    work_block: Option<&WorkBlockSnapshot>,
    now: DateTime<Utc>,
    requested_window_seconds: u32,
    utc_offset_seconds: i32,
) -> Result<LocalDashboardSnapshot, PersistenceError> {
    let window_seconds = requested_window_seconds.clamp(MIN_WINDOW_SECONDS, MAX_WINDOW_SECONDS);
    let window_start = now - Duration::seconds(i64::from(window_seconds));
    let events = repo.events_between(
        window_start - Duration::seconds(MAX_EVENT_DURATION_SECONDS),
        now,
        MAX_WINDOW_EVENTS,
    )?;
    let base = aggregate_window(events, window_start, now);
    let offset = FixedOffset::east_opt(utc_offset_seconds.clamp(-86_399, 86_399))
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("zero offset is valid"));
    let daily_activity = daily_activity(repo, now, offset)?;
    let focus_fragmentation = focus_fragmentation(repo, work_block, now, offset)?;

    Ok(LocalDashboardSnapshot {
        generated_at: now,
        window_start,
        window_end: now,
        switch_count: base.switch_count,
        switches_per_hour: base.switches_per_hour,
        coverage: base.coverage,
        early_signal: base.early_signal,
        segments: base.segments,
        focus_fragmentation,
        daily_activity,
    })
}

struct WindowAggregate {
    switch_count: u32,
    switches_per_hour: f64,
    coverage: LocalDashboardCoverage,
    coverage_ratio: f64,
    /// Seconds of the window Velvt could actually categorize. The copy below
    /// counts minutes in these, never in wall-clock minutes: telling someone
    /// what their last hour looked like when only twenty minutes of it were
    /// seen is a claim the evidence does not support.
    observed_seconds: u64,
    longest_uninterrupted_seconds: u64,
    recovery_count: u32,
    early_signal: LocalEarlySignal,
    segments: Vec<LocalTimelineSegment>,
    transitions: Vec<LocalTransitionMarker>,
    clusters: Vec<LocalSwitchingCluster>,
}

fn aggregate_window(
    events: Vec<RawEventEntry>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> WindowAggregate {
    let evidence_event_count = events.len() as u32;
    let segments = build_segments(events, window_start, window_end);
    let transitions = build_transitions(&segments);
    let clusters = group_switching_clusters(&transitions);
    let observed_seconds = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .map(segment_seconds)
        .sum::<u64>();
    let window_seconds = (window_end - window_start).num_seconds().max(0) as u64;
    let coverage_ratio = if window_seconds == 0 {
        0.0
    } else {
        (observed_seconds as f64 / window_seconds as f64).clamp(0.0, 1.0)
    };
    let coverage = coverage_for(observed_seconds, window_seconds);
    let switch_count = transitions.len() as u32;
    let switches_per_hour = if observed_seconds == 0 {
        0.0
    } else {
        f64::from(switch_count) * 3600.0 / observed_seconds as f64
    };
    let longest_uninterrupted_seconds = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .map(segment_seconds)
        .max()
        .unwrap_or(0);
    let recovery_count = recovery_count(&segments);
    let early_signal = early_signal(&segments, evidence_event_count, window_end);

    WindowAggregate {
        switch_count,
        switches_per_hour,
        coverage,
        coverage_ratio,
        observed_seconds,
        longest_uninterrupted_seconds,
        recovery_count,
        early_signal,
        segments,
        transitions,
        clusters,
    }
}

fn build_segments(
    mut events: Vec<RawEventEntry>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Vec<LocalTimelineSegment> {
    events.sort_by_key(|event| event.occurred_at);
    let mut segments: Vec<LocalTimelineSegment> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let Some((started_at, ended_at)) = measured_span(&events, index, window_start, window_end)
        else {
            continue;
        };
        let category = safe_category(event);
        let confidence = parse_confidence(&event.classification_confidence);
        if let Some(previous) = segments.last_mut() {
            if previous.category == category && previous.ended_at >= started_at {
                previous.ended_at = previous.ended_at.max(ended_at);
                previous.confidence = weaker_confidence(previous.confidence, confidence);
                continue;
            }
        }
        segments.push(LocalTimelineSegment {
            id: format!("segment-{}-{}", started_at.timestamp(), segments.len()),
            started_at,
            ended_at,
            category,
            confidence,
        });
    }
    segments
}

/// The part of `[window_start, window_end)` the event at `index` of
/// `events` (sorted by `occurred_at`) was on screen, or `None` when it
/// covers none of it.
///
/// A dwell lasts its reported `duration_seconds`, or until the next event
/// when none was reported, and never past the next event or the window's
/// end: the one measure every local surface uses, so the Daily Activity
/// chart and the summaries built on this Mac count the same seconds.
fn measured_span(
    events: &[RawEventEntry],
    index: usize,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let event = events.get(index)?;
    let started_at = event.occurred_at.max(window_start);
    let next_at = events
        .get(index + 1)
        .map(|next| next.occurred_at)
        .unwrap_or(window_end);
    let measured_end = if event.duration_seconds > 0 {
        event.occurred_at + Duration::seconds(i64::try_from(event.duration_seconds).unwrap_or(1800))
    } else {
        next_at
    };
    let ended_at = measured_end.min(next_at).min(window_end);
    (ended_at > started_at).then_some((started_at, ended_at))
}

fn build_transitions(segments: &[LocalTimelineSegment]) -> Vec<LocalTransitionMarker> {
    let meaningful = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .collect::<Vec<_>>();
    meaningful
        .windows(2)
        .filter(|pair| !pair[0].category.eq_ignore_ascii_case(&pair[1].category))
        .enumerate()
        .map(|(index, pair)| LocalTransitionMarker {
            id: format!("transition-{}-{index}", pair[1].started_at.timestamp()),
            occurred_at: pair[1].started_at,
            from_category: pair[0].category.clone(),
            to_category: pair[1].category.clone(),
            confidence: weaker_confidence(pair[0].confidence, pair[1].confidence),
        })
        .collect()
}

fn group_switching_clusters(transitions: &[LocalTransitionMarker]) -> Vec<LocalSwitchingCluster> {
    let mut qualifying = Vec::<(usize, usize)>::new();
    for start in 0..transitions.len() {
        let mut end = start;
        while end + 1 < transitions.len()
            && (transitions[end + 1].occurred_at - transitions[start].occurred_at).num_seconds()
                <= SWITCHING_CLUSTER_WINDOW_SECONDS
        {
            end += 1;
        }
        if end + 1 - start >= SWITCHING_CLUSTER_MIN_TRANSITIONS {
            qualifying.push((start, end));
        }
    }

    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in qualifying {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    merged
        .into_iter()
        .enumerate()
        .map(|(cluster_index, (start, end))| {
            let slice = &transitions[start..=end];
            let mut categories = Vec::<String>::new();
            for transition in slice {
                for category in [&transition.from_category, &transition.to_category] {
                    if !categories.contains(category) {
                        categories.push(category.clone());
                    }
                }
            }
            let confidence = slice
                .iter()
                .fold(ClassificationConfidence::High, |value, item| {
                    weaker_confidence(value, item.confidence)
                });
            let seconds = (slice.last().expect("cluster is not empty").occurred_at
                - slice.first().expect("cluster is not empty").occurred_at)
                .num_seconds()
                .max(0);
            let explanation = format!(
                "{} switches in {} between {}.",
                slice.len(),
                plain_duration(seconds as u64),
                friendly_list(&categories)
            );
            LocalSwitchingCluster {
                id: format!(
                    "cluster-{}-{cluster_index}",
                    slice
                        .first()
                        .expect("cluster is not empty")
                        .occurred_at
                        .timestamp()
                ),
                rule_version: SWITCHING_CLUSTER_RULE_VERSION,
                started_at: slice.first().expect("cluster is not empty").occurred_at,
                ended_at: slice.last().expect("cluster is not empty").occurred_at,
                transition_count: slice.len() as u32,
                categories,
                confidence,
                explanation,
            }
        })
        .collect()
}

fn focus_fragmentation(
    repo: &dyn RawEventRepo,
    block: Option<&WorkBlockSnapshot>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<Option<LocalFocusFragmentation>, PersistenceError> {
    let Some(block) = block else { return Ok(None) };
    let (Some(block_id), Some(block_start)) = (block.block_id, block.started_at) else {
        return Ok(None);
    };
    if block.phase == WorkBlockPhase::Idle {
        return Ok(None);
    }

    let analysis_end = match block.phase {
        WorkBlockPhase::Active => now,
        WorkBlockPhase::Paused => block.paused_at.unwrap_or(now),
        _ => block
            .analysis_ended_at
            .unwrap_or(block_start + Duration::seconds(i64::from(block.elapsed_duration_seconds))),
    };
    let actual_seconds = (analysis_end - block_start).num_seconds().max(0) as u32;
    let analysis_start = clipped_window_start(block_start, analysis_end);
    let events = repo.events_between(
        analysis_start - Duration::seconds(MAX_EVENT_DURATION_SECONDS),
        analysis_end,
        MAX_WINDOW_EVENTS,
    )?;
    let aggregate = aggregate_window(events, analysis_start, analysis_end);
    let comparison =
        earlier_today_comparison(repo, &aggregate, analysis_start, analysis_end, offset)?;
    let observation = if aggregate.coverage != LocalDashboardCoverage::Good {
        LOW_COVERAGE_BLOCK_COPY.to_owned()
    } else {
        direction_copy(aggregate.switch_count, aggregate.observed_seconds)
    };
    let next_action = block
        .result
        .as_ref()
        .map(|result| result.next_action.label.clone())
        .unwrap_or_else(|| "Protect the next 10 minutes for the work you chose.".to_owned());
    let window_seconds = (analysis_end - analysis_start).num_seconds().max(0) as u64;

    Ok(Some(LocalFocusFragmentation {
        block_id,
        phase: block.phase,
        window_label: window_label(window_seconds, actual_seconds),
        window_started_at: analysis_start,
        window_ended_at: analysis_end,
        planned_duration_seconds: block.planned_duration_seconds,
        elapsed_duration_seconds: block.elapsed_duration_seconds,
        longest_uninterrupted_seconds: aggregate.longest_uninterrupted_seconds,
        observed_switch_count: aggregate.switch_count,
        recovery_count: block
            .result
            .as_ref()
            .map(|result| result.recovery_count)
            .unwrap_or(aggregate.recovery_count),
        coverage: aggregate.coverage,
        coverage_ratio: aggregate.coverage_ratio,
        comparison,
        observation,
        next_action,
        segments: aggregate.segments,
        transitions: aggregate.transitions,
        clusters: aggregate.clusters,
    }))
}

fn earlier_today_comparison(
    repo: &dyn RawEventRepo,
    current: &WindowAggregate,
    current_start: DateTime<Utc>,
    current_end: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<Option<LocalFocusComparison>, PersistenceError> {
    if !comparison_is_eligible(current_start, current_end, current.coverage_ratio) {
        return Ok(None);
    }
    let day_start = local_day_bounds(current_end.with_timezone(&offset).date_naive(), offset).0;
    let earlier_end = current_start;
    let earlier_start = earlier_end - Duration::seconds(i64::from(MAX_WINDOW_SECONDS));
    if earlier_start < day_start {
        return Ok(None);
    }
    let events = repo.events_between(
        earlier_start - Duration::seconds(MAX_EVENT_DURATION_SECONDS),
        earlier_end,
        MAX_WINDOW_EVENTS,
    )?;
    let earlier = aggregate_window(events, earlier_start, earlier_end);
    if earlier.coverage_ratio < SUFFICIENT_COVERAGE_RATIO {
        return Ok(None);
    }
    let delta = current.switch_count as i32 - earlier.switch_count as i32;
    Ok(Some(LocalFocusComparison {
        kind: LocalComparisonKind::EarlierToday,
        label: "versus earlier today".to_owned(),
        switch_delta: delta,
        explanation: comparison_copy(delta),
    }))
}

/// The window a block's evidence covers, as a person reads it.
///
/// `window_seconds` is seconds-since-start for any block under an hour old,
/// so the raw `div_ceil` printed "0 work-block minutes" the instant a block
/// began and "1 work-block minutes" for the rest of the first minute. This
/// label sits outside the timeline gate, so it is the first text a person
/// reads after pressing start, and both readings looked like a broken app.
/// Clamped to a minute because a window shorter than one is not a window the
/// copy can describe, and pluralised because "1 minutes" retracts the care
/// every other string here takes.
fn window_label(window_seconds: u64, actual_seconds: u32) -> String {
    if actual_seconds > MAX_WINDOW_SECONDS {
        return "Most recent 60 work-block minutes".to_owned();
    }
    let minutes = window_seconds.div_ceil(60).max(1);
    format!(
        "{minutes} work-block {}",
        if minutes == 1 { "minute" } else { "minutes" }
    )
}

fn daily_activity(
    repo: &dyn RawEventRepo,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<Vec<LocalDailyActivityDay>, PersistenceError> {
    fold_local_days(repo, now, offset, DAILY_ACTIVITY_DAYS, |day| {
        aggregate_day(
            day.date,
            day.events,
            day.start,
            day.end,
            day.is_today,
            day.truncated,
        )
    })
}

/// One local calendar day's evidence, as read for the chart and for the
/// summaries built on this Mac.
struct LocalDay {
    date: NaiveDate,
    /// Local midnight, in UTC.
    start: DateTime<Utc>,
    /// The next local midnight, or `now` for today.
    end: DateTime<Utc>,
    is_today: bool,
    /// Every event that can reach into the day, including those that began
    /// up to `MAX_EVENT_DURATION_SECONDS` before it.
    events: Vec<RawEventEntry>,
    /// Whether the read stopped at `MAX_DAY_EVENTS`. The cap binding is
    /// indistinguishable, from inside the day, from a day that simply ended
    /// there.
    truncated: bool,
}

/// The last `count` local days, oldest first, each read and folded before
/// the next is loaded, so the cost is one day's rows in memory.
fn fold_local_days<T>(
    repo: &dyn RawEventRepo,
    now: DateTime<Utc>,
    offset: FixedOffset,
    count: i64,
    mut fold: impl FnMut(LocalDay) -> T,
) -> Result<Vec<T>, PersistenceError> {
    let today = now.with_timezone(&offset).date_naive();
    let mut days = Vec::with_capacity(usize::try_from(count).unwrap_or_default());
    for days_ago in (0..count).rev() {
        let date = today - Duration::days(days_ago);
        let (start, end) = local_day_bounds(date, offset);
        let end = end.min(now);
        let events = repo.events_between(
            start - Duration::seconds(MAX_EVENT_DURATION_SECONDS),
            end,
            MAX_DAY_EVENTS,
        )?;
        let truncated = events.len() >= MAX_DAY_EVENTS;
        days.push(fold(LocalDay {
            date,
            start,
            end,
            is_today: date == today,
            events,
            truncated,
        }));
    }
    Ok(days)
}

#[derive(Clone)]
struct DisplayBucket {
    label: String,
    representative_event_id: Option<String>,
    stable_id: Option<String>,
    suggested_name: Option<String>,
    alias_confirmed: bool,
    category: String,
    seconds: u64,
    confidence: ClassificationConfidence,
}

fn aggregate_day(
    date: NaiveDate,
    mut events: Vec<RawEventEntry>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    is_today: bool,
    truncated: bool,
) -> LocalDailyActivityDay {
    events.sort_by_key(|event| event.occurred_at);
    let segments = build_segments(events.clone(), start, end);
    let active_seconds = segments
        .iter()
        .filter(|segment| !segment.category.eq_ignore_ascii_case("SYSTEM"))
        .map(segment_seconds)
        .sum::<u64>();
    let classified_seconds = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .map(segment_seconds)
        .sum::<u64>();
    let coverage_ratio = if active_seconds == 0 {
        0.0
    } else {
        classified_seconds as f64 / active_seconds as f64
    };
    let coverage = if active_seconds == 0 {
        LocalDashboardCoverage::NoData
    } else if truncated || coverage_ratio < SUFFICIENT_COVERAGE_RATIO {
        // A day read up to its cap is partial by construction, however well
        // classified the part that was read happens to be. Calling it `Good`
        // would attach full confidence to a fraction of a day.
        LocalDashboardCoverage::Partial
    } else {
        LocalDashboardCoverage::Good
    };

    let mut by_label = BTreeMap::<(String, String), DisplayBucket>::new();
    for (index, event) in events.iter().enumerate() {
        let Some((measured_start, measured_end)) = measured_span(&events, index, start, end) else {
            continue;
        };
        let seconds = (measured_end - measured_start).num_seconds().max(0) as u64;
        if seconds == 0 || event.category.eq_ignore_ascii_case("SYSTEM") {
            continue;
        }
        let confident = event.classification_status == "classified"
            && matches!(event.classification_confidence.as_str(), "high" | "medium");
        let category = if confident {
            event.category.clone()
        } else {
            "UNCLASSIFIED".to_owned()
        };
        let label = if confident {
            event
                .local_display_label
                .as_deref()
                .filter(|label| !label.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| friendly_category(&category))
        } else {
            "Unclassified".to_owned()
        };
        let confidence = parse_confidence(&event.classification_confidence);
        let bucket = by_label
            .entry((event.stable_id.clone(), category.clone()))
            .or_insert(DisplayBucket {
                label,
                representative_event_id: Some(event.event_id.clone()),
                stable_id: Some(event.stable_id.clone()),
                suggested_name: event.local_name_suggestion.clone(),
                alias_confirmed: event.classification_source == "user_rule",
                category,
                seconds: 0,
                confidence,
            });
        bucket.seconds = bucket.seconds.saturating_add(seconds);
        bucket.confidence = weaker_confidence(bucket.confidence, confidence);
    }
    let mut buckets = by_label.into_values().collect::<Vec<_>>();
    buckets.sort_by(|left, right| {
        right
            .seconds
            .cmp(&left.seconds)
            .then_with(|| left.label.cmp(&right.label))
    });
    let mut selected = Vec::<DisplayBucket>::new();
    let mut other_seconds = 0_u64;
    for bucket in buckets {
        let is_tiny = bucket.seconds < TINY_SEGMENT_SECONDS
            || (active_seconds > 0
                && bucket.seconds.saturating_mul(100)
                    < active_seconds.saturating_mul(MINOR_SEGMENT_PERCENT));
        if is_tiny || selected.len() >= MAX_DISPLAY_BUCKETS {
            other_seconds = other_seconds.saturating_add(bucket.seconds);
        } else {
            selected.push(bucket);
        }
    }
    if other_seconds > 0 {
        selected.push(DisplayBucket {
            label: "Other".to_owned(),
            representative_event_id: None,
            stable_id: None,
            suggested_name: None,
            alias_confirmed: false,
            category: "OTHER".to_owned(),
            seconds: other_seconds,
            confidence: ClassificationConfidence::None,
        });
    }

    let transitions = build_transitions(&segments);
    let clusters = group_switching_clusters(&transitions);
    let longest = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .max_by_key(|segment| segment_seconds(segment));
    let percentages = bucket_percentages(&selected, active_seconds);
    let rendered_segments = selected
        .into_iter()
        .zip(percentages)
        .enumerate()
        .map(|(index, (bucket, percentage))| {
            let cluster = clusters.iter().find(|cluster| {
                cluster.categories.iter().any(|category| category == &bucket.category)
            });
            let explanation = cluster.map(|cluster| {
                format!(
                    "{} Evidence window {}–{} UTC; category confidence is {}.",
                    cluster.explanation,
                    clock_label(cluster.started_at),
                    clock_label(cluster.ended_at),
                    confidence_label(cluster.confidence)
                )
            }).or_else(|| longest.filter(|segment| segment.category == bucket.category).map(|segment| {
                format!(
                    "One sustained {} block lasted {}; evidence window {}–{} UTC with {} confidence.",
                    friendly_category(&bucket.category).to_ascii_lowercase(),
                    plain_duration(segment_seconds(segment)),
                    clock_label(segment.started_at),
                    clock_label(segment.ended_at),
                    confidence_label(segment.confidence)
                )
            }));
            LocalDailyActivitySegment {
                id: format!("{date}-segment-{index}-{}", bucket.category.to_ascii_lowercase()),
                label: bucket.label,
                representative_event_id: bucket
                    .representative_event_id
                    .as_deref()
                    .and_then(|value| uuid::Uuid::parse_str(value).ok()),
                stable_id: bucket.stable_id,
                suggested_name: bucket.suggested_name,
                alias_confirmed: bucket.alias_confirmed,
                category: bucket.category,
                duration_seconds: bucket.seconds,
                percentage,
                confidence: bucket.confidence,
                explanation,
            }
        })
        .collect::<Vec<_>>();
    let state = if active_seconds == 0 {
        LocalDailyActivityState::NoData
    } else if is_today && active_seconds < EARLY_SIGNAL_REQUIRED_SECONDS {
        LocalDailyActivityState::StillBuilding
    } else if coverage != LocalDashboardCoverage::Good {
        LocalDailyActivityState::LowConfidence
    } else {
        LocalDailyActivityState::Ready
    };
    LocalDailyActivityDay {
        id: date.to_string(),
        date,
        state,
        active_seconds,
        coverage,
        segments: rendered_segments,
    }
}

// ---------------------------------------------------------------------------
// Daily summaries built on this Mac (`history_payload`, source `this_mac`)
// ---------------------------------------------------------------------------

/// The furthest from UTC a client's offset is read at: 18 hours, as
/// `request_category_prompt`, the weekly digest and Focus clamp theirs. The
/// dashboard's own request allows up to 86399; no zone is that far out, so
/// the chart and these summaries read every real offset alike.
const MAX_CLIENT_UTC_OFFSET_SECONDS: i32 = 64_800;

/// velvt-core's `modeling_session_gap_seconds`: more than this between two
/// pieces of evidence starts a new work session, and a change of lane across
/// a session boundary is not a switch.
const SESSION_GAP_SECONDS: i64 = 30 * 60;

/// Per-local-day summaries for the last `requested_days` days (at most
/// [`DAILY_ACTIVITY_DAYS`], the raw-event retention), built from this Mac's
/// own retained events: what `request_latest_history` answers with when the
/// account is signed out or the cloud cannot answer. Nothing is stored and
/// nothing is sent anywhere.
///
/// Each field mirrors velvt-core's daily summary
/// (`app/services/daily_summary_service.py` `recompute_daily_summary`) as
/// closely as the local evidence allows, and every difference is stated
/// below. Where the two readings differ, focused time and switches count only
/// confident evidence, so neither claims more focus or more switching than
/// the evidence carries; dwell time is the chart's, so the card and the chart
/// above it never disagree about a day.
///
/// - **Evidence** is every event this Mac retained. Core sees only the events
///   that were uploaded, and none collected while signed out.
/// - **Days** are local calendar days at the client's current offset, the
///   same days the Daily Activity chart draws. Core's are UTC days
///   (`clip_events_to_utc_day`). One fixed offset bounds every day, as it does
///   for the chart, so a day before a daylight-saving change is an hour off
///   its wall-clock midnight rather than 23 or 25 hours long.
/// - **Dwell time** is the chart's measure ([`measured_span`]): a reported
///   dwell, or the time to the next event when none was reported, clipped at
///   the next event, the day's end and now. Core caps every dwell at 30
///   minutes (`modeling_max_inferred_duration_seconds`) and gives an event
///   with no successor 60 seconds; this Mac has the reported dwells those
///   caps approximate.
/// - **`active_seconds`** is the day's measured time outside SYSTEM, the
///   chart's number for the same day. Core also counts SYSTEM time.
/// - **`status`** is `ready` when there is active time, else `no_data`. A day
///   with only SYSTEM time is `ready` in core and `no_data` here, as the
///   chart draws it.
/// - **`event_count`** counts the events with measured time in the day, as
///   core counts the events it modelled into it.
/// - **`focused_seconds`** is confident time ([`segment_is_confident`], the
///   drift gate's bar) in FOCUS_WORK, TASK_MANAGEMENT and REFERENCE: the
///   categories that upload as `document:`, `task:` and `reference:` types
///   and so land in core's `focus_work` and `development_work` lanes
///   (`app/analytics/types.py` `work_lane_for_event`). Core reads the lane
///   off the abstraction type whatever the classification confidence, so it
///   also counts low-confidence time there.
/// - **`meaningful_switch_count`** counts changes of core's work lane
///   ([`work_lane`]) between consecutive confident stretches no more than
///   [`SESSION_GAP_SECONDS`] apart. Core counts lane changes between
///   consecutive events within a session (`sessionization.py`
///   `switch_count`), and there SYSTEM and unclassified time are lanes of
///   their own, so a detour through either is two switches; here it is none,
///   and a change around it is one.
/// - **`longest_uninterrupted_seconds`** is the longest contiguous confident
///   stretch in one category, the dashboard's longest stretch. Core's is the
///   longest work session with no lane change at all (`focus_seconds`): zero
///   when every session had a switch, and otherwise summed across gaps of up
///   to 30 minutes, SYSTEM sessions included. Either can be the longer.
/// - **`confidence_level`** is `low` for every ready day. Core's is `medium`
///   only after 14 earlier summarised days (`confidence_for_prior_days` with
///   `modeling_baseline_mature_summary_count`), and this Mac keeps 14 days in
///   all, so no local day has 14 before it. A `no_data` day is `none`, as in
///   core.
/// - **Cloud-only fields** are never invented: `focus_score` and
///   `fragmentation_score` are null, `baseline_status` is `unavailable`,
///   `baseline_comparison` is `{"status": "unavailable"}`, and
///   `type_proportions` is empty (the chart above the card already draws the
///   day's categories, and nothing reads them from here).
pub fn local_daily_history(
    repo: &dyn RawEventRepo,
    now: DateTime<Utc>,
    utc_offset_seconds: i32,
    requested_days: u8,
) -> Result<HistoryPayload, PersistenceError> {
    let offset = FixedOffset::east_opt(utc_offset_seconds.clamp(
        -MAX_CLIENT_UTC_OFFSET_SECONDS,
        MAX_CLIENT_UTC_OFFSET_SECONDS,
    ))
    .expect("an offset within 18 hours is valid");
    let count = i64::from(requested_days).clamp(1, DAILY_ACTIVITY_DAYS);
    let summaries = fold_local_days(repo, now, offset, count, |day| {
        local_daily_summary(day.date, day.events, day.start, day.end)
    })?;
    Ok(HistoryPayload {
        days: u32::try_from(summaries.len()).unwrap_or(u32::MAX),
        source: HistorySource::ThisMac,
        summaries,
    })
}

fn local_daily_summary(
    date: NaiveDate,
    mut events: Vec<RawEventEntry>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> DailySummary {
    events.sort_by_key(|event| event.occurred_at);
    let event_count = (0..events.len())
        .filter(|&index| measured_span(&events, index, start, end).is_some())
        .count() as u64;
    let segments = build_segments(events, start, end);
    let active_seconds = segments
        .iter()
        .filter(|segment| !segment.category.eq_ignore_ascii_case("SYSTEM"))
        .map(segment_seconds)
        .sum::<u64>();
    if active_seconds == 0 {
        return no_data_summary(date);
    }
    let confident = segments
        .iter()
        .filter(|segment| segment_is_confident(segment))
        .collect::<Vec<_>>();
    let focused_seconds = confident
        .iter()
        .filter(|segment| {
            matches!(
                work_lane(&segment.category),
                "focus_work" | "development_work"
            )
        })
        .map(|segment| segment_seconds(segment))
        .sum();
    let meaningful_switch_count = confident
        .windows(2)
        .filter(|pair| {
            work_lane(&pair[0].category) != work_lane(&pair[1].category)
                && (pair[1].started_at - pair[0].ended_at).num_seconds() <= SESSION_GAP_SECONDS
        })
        .count() as u64;
    let longest_uninterrupted_seconds = confident
        .iter()
        .map(|segment| segment_seconds(segment))
        .max()
        .unwrap_or(0);
    DailySummary {
        date,
        status: HistoryStatus::Ready,
        event_count,
        focus_score: None,
        fragmentation_score: None,
        confidence_level: ConfidenceLevel::Low,
        active_seconds,
        focused_seconds,
        meaningful_switch_count,
        longest_uninterrupted_seconds,
        baseline_status: UNAVAILABLE.to_owned(),
        baseline_comparison: serde_json::json!({ "status": UNAVAILABLE }),
        type_proportions: Vec::new(),
    }
}

/// What a cloud-only field says in a summary built on this Mac.
const UNAVAILABLE: &str = "unavailable";

/// A day with no active time, shaped as core shapes a day it has no summary
/// for (`history_service.py` `serialize_summary(None, …)`): every count zero,
/// confidence `none`.
fn no_data_summary(date: NaiveDate) -> DailySummary {
    DailySummary {
        date,
        status: HistoryStatus::NoData,
        event_count: 0,
        focus_score: None,
        fragmentation_score: None,
        confidence_level: ConfidenceLevel::None,
        active_seconds: 0,
        focused_seconds: 0,
        meaningful_switch_count: 0,
        longest_uninterrupted_seconds: 0,
        baseline_status: UNAVAILABLE.to_owned(),
        baseline_comparison: serde_json::json!({ "status": UNAVAILABLE }),
        type_proportions: Vec::new(),
    }
}

/// Whether a stretch is evidence by the drift gate's bar
/// (`work_block::is_confident`).
///
/// A segment carries a real category only when every event merged into it
/// was classified with high or medium confidence (`safe_category`), and its
/// confidence is the weakest of theirs, so asking the gate about it as
/// `Classified` asks exactly what the gate asks of each event.
fn segment_is_confident(segment: &LocalTimelineSegment) -> bool {
    crate::work_block::is_confident(
        &segment.category,
        ClassificationStatus::Classified,
        segment.confidence,
    )
}

/// velvt-core's work lane for a local category: the lane
/// `work_lane_for_event` (`app/analytics/types.py`) gives the one abstraction
/// type the category uploads as (`upload::dto::cloud_abstraction_type`).
/// FOCUS_WORK uploads as `document:inferred` and TASK_MANAGEMENT as
/// `task:inferred`, both `focus_work`; REFERENCE as `reference:inferred`,
/// `development_work`; PASSIVE_CONSUMPTION and SOCIAL_FEED as `video:` and
/// `social:`, both `consumption`. Any other category is a lane of its own.
fn work_lane(category: &str) -> &str {
    match category {
        "FOCUS_WORK" | "TASK_MANAGEMENT" => "focus_work",
        "REFERENCE" => "development_work",
        "COMMUNICATION" => "communication",
        "PASSIVE_CONSUMPTION" | "SOCIAL_FEED" => "consumption",
        other => other,
    }
}

fn early_signal(
    segments: &[LocalTimelineSegment],
    evidence_event_count: u32,
    observed_through: DateTime<Utc>,
) -> LocalEarlySignal {
    let evidence_segments = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .collect::<Vec<_>>();
    let observed_seconds = evidence_segments
        .iter()
        .map(|segment| segment_seconds(segment))
        .sum();
    let focused_seconds = evidence_segments
        .iter()
        .filter(|segment| segment.category.eq_ignore_ascii_case("FOCUS_WORK"))
        .map(|segment| segment_seconds(segment))
        .sum();
    let transitions = build_transitions(segments);
    let longest_uninterrupted_seconds = evidence_segments
        .iter()
        .map(|segment| segment_seconds(segment))
        .max()
        .unwrap_or(0);
    let observed_from = evidence_segments.first().map(|segment| segment.started_at);
    let is_ready = observed_seconds >= EARLY_SIGNAL_REQUIRED_SECONDS && evidence_event_count > 0;
    LocalEarlySignal {
        status: if is_ready {
            LocalEarlySignalStatus::Ready
        } else {
            LocalEarlySignalStatus::InsufficientEvidence
        },
        observed_from,
        observed_through,
        observed_seconds,
        required_seconds: EARLY_SIGNAL_REQUIRED_SECONDS.saturating_sub(observed_seconds),
        evidence_event_count,
        focused_seconds,
        meaningful_switch_count: transitions.len() as u32,
        longest_uninterrupted_seconds,
        // Does not lead with the count. With no baseline there is nothing to
        // compare 4 to, so a number in first position is Screen Time's exact
        // grammar — count over window, plus a soft nag — and the reader has
        // no way to tell whether it is a lot. The count still appears; it is
        // just no longer the claim.
        observation: is_ready.then(|| direction_copy(transitions.len() as u32, observed_seconds)),
        // One line, carrying the evidence, instead of four rotations of the
        // same contentless advice. Rewording generic advice to seem fresh is
        // a symptom of advice that carries no information, and the rotation
        // was seeded, tested and maintained as if it were a feature.
        //
        // The minutes offered are `EARLY_SIGNAL_ACTION_MINUTES` and not a
        // separately chosen number: the sentence sits directly above a button
        // that starts a block of exactly that length, and a sentence that
        // proposes a different duration from the button under it is a
        // sentence the product does not keep.
        suggested_action: is_ready
            .then(|| {
                dominant_category(&evidence_segments).map(|category| {
                    format!(
                        "{} has had most of your last {}. Want {EARLY_SIGNAL_ACTION_MINUTES} \
                         minutes on it, uninterrupted?",
                        friendly_category(&category),
                        window_minutes_phrase(observed_seconds)
                    )
                })
            })
            .flatten(),
        action_minutes: if is_ready {
            EARLY_SIGNAL_ACTION_MINUTES
        } else {
            0
        },
    }
}

/// The only within-person comparison the product currently has, which makes
/// it the one place a count actually means something. It was spent on "this
/// comparable 60-minute window had 3 more observed category switches than
/// the preceding covered 60-minute window" — a sentence about two windows
/// rather than about the person who was in them.
///
/// Both windows are exactly `MAX_WINDOW_SECONDS`; `comparison_is_eligible`
/// refuses anything shorter, so "hour" is the measurement, not a rounding.
fn comparison_copy(switch_delta: i32) -> String {
    match switch_delta.cmp(&0) {
        std::cmp::Ordering::Less => format!(
            "You changed direction {} fewer times in this hour than in the hour before it.",
            switch_delta.unsigned_abs()
        ),
        std::cmp::Ordering::Equal => {
            "You changed direction the same number of times in this hour as in the hour before it."
                .to_owned()
        }
        std::cmp::Ordering::Greater => format!(
            "You changed direction {} more times in this hour than in the hour before it.",
            switch_delta.unsigned_abs()
        ),
    }
}

/// Said when too little of a declared block could be categorized to support
/// any claim about it. Velvt is the subject on purpose: this is a statement
/// about what the instrument could see, not about what the person did.
///
/// It replaces "Coverage is still building, so Velvt is not making a
/// confident switching comparison." — which shipped a `BANNED_COPY_TOKENS`
/// word ("still") to users because the registry was enforced in four modules
/// and not in this one.
const LOW_COVERAGE_BLOCK_COPY: &str = "Velvt hasn't seen enough of this block yet to say anything.";

/// The one sentence both local surfaces say about a stretch of time: the
/// early signal about the last hour, the work-block card about the block.
///
/// The interpretation in the last branch ("in pieces") is entailed by the
/// number rather than added to it, and it is a claim about the hour, not
/// about the person — which is the line between describing evidence and
/// diagnosing someone.
fn direction_copy(switch_count: u32, observed_seconds: u64) -> String {
    let window = window_minutes_phrase(observed_seconds);
    match switch_count {
        0 => format!("You've been on one thing for the last {window}."),
        1 => format!("One change of direction in the last {window}."),
        2 => format!("Two changes of direction in the last {window}."),
        count => {
            let verb = if observed_seconds / 60 == 1 {
                "has"
            } else {
                "have"
            };
            format!("The last {window} {verb} been in pieces — {count} changes of direction.")
        }
    }
}

/// "minute" or "42 minutes" — the tail of "the last …" and "your last …",
/// counted in categorized seconds rather than wall-clock seconds.
fn window_minutes_phrase(observed_seconds: u64) -> String {
    let minutes = observed_seconds / 60;
    if minutes == 1 {
        "minute".to_owned()
    } else {
        format!("{minutes} minutes")
    }
}

/// The category holding the most categorized time in a window. Ties break on
/// the category name so the answer cannot oscillate between equal candidates
/// across two refreshes of the same popover.
fn dominant_category(segments: &[&LocalTimelineSegment]) -> Option<String> {
    let mut durations = HashMap::<&str, u64>::new();
    for segment in segments {
        *durations.entry(&segment.category).or_default() += segment_seconds(segment);
    }
    durations
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(category, _)| category.to_owned())
}

fn recovery_count(segments: &[LocalTimelineSegment]) -> u32 {
    let meaningful = segments
        .iter()
        .filter(|segment| is_meaningful_category(&segment.category))
        .collect::<Vec<_>>();
    let Some(dominant) = dominant_category(&meaningful) else {
        return 0;
    };
    let mut seen = false;
    let mut away = false;
    let mut recoveries = 0;
    for segment in meaningful {
        if segment.category == dominant {
            if seen && away {
                recoveries += 1;
            }
            seen = true;
            away = false;
        } else if seen {
            away = true;
        }
    }
    recoveries
}

fn local_day_bounds(date: NaiveDate, offset: FixedOffset) -> (DateTime<Utc>, DateTime<Utc>) {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    let start = offset
        .from_local_datetime(&midnight)
        .single()
        .expect("fixed offsets have one local time")
        .with_timezone(&Utc);
    (start, start + Duration::days(1))
}

fn clipped_window_start(block_start: DateTime<Utc>, analysis_end: DateTime<Utc>) -> DateTime<Utc> {
    if (analysis_end - block_start).num_seconds() > i64::from(MAX_WINDOW_SECONDS) {
        analysis_end - Duration::seconds(i64::from(MAX_WINDOW_SECONDS))
    } else {
        block_start
    }
}

fn comparison_is_eligible(
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    coverage_ratio: f64,
) -> bool {
    (window_end - window_start).num_seconds() >= i64::from(MAX_WINDOW_SECONDS)
        && coverage_ratio >= SUFFICIENT_COVERAGE_RATIO
}

#[cfg(test)]
fn rounded_percentage(seconds: u64, total_seconds: u64) -> u32 {
    if total_seconds == 0 {
        0
    } else {
        ((seconds as f64 / total_seconds as f64) * 100.0).round() as u32
    }
}

fn bucket_percentages(buckets: &[DisplayBucket], total_seconds: u64) -> Vec<u32> {
    if total_seconds == 0 {
        return vec![0; buckets.len()];
    }
    let mut percentages = buckets
        .iter()
        .map(|bucket| (bucket.seconds.saturating_mul(100) / total_seconds) as u32)
        .collect::<Vec<_>>();
    let assigned = percentages.iter().sum::<u32>();
    let mut residuals = buckets
        .iter()
        .enumerate()
        .map(|(index, bucket)| (index, bucket.seconds.saturating_mul(100) % total_seconds))
        .collect::<Vec<_>>();
    residuals.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    for (index, _) in residuals
        .into_iter()
        .take(100_u32.saturating_sub(assigned) as usize)
    {
        percentages[index] = percentages[index].saturating_add(1);
    }
    percentages
}

fn coverage_for(observed_seconds: u64, window_seconds: u64) -> LocalDashboardCoverage {
    if observed_seconds == 0 {
        LocalDashboardCoverage::NoData
    } else if window_seconds == 0
        || observed_seconds as f64 / (window_seconds as f64) < SUFFICIENT_COVERAGE_RATIO
    {
        LocalDashboardCoverage::Partial
    } else {
        LocalDashboardCoverage::Good
    }
}

fn segment_seconds(segment: &LocalTimelineSegment) -> u64 {
    (segment.ended_at - segment.started_at).num_seconds().max(0) as u64
}

fn is_meaningful_category(category: &str) -> bool {
    !matches!(
        category.to_ascii_uppercase().as_str(),
        "UNCLASSIFIED" | "SYSTEM" | "IDLE" | "UNLOGGED"
    )
}

fn safe_category(event: &RawEventEntry) -> String {
    if event.classification_status == "classified"
        && matches!(event.classification_confidence.as_str(), "high" | "medium")
    {
        event.category.to_ascii_uppercase()
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

fn weaker_confidence(
    left: ClassificationConfidence,
    right: ClassificationConfidence,
) -> ClassificationConfidence {
    fn rank(value: ClassificationConfidence) -> u8 {
        match value {
            ClassificationConfidence::None => 0,
            ClassificationConfidence::Low => 1,
            ClassificationConfidence::Medium => 2,
            ClassificationConfidence::High => 3,
        }
    }
    if rank(left) <= rank(right) {
        left
    } else {
        right
    }
}

fn friendly_category(category: &str) -> String {
    category
        .replace('_', " ")
        .to_ascii_lowercase()
        .split_whitespace()
        .enumerate()
        .map(|(index, word)| {
            if index == 0 {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn friendly_list(categories: &[String]) -> String {
    let values = categories
        .iter()
        .map(|value| friendly_category(value).to_ascii_lowercase())
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => "classified activity".to_owned(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => format!(
            "{}, and {}",
            values[..values.len() - 1].join(", "),
            values.last().expect("not empty")
        ),
    }
}

fn plain_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} seconds")
    } else {
        let minutes = (seconds + 30) / 60;
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    }
}

fn clock_label(value: DateTime<Utc>) -> String {
    format!("{:02}:{:02}", value.hour(), value.minute())
}

fn confidence_label(value: ClassificationConfidence) -> &'static str {
    match value {
        ClassificationConfidence::High => "high",
        ClassificationConfidence::Medium => "medium",
        ClassificationConfidence::Low => "low",
        ClassificationConfidence::None => "no",
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
            local_name_suggestion: None,
            category: category.to_owned(),
            taxonomy_version: "test".to_owned(),
            classification_tier: "exact_match".to_owned(),
            classification_status: status.to_owned(),
            classification_confidence: confidence.to_owned(),
            classification_source: "seed".to_owned(),
            occurred_at: DateTime::from_timestamp(at, 0).unwrap(),
            duration_seconds: 0,
            upload_eligible: true,
            app_stable_id: None,
            app_scope_eligible: true,
            site_stable_id: None,
        }
    }

    fn measured_event(at: i64, duration: u64, category: &str) -> RawEventEntry {
        let mut value = event(at, category, "classified", "high");
        value.duration_seconds = duration;
        value
    }

    #[test]
    fn clips_segments_to_window_and_deduplicates_categories() {
        let start = DateTime::from_timestamp(100, 0).unwrap();
        let end = DateTime::from_timestamp(700, 0).unwrap();
        let result = aggregate_window(
            vec![
                event(0, "FOCUS_WORK", "classified", "high"),
                event(300, "FOCUS_WORK", "classified", "medium"),
                event(600, "REFERENCE", "classified", "high"),
            ],
            start,
            end,
        );
        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].started_at, start);
        assert_eq!(result.switch_count, 1);
    }

    #[test]
    fn excludes_idle_system_and_unclassified_transitions() {
        let segments = build_segments(
            vec![
                measured_event(0, 60, "FOCUS_WORK"),
                measured_event(60, 60, "SYSTEM"),
                measured_event(120, 60, "REFERENCE"),
                event(180, "COMMUNICATION", "ambiguous", "low"),
            ],
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(240, 0).unwrap(),
        );
        let transitions = build_transitions(&segments);
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].from_category, "FOCUS_WORK");
        assert_eq!(transitions[0].to_category, "REFERENCE");
    }

    #[test]
    fn cluster_boundary_is_inclusive_and_just_above_is_excluded() {
        let marker = |index: usize, at: i64| LocalTransitionMarker {
            id: format!("t-{index}"),
            occurred_at: DateTime::from_timestamp(at, 0).unwrap(),
            from_category: "FOCUS_WORK".to_owned(),
            to_category: "REFERENCE".to_owned(),
            confidence: ClassificationConfidence::High,
        };
        assert_eq!(
            group_switching_clusters(&[marker(0, 0), marker(1, 150), marker(2, 300)]).len(),
            1
        );
        assert!(
            group_switching_clusters(&[marker(0, 0), marker(1, 150), marker(2, 301)]).is_empty()
        );
        assert!(group_switching_clusters(&[marker(0, 0), marker(1, 299)]).is_empty());
    }

    #[test]
    fn overlapping_cluster_windows_merge_deterministically() {
        let transitions = [0, 60, 120, 180, 240]
            .into_iter()
            .enumerate()
            .map(|(index, at)| LocalTransitionMarker {
                id: format!("t-{index}"),
                occurred_at: DateTime::from_timestamp(at, 0).unwrap(),
                from_category: if index % 2 == 0 {
                    "FOCUS_WORK"
                } else {
                    "REFERENCE"
                }
                .to_owned(),
                to_category: if index % 2 == 0 {
                    "REFERENCE"
                } else {
                    "FOCUS_WORK"
                }
                .to_owned(),
                confidence: ClassificationConfidence::High,
            })
            .collect::<Vec<_>>();
        let clusters = group_switching_clusters(&transitions);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].transition_count, 5);
        assert_eq!(clusters[0].rule_version, 1);
    }

    #[test]
    fn recovery_uses_return_to_dominant_category_rule() {
        let result = aggregate_window(
            vec![
                measured_event(0, 180, "FOCUS_WORK"),
                measured_event(180, 60, "REFERENCE"),
                measured_event(240, 180, "FOCUS_WORK"),
            ],
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(420, 0).unwrap(),
        );
        assert_eq!(result.recovery_count, 1);
        assert_eq!(result.longest_uninterrupted_seconds, 180);
    }

    #[test]
    fn work_block_windows_keep_short_duration_and_clip_long_duration_to_sixty_minutes() {
        let start = DateTime::from_timestamp(0, 0).unwrap();
        let short_end = DateTime::from_timestamp(1_500, 0).unwrap();
        let long_end = DateTime::from_timestamp(7_200, 0).unwrap();
        assert_eq!(clipped_window_start(start, short_end), start);
        assert_eq!(
            clipped_window_start(start, long_end),
            DateTime::from_timestamp(3_600, 0).unwrap()
        );
    }

    #[test]
    fn comparison_rejects_partial_and_low_coverage_windows() {
        let start = DateTime::from_timestamp(0, 0).unwrap();
        assert!(!comparison_is_eligible(
            start,
            DateTime::from_timestamp(3_599, 0).unwrap(),
            1.0
        ));
        assert!(!comparison_is_eligible(
            start,
            DateTime::from_timestamp(3_600, 0).unwrap(),
            0.749
        ));
        assert!(comparison_is_eligible(
            start,
            DateTime::from_timestamp(3_600, 0).unwrap(),
            0.75
        ));
    }

    #[test]
    fn day_aggregation_clips_dwell_at_day_boundary_and_handles_zero_rounding() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let start = DateTime::from_timestamp(0, 0).unwrap();
        let end = DateTime::from_timestamp(120, 0).unwrap();
        let day = aggregate_day(
            date,
            vec![measured_event(90, 60, "FOCUS_WORK")],
            start,
            end,
            false,
            false,
        );
        assert_eq!(day.active_seconds, 30);
        assert_eq!(
            day.segments
                .iter()
                .map(|segment| segment.duration_seconds)
                .sum::<u64>(),
            30
        );
        assert_eq!(
            day.segments
                .iter()
                .map(|segment| segment.percentage)
                .sum::<u32>(),
            100
        );
        assert_eq!(rounded_percentage(0, 0), 0);
        assert_eq!(rounded_percentage(1, 3), 33);
        assert_eq!(rounded_percentage(2, 3), 67);
    }

    /// A day cut off at the read cap describes its earliest events only. The
    /// part that was read can be perfectly classified, so `coverage_ratio`
    /// alone would call it Good and hand full confidence to a fraction of a
    /// day. Truncation has to beat the ratio, not be averaged with it.
    #[test]
    fn a_day_cut_off_at_the_read_cap_is_never_reported_as_good_coverage() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let start = DateTime::from_timestamp(0, 0).unwrap();
        let end = DateTime::from_timestamp(120, 0).unwrap();
        let fully_classified = vec![measured_event(0, 120, "FOCUS_WORK")];

        let whole = aggregate_day(date, fully_classified.clone(), start, end, false, false);
        assert_eq!(
            whole.coverage,
            LocalDashboardCoverage::Good,
            "the same evidence, read whole, is good coverage"
        );

        let truncated = aggregate_day(date, fully_classified, start, end, false, true);
        assert_eq!(
            truncated.coverage,
            LocalDashboardCoverage::Partial,
            "a day the cap cut short is partial however well the read part classified"
        );
        assert_eq!(
            truncated.active_seconds, whole.active_seconds,
            "truncation changes the claim about the day, not the seconds actually observed"
        );
    }

    #[test]
    fn day_aggregation_includes_only_the_overlap_from_an_event_before_midnight() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let start = DateTime::from_timestamp(0, 0).unwrap();
        let end = DateTime::from_timestamp(120, 0).unwrap();
        let day = aggregate_day(
            date,
            vec![measured_event(-60, 120, "FOCUS_WORK")],
            start,
            end,
            false,
            false,
        );
        assert_eq!(day.active_seconds, 60);
        assert_eq!(day.segments[0].duration_seconds, 60);
    }

    #[test]
    fn daily_groups_tiny_and_overflow_buckets_into_other() {
        let mut events = (0..7)
            .map(|index| {
                let mut value =
                    measured_event(index * 100, if index < 5 { 100 } else { 20 }, "FOCUS_WORK");
                value.local_display_label = Some(format!("Label {index}"));
                value
            })
            .collect::<Vec<_>>();
        events.push(measured_event(750, 20, "REFERENCE"));
        let day = aggregate_day(
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            events,
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(900, 0).unwrap(),
            false,
            false,
        );
        assert!(day.segments.len() <= 6);
        assert_eq!(
            day.segments.last().map(|segment| segment.label.as_str()),
            Some("Other")
        );
    }

    /// Many apps of one category is the ordinary shape of a working day, and
    /// it used to be the shape the display buckets erased. Each of these ten
    /// reference apps holds 30 seconds of an 1,100-second day — 2.7% each,
    /// under the old 5% floor and at the old 60-second tiny threshold — so
    /// every one of them folded into `Other` individually, and a third of the
    /// day rendered as one grey slice attributed to nothing. Grouping happens
    /// downstream, by category; the buckets have to survive long enough to
    /// reach it.
    #[test]
    fn many_small_apps_of_one_category_survive_to_be_grouped() {
        // One dominant app, then ten small ones that each sit below the old
        // thresholds but above the new ones.
        let mut events = vec![measured_event(0, 800, "FOCUS_WORK")];
        events[0].stable_id = "editor".to_owned();
        for index in 0..10 {
            let mut value = measured_event(800 + index * 30, 30, "REFERENCE");
            value.stable_id = format!("reference-app-{index}");
            value.local_display_label = Some(format!("Reference app {index}"));
            events.push(value);
        }
        let day = aggregate_day(
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            events,
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(1_100, 0).unwrap(),
            false,
            false,
        );

        let other_seconds: u64 = day
            .segments
            .iter()
            .filter(|segment| segment.label == "Other")
            .map(|segment| segment.duration_seconds)
            .sum();
        assert_eq!(
            other_seconds, 0,
            "no app in this day is small enough to be nothing"
        );

        let reference_seconds: u64 = day
            .segments
            .iter()
            .filter(|segment| segment.category.eq_ignore_ascii_case("REFERENCE"))
            .map(|segment| segment.duration_seconds)
            .sum();
        assert_eq!(
            reference_seconds, 300,
            "every reference second is attributable to the category it was spent in"
        );
    }

    #[test]
    fn low_confidence_label_is_replaced_with_unclassified() {
        let mut value = measured_event(0, 120, "COMMUNICATION");
        value.local_display_label = Some("Private Local Label".to_owned());
        value.classification_status = "ambiguous".to_owned();
        value.classification_confidence = "low".to_owned();
        let day = aggregate_day(
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            vec![value],
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(120, 0).unwrap(),
            false,
            false,
        );
        assert_eq!(day.segments[0].label, "Unclassified");
        assert_eq!(day.state, LocalDailyActivityState::LowConfidence);
    }

    /// The enforcement gap that let "Coverage is still building" ship.
    ///
    /// `BANNED_COPY_TOKENS` was checked in `receipts`, `focus`, `initiation`
    /// and `work_block` and in no other module, so this one — which authors
    /// the two sentences a user sees most often — was the only Rust copy
    /// surface with no registry test at all. The registry was not wrong; it
    /// was simply not asked. Every sentence this module can produce is
    /// enumerated below, including the branches a single fixture would never
    /// reach, because a banned word in branch three reaches users exactly as
    /// easily as one in branch one.
    #[test]
    fn every_local_dashboard_sentence_passes_both_copy_registries() {
        let mut registry = vec![
            LOW_COVERAGE_BLOCK_COPY.to_owned(),
            friendly_list(&[]),
            "Protect the next 10 minutes for the work you chose.".to_owned(),
        ];
        for delta in [-4_i32, -1, 0, 1, 4] {
            registry.push(comparison_copy(delta));
        }
        for observed_seconds in [60_u64, 120, 600, 3_600] {
            registry.push(window_minutes_phrase(observed_seconds));
            for switch_count in 0..6_u32 {
                registry.push(direction_copy(switch_count, observed_seconds));
            }
        }
        for category in ["FOCUS_WORK", "COMMUNICATION", "REFERENCE", "UNCLASSIFIED"] {
            registry.push(friendly_category(category));
            registry.push(friendly_list(&[category.to_owned()]));
        }
        for seconds in [0_u64, 1, 59, 60, 90, 3_600] {
            registry.push(plain_duration(seconds));
        }

        // The two rendered surfaces, end to end, rather than only the copy
        // helpers behind them: the early signal and the work-block card.
        for events in [
            vec![measured_event(0, 900, "FOCUS_WORK")],
            vec![
                measured_event(0, 600, "FOCUS_WORK"),
                measured_event(600, 300, "COMMUNICATION"),
                measured_event(900, 600, "FOCUS_WORK"),
            ],
            vec![
                measured_event(0, 120, "FOCUS_WORK"),
                measured_event(120, 120, "COMMUNICATION"),
                measured_event(240, 120, "REFERENCE"),
                measured_event(360, 120, "FOCUS_WORK"),
                measured_event(480, 120, "COMMUNICATION"),
            ],
            vec![measured_event(0, 30, "FOCUS_WORK")],
        ] {
            let aggregate = aggregate_window(
                events,
                DateTime::from_timestamp(0, 0).unwrap(),
                DateTime::from_timestamp(1_800, 0).unwrap(),
            );
            registry.extend(aggregate.early_signal.observation.clone());
            registry.extend(aggregate.early_signal.suggested_action.clone());
            registry.extend(
                aggregate
                    .clusters
                    .iter()
                    .map(|cluster| cluster.explanation.clone()),
            );
            registry.push(if aggregate.coverage != LocalDashboardCoverage::Good {
                LOW_COVERAGE_BLOCK_COPY.to_owned()
            } else {
                direction_copy(aggregate.switch_count, aggregate.observed_seconds)
            });
        }

        for copy in &registry {
            let lowered = copy.to_ascii_lowercase();
            for forbidden in crate::work_block::BANNED_COPY_TOKENS {
                assert!(
                    !lowered.contains(forbidden),
                    "banned copy token {forbidden:?} in local dashboard copy {copy:?}"
                );
            }
            for forbidden in crate::work_block::BANNED_JARGON_TOKENS {
                assert!(
                    !lowered.contains(forbidden),
                    "banned jargon token {forbidden:?} in local dashboard copy {copy:?}"
                );
            }
        }
    }

    /// The early signal must not open with the count. That is Screen Time's
    /// grammar — a number over a window with no baseline to read it against
    /// — and it is the specific defect the rewrite exists to remove, so it
    /// is asserted separately from the vocabulary registries: a sentence can
    /// pass both and still lead with 4.
    #[test]
    fn the_early_signal_never_leads_with_the_count() {
        for observed_seconds in [60_u64, 600, 3_600] {
            for switch_count in 0..6_u32 {
                let copy = direction_copy(switch_count, observed_seconds);
                assert!(
                    !copy.starts_with(|character: char| character.is_ascii_digit()),
                    "early signal leads with the count: {copy:?}"
                );
                assert!(
                    !copy.contains("worth noticing"),
                    "the soft nag is back in {copy:?}"
                );
            }
        }
        assert_eq!(
            direction_copy(0, 1_500),
            "You've been on one thing for the last 25 minutes."
        );
        assert_eq!(
            direction_copy(4, 1_500),
            "The last 25 minutes have been in pieces — 4 changes of direction."
        );
        // Singular windows are reachable: the signal is ready at 60 seconds.
        assert_eq!(
            direction_copy(0, 90),
            "You've been on one thing for the last minute."
        );
        assert_eq!(
            direction_copy(3, 90),
            "The last minute has been in pieces — 3 changes of direction."
        );
    }

    /// The suggested action carries evidence and proposes exactly the block
    /// the button under it starts. The four rotating variants of "Start a
    /// block to hold one thing for a while" are gone: a suggestion that has
    /// to be reworded to seem fresh is a suggestion with nothing in it.
    #[test]
    fn the_suggested_action_names_the_dominant_category_and_matches_its_button() {
        let signal = aggregate_window(
            vec![
                measured_event(0, 900, "FOCUS_WORK"),
                measured_event(900, 300, "COMMUNICATION"),
            ],
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(1_200, 0).unwrap(),
        )
        .early_signal;
        let action = signal
            .suggested_action
            .expect("a ready signal suggests one");
        assert_eq!(
            action,
            "Focus work has had most of your last 20 minutes. Want 10 minutes on it, uninterrupted?"
        );
        assert_eq!(signal.action_minutes, EARLY_SIGNAL_ACTION_MINUTES);
        assert!(
            action.contains(&signal.action_minutes.to_string()),
            "the sentence proposes a different duration from the button under it"
        );
    }

    #[test]
    fn local_display_label_is_redacted_from_debug_and_safe_log_surfaces() {
        let sentinel = "PRIVATE_LOCAL_DISPLAY_SENTINEL";
        let mut value = measured_event(0, 120, "FOCUS_WORK");
        value.local_display_label = Some(sentinel.to_owned());
        let day = aggregate_day(
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            vec![value],
            DateTime::from_timestamp(0, 0).unwrap(),
            DateTime::from_timestamp(120, 0).unwrap(),
            false,
            false,
        );
        assert_eq!(day.segments[0].label, sentinel);
        assert!(!format!("{day:?}").contains(sentinel));
    }

    /// The first label a person reads after pressing start. Both broken
    /// readings were reachable in the first minute of every block, and the
    /// card shows this line in every coverage state, including the one that
    /// says Velvt has not seen enough yet.
    #[test]
    fn the_window_label_is_never_zero_and_never_says_one_minutes() {
        assert_eq!(window_label(0, 0), "1 work-block minute");
        assert_eq!(window_label(1, 1), "1 work-block minute");
        assert_eq!(window_label(59, 59), "1 work-block minute");
        assert_eq!(window_label(60, 60), "1 work-block minute");
        assert_eq!(window_label(61, 61), "2 work-block minutes");
        assert_eq!(window_label(120, 120), "2 work-block minutes");
        // Past the cap the copy stops counting and says so.
        assert_eq!(
            window_label(9_999, MAX_WINDOW_SECONDS + 1),
            "Most recent 60 work-block minutes"
        );
    }

    // --- Daily summaries built on this Mac ---------------------------------

    use crate::persistence::SqlitePersistence;
    use chrono::NaiveDateTime;

    /// UTC-04:00, the offset every local-history test reads at unless it
    /// says otherwise.
    const EDT: i32 = -4 * 3_600;

    /// The instant `local` (`YYYY-MM-DD HH:MM:SS`) names at `offset`.
    fn at_local(local: &str, offset: i32) -> i64 {
        FixedOffset::east_opt(offset)
            .unwrap()
            .from_local_datetime(
                &NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap(),
            )
            .single()
            .unwrap()
            .timestamp()
    }

    fn confident(at: i64, duration: u64, category: &str, confidence: &str) -> RawEventEntry {
        let mut value = event(at, category, "classified", confidence);
        value.duration_seconds = duration;
        value
    }

    fn unconfident(at: i64, duration: u64, category: &str) -> RawEventEntry {
        let mut value = event(at, category, "ambiguous", "low");
        value.duration_seconds = duration;
        value
    }

    fn store(events: &[RawEventEntry]) -> SqlitePersistence {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let repo = persistence.raw_event_repo();
        for value in events {
            repo.insert(value).unwrap();
        }
        persistence
    }

    fn history_at(events: &[RawEventEntry], now: i64, offset: i32, days: u8) -> HistoryPayload {
        let persistence = store(events);
        local_daily_history(
            &*persistence.raw_event_repo(),
            DateTime::from_timestamp(now, 0).unwrap(),
            offset,
            days,
        )
        .unwrap()
    }

    fn day<'a>(history: &'a HistoryPayload, date: &str) -> &'a DailySummary {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        history
            .summaries
            .iter()
            .find(|summary| summary.date == date)
            .unwrap_or_else(|| panic!("no summary for {date}"))
    }

    /// Days are local calendar days at the offset the client sent: a dwell
    /// across local midnight is split between the two days, evidence half an
    /// hour either side of it lands on its own side, and the UTC date of an
    /// event does not decide its day.
    #[test]
    fn local_history_days_end_at_local_midnight_at_the_clients_offset() {
        let events = [
            // 23:30 local on the 25th is 03:30Z on the 26th.
            confident(
                at_local("2026-09-25 23:30:00", EDT),
                600,
                "FOCUS_WORK",
                "high",
            ),
            // 23:55 to 00:05 local: five minutes on each side.
            confident(
                at_local("2026-09-25 23:55:00", EDT),
                600,
                "FOCUS_WORK",
                "high",
            ),
            confident(
                at_local("2026-09-26 00:30:00", EDT),
                600,
                "FOCUS_WORK",
                "high",
            ),
        ];
        let now = at_local("2026-09-27 11:00:00", EDT);

        let history = history_at(&events, now, EDT, 14);

        assert_eq!(day(&history, "2026-09-25").active_seconds, 600 + 300);
        assert_eq!(day(&history, "2026-09-26").active_seconds, 300 + 600);
        assert_eq!(
            history.summaries.last().unwrap().date.to_string(),
            "2026-09-27"
        );

        // The same instants at UTC+09:00 fall on other days entirely.
        let tokyo = history_at(&events, now, 9 * 3_600, 14);
        assert_eq!(day(&tokyo, "2026-09-26").active_seconds, 1_800);
        assert_eq!(day(&tokyo, "2026-09-25").status, HistoryStatus::NoData);
    }

    /// One fixed offset bounds every day, as it bounds the chart's, so each
    /// of the fourteen days is exactly 24 hours whatever the host's zone or
    /// a daylight-saving change inside the window would make of it. The
    /// window below spans the end of US daylight saving (2026-11-01).
    #[test]
    fn local_history_reads_every_day_at_one_fixed_offset() {
        let now = at_local("2026-11-08 12:00:00", EDT);
        // Two minutes centred on each local midnight, from the one that
        // opens today back fourteen days.
        let events = (0..14)
            .map(|days_ago| {
                let midnight = at_local("2026-11-08 00:00:00", EDT) - days_ago * 86_400;
                confident(midnight - 60, 120, "FOCUS_WORK", "high")
            })
            .collect::<Vec<_>>();

        let history = history_at(&events, now, EDT, 14);

        assert_eq!(history.summaries.len(), 14);
        for (index, summary) in history.summaries.iter().enumerate() {
            // A minute at each end of the day; today has no end yet. A
            // boundary an hour off would put both minutes on one side.
            let expected = if index == 13 { 60 } else { 120 };
            assert_eq!(
                summary.active_seconds, expected,
                "{} is not bounded at local midnight",
                summary.date
            );
        }
    }

    /// Confident time is the drift gate's bar. Focused time is confident time
    /// in the categories core's focus lanes hold; low-confidence time is
    /// active but never focused; SYSTEM time is neither, as in the chart.
    #[test]
    fn local_history_counts_only_confident_time_as_focused() {
        let start = at_local("2026-09-26 09:00:00", EDT);
        let events = [
            confident(start, 600, "FOCUS_WORK", "high"),
            confident(start + 600, 300, "REFERENCE", "medium"),
            confident(start + 900, 120, "TASK_MANAGEMENT", "high"),
            unconfident(start + 1_020, 400, "FOCUS_WORK"),
            confident(start + 1_420, 200, "COMMUNICATION", "high"),
            confident(start + 1_620, 180, "PASSIVE_CONSUMPTION", "high"),
            confident(start + 1_800, 500, "SYSTEM", "high"),
            // Classified, but at low confidence: not evidence.
            confident(start + 2_300, 100, "REFERENCE", "low"),
        ];

        let history = history_at(&events, at_local("2026-09-27 11:00:00", EDT), EDT, 14);
        let summary = day(&history, "2026-09-26");

        assert_eq!(summary.status, HistoryStatus::Ready);
        assert_eq!(
            summary.active_seconds,
            600 + 300 + 120 + 400 + 200 + 180 + 100
        );
        assert_eq!(summary.focused_seconds, 600 + 300 + 120);
        assert_eq!(summary.event_count, 8);
    }

    /// A switch is a change of core's work lane between confident stretches
    /// of one session. FOCUS_WORK and TASK_MANAGEMENT share a lane; a detour
    /// through time Velvt could not categorize is not a switch; nor is a
    /// change after more than half an hour without confident evidence.
    #[test]
    fn local_history_counts_lane_changes_within_a_session() {
        let start = at_local("2026-09-26 09:00:00", EDT);
        let events = [
            confident(start, 300, "FOCUS_WORK", "high"),
            // Same lane: no switch.
            confident(start + 300, 300, "TASK_MANAGEMENT", "high"),
            // focus_work -> communication: one.
            confident(start + 600, 300, "COMMUNICATION", "high"),
            // A detour through unclassified time back to the same lane: none.
            unconfident(start + 900, 60, "UNLOGGED"),
            confident(start + 960, 300, "COMMUNICATION", "high"),
            // communication -> development_work: two.
            confident(start + 1_260, 300, "REFERENCE", "high"),
            // Forty-five minutes of SYSTEM time, then another lane: no switch
            // across the session gap.
            confident(start + 1_560, 2_700, "SYSTEM", "high"),
            confident(start + 4_260, 300, "COMMUNICATION", "high"),
            // Consumption lanes are one lane.
            confident(start + 4_560, 300, "PASSIVE_CONSUMPTION", "high"),
            confident(start + 4_860, 300, "SOCIAL_FEED", "high"),
        ];

        let history = history_at(&events, at_local("2026-09-27 11:00:00", EDT), EDT, 14);
        let summary = day(&history, "2026-09-26");

        // Two before the gap, and communication -> consumption after it.
        assert_eq!(summary.meaningful_switch_count, 3);
    }

    /// The longest uninterrupted stretch is the longest contiguous confident
    /// stretch in one category, as the dashboard reads it: unclassified time
    /// ends it.
    #[test]
    fn local_history_longest_stretch_is_contiguous_confident_time() {
        let start = at_local("2026-09-26 09:00:00", EDT);
        let events = [
            confident(start, 1_200, "FOCUS_WORK", "high"),
            confident(start + 1_200, 600, "FOCUS_WORK", "medium"),
            unconfident(start + 1_800, 60, "FOCUS_WORK"),
            confident(start + 1_860, 2_400, "FOCUS_WORK", "high"),
        ];

        let history = history_at(&events, at_local("2026-09-27 11:00:00", EDT), EDT, 14);

        assert_eq!(
            day(&history, "2026-09-26").longest_uninterrupted_seconds,
            2_400
        );
    }

    /// Dwells are measured as the chart measures them: a reported dwell ends
    /// at the next event, one with no reported length runs to the next
    /// event, and the last one of a finished day runs to midnight at most.
    /// The summary's active time is the chart's for the same day.
    #[test]
    fn local_history_measures_dwells_as_the_chart_does() {
        let start = at_local("2026-09-26 22:00:00", EDT);
        let events = [
            // Reported as an hour, but the next event began five minutes in.
            confident(start, 3_600, "FOCUS_WORK", "high"),
            // No reported length: runs to the next event, ten minutes later.
            confident(start + 300, 0, "COMMUNICATION", "high"),
            // No reported length and nothing after it: to midnight, 105 min.
            confident(start + 900, 0, "REFERENCE", "high"),
        ];
        let now = at_local("2026-09-27 11:00:00", EDT);

        let history = history_at(&events, now, EDT, 14);
        let summary = day(&history, "2026-09-26");
        assert_eq!(summary.active_seconds, 300 + 600 + 6_300);

        let persistence = store(&events);
        let chart = daily_activity(
            &*persistence.raw_event_repo(),
            DateTime::from_timestamp(now, 0).unwrap(),
            FixedOffset::east_opt(EDT).unwrap(),
        )
        .unwrap();
        let chart_day = chart.iter().find(|row| row.date == summary.date).unwrap();
        assert_eq!(summary.active_seconds, chart_day.active_seconds);
    }

    /// Today ends now: an open-ended dwell does not claim time that has not
    /// happened yet.
    #[test]
    fn local_history_today_ends_now() {
        let now = at_local("2026-09-27 11:00:00", EDT);
        let events = [confident(now - 1_200, 0, "FOCUS_WORK", "high")];

        let history = history_at(&events, now, EDT, 14);

        assert_eq!(day(&history, "2026-09-27").active_seconds, 1_200);
    }

    /// A day with no active time is `no_data` and says nothing else: every
    /// count zero, confidence none. A day with only SYSTEM time is one of
    /// them, as the chart draws it. A ready day carries no cloud-only value:
    /// the scores are null and the baseline is `unavailable`.
    #[test]
    fn local_history_invents_nothing() {
        let events = [
            confident(at_local("2026-09-25 10:00:00", EDT), 900, "SYSTEM", "high"),
            confident(
                at_local("2026-09-26 10:00:00", EDT),
                900,
                "FOCUS_WORK",
                "high",
            ),
        ];

        let history = history_at(&events, at_local("2026-09-27 11:00:00", EDT), EDT, 14);

        let system_only = day(&history, "2026-09-25");
        assert_eq!(system_only.status, HistoryStatus::NoData);
        assert_eq!(system_only.event_count, 0);
        assert_eq!(system_only.active_seconds, 0);
        assert_eq!(system_only.confidence_level, ConfidenceLevel::None);

        let empty = day(&history, "2026-09-27");
        assert_eq!(empty.status, HistoryStatus::NoData);

        let ready = day(&history, "2026-09-26");
        assert_eq!(ready.status, HistoryStatus::Ready);
        assert_eq!(ready.confidence_level, ConfidenceLevel::Low);
        assert_eq!(ready.focus_score, None);
        assert_eq!(ready.fragmentation_score, None);
        assert_eq!(ready.baseline_status, "unavailable");
        assert_eq!(
            ready.baseline_comparison,
            serde_json::json!({ "status": "unavailable" })
        );
        assert!(ready.type_proportions.is_empty());
    }

    /// At most the fourteen retained days, oldest first, ending on the local
    /// today, and labelled with the rows it carries; evidence older than the
    /// window is not read.
    #[test]
    fn local_history_covers_at_most_fourteen_days_ending_today() {
        let now = at_local("2026-09-27 11:00:00", EDT);
        let events = [
            confident(
                at_local("2026-09-13 10:00:00", EDT),
                900,
                "FOCUS_WORK",
                "high",
            ),
            confident(
                at_local("2026-09-14 10:00:00", EDT),
                900,
                "FOCUS_WORK",
                "high",
            ),
        ];

        let asked_for_thirty = history_at(&events, now, EDT, 30);
        assert_eq!(asked_for_thirty.source, HistorySource::ThisMac);
        assert_eq!(asked_for_thirty.days, 14);
        assert_eq!(asked_for_thirty.summaries.len(), 14);
        assert_eq!(asked_for_thirty.summaries[0].date.to_string(), "2026-09-14");
        assert_eq!(
            asked_for_thirty.summaries[13].date.to_string(),
            "2026-09-27"
        );
        assert!(asked_for_thirty
            .summaries
            .windows(2)
            .all(|pair| pair[1].date - pair[0].date == Duration::days(1)));
        assert_eq!(asked_for_thirty.summaries[0].active_seconds, 900);

        let asked_for_seven = history_at(&events, now, EDT, 7);
        assert_eq!(asked_for_seven.days, 7);
        assert_eq!(asked_for_seven.summaries[0].date.to_string(), "2026-09-21");

        assert_eq!(history_at(&events, now, EDT, 0).days, 1);
    }

    /// An offset past 18 hours is read at 18 hours, as every other
    /// offset-bearing request reads it, rather than refused.
    #[test]
    fn local_history_clamps_the_offset() {
        let now = at_local("2026-09-27 11:00:00", 0);
        let beyond = history_at(&[], now, 90_000, 1);
        let clamped = history_at(&[], now, 64_800, 1);
        assert_eq!(beyond.summaries[0].date, clamped.summaries[0].date);
        assert_eq!(beyond.summaries[0].date.to_string(), "2026-09-28");
    }
}
