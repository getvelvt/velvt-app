//! The block-features file of the cohort export (format 7, attention plan
//! PR-5) states each block the way the end-of-block result and the drift
//! gate see it: the result's coverage, switch-aways, returns and longest run
//! as stored in `work_block_result`, the card's branch, and two shares of
//! confident time, one in the block's dominant work category and one in
//! feeds. The exporter computes them in SQL and `scripts/analyze_cohort.py`
//! restates them (`block_features`); `scripts/tests/block_features_test.sh`
//! holds both to the vectors in
//! `scripts/tests/fixtures/block-feature-vectors.json`.
//!
//! This test holds the vectors to the app. For every case it replays the
//! observations through [`aggregate_result`], the function that writes the
//! stored result, and requires:
//!
//! - every `result` field the exporter reads, under the key it reads it by,
//!   so a renamed field or a changed count fails here rather than in an
//!   export;
//! - the card's branch, read from the sentence [`block_result_copy`] chose;
//! - whether the category that sentence is about is one of the gate's work
//!   categories ([`ANCHOR_CATEGORIES`]), which the exporter's list must equal;
//! - the work share, from [`work_anchor`] and [`is_confident`], and the feed
//!   share, from [`is_confident`].
//!
//! Test-only: nothing here runs in the app.

use serde_json::Value;

use super::*;

const VECTORS: &str = include_str!("../../../scripts/tests/fixtures/block-feature-vectors.json");

const WORK_CATEGORIES: [&str; 4] = [
    "FOCUS_WORK",
    "REFERENCE",
    "TASK_MANAGEMENT",
    "COMMUNICATION",
];
const FEED_CATEGORIES: [&str; 2] = ["SOCIAL_FEED", "PASSIVE_CONSUMPTION"];

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string in {value}"))
}

fn integer(value: &Value, key: &str) -> i64 {
    value[key]
        .as_i64()
        .unwrap_or_else(|| panic!("{key} is not an integer in {value}"))
}

fn seconds_u32(value: &Value, key: &str) -> u32 {
    u32::try_from(integer(value, key)).expect("seconds fit in u32")
}

fn status(name: &str) -> ClassificationStatus {
    match name {
        "classified" => ClassificationStatus::Classified,
        "ambiguous" => ClassificationStatus::Ambiguous,
        "unclassified" => ClassificationStatus::Unclassified,
        other => panic!("unknown classification status {other}"),
    }
}

fn confidence(name: &str) -> ClassificationConfidence {
    match name {
        "high" => ClassificationConfidence::High,
        "medium" => ClassificationConfidence::Medium,
        "low" => ClassificationConfidence::Low,
        "none" => ClassificationConfidence::None,
        other => panic!("unknown classification confidence {other}"),
    }
}

fn phase(name: &str) -> WorkBlockPhase {
    match name {
        "completed" => WorkBlockPhase::Completed,
        "abandoned" => WorkBlockPhase::Abandoned,
        "expired" => WorkBlockPhase::Expired,
        other => panic!("a rated block has ended; {other} is not an ended phase"),
    }
}

/// The branch of [`block_result_copy`] a sentence came from. The exporter
/// derives the same branch from the stored fields instead of the words.
fn branch_of(observation: &str) -> &'static str {
    if observation == LOW_COVERAGE_RESULT_COPY {
        "low_coverage"
    } else if observation.starts_with("You stayed on ") {
        "stayed"
    } else if observation.contains(" and the block ended somewhere else.") {
        "ended_elsewhere"
    } else if observation.contains(" came back") {
        "came_back"
    } else {
        panic!("no card branch names the sentence {observation:?}")
    }
}

/// Six decimal places, as the export writes every share.
fn round6(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

fn share(part: u32, whole: u32) -> f64 {
    if whole == 0 {
        0.0
    } else {
        round6(f64::from(part) / f64::from(whole))
    }
}

/// The exporter's SQL and `analyze_cohort.py` name the work categories in
/// their own words; this is the list they copy, and it must be the gate's.
#[test]
fn the_exported_work_categories_are_the_gates() {
    assert_eq!(WORK_CATEGORIES, ANCHOR_CATEGORIES);
}

#[test]
fn block_feature_vectors_match_the_result_the_app_records() {
    let vectors: Value = serde_json::from_str(VECTORS).expect("the vectors are JSON");
    assert_eq!(vectors["block_feature_version"], 1);
    let cases = vectors["cases"].as_array().expect("cases");
    assert!(cases.len() >= 12, "the vectors lost their cases");
    for case in cases {
        let name = text(case, "name");
        let base = integer(case, "base");
        let at = |offset: i64| DateTime::from_timestamp(base + offset, 0).expect("a valid time");
        let block = &case["block"];
        let ended_at = at(integer(block, "ended_at"));
        let phase = phase(text(block, "phase"));
        let record = WorkBlockRecord {
            block_id: name.to_owned(),
            phase,
            intention: None,
            purpose: None,
            intensity: WorkBlockIntensity::Medium,
            planned_duration_seconds: seconds_u32(block, "planned_duration_seconds"),
            started_at: at(integer(block, "started_at")),
            paused_at: None,
            total_paused_seconds: seconds_u32(block, "total_paused_seconds"),
            ended_at: Some(ended_at),
            recovered_after_restart: false,
            recovery_of: None,
            origin: WorkBlockOrigin::Manual,
            intention_expires_at: ended_at,
            updated_at: ended_at,
        };
        let observations = case["observations"]
            .as_array()
            .expect("observations")
            .iter()
            .map(|row| WorkBlockObservation {
                occurred_at: at(row[0].as_i64().expect("start")),
                ended_at: row[1].as_i64().map(at),
                category: row[2].as_str().expect("category").to_owned(),
                classification_status: status(row[3].as_str().expect("status")),
                classification_confidence: confidence(row[4].as_str().expect("confidence")),
            })
            .collect::<Vec<_>>();
        let expect = &case["expect"];

        // The gate's elapsed-time arithmetic is the export's elapsed_seconds.
        let elapsed = elapsed_seconds(&record, ended_at);
        assert_eq!(
            i64::from(elapsed),
            integer(expect, "elapsed_seconds"),
            "{name}: elapsed"
        );

        let result = aggregate_result(
            &record,
            phase,
            elapsed,
            &observations,
            seconds_u32(case, "covered_seconds"),
        );
        let stored = serde_json::to_value(&result).expect("the result serializes");
        for (key, want) in case["result"].as_object().expect("result") {
            assert_eq!(&stored[key], want, "{name}: stored result field {key}");
        }
        let branch = branch_of(&result.observation);
        assert_eq!(branch, text(expect, "card_branch"), "{name}: card branch");

        // Whether the card is about work: the category its sentence names is
        // the stored safe_evidence_category, and a card with nothing honest
        // to say names none.
        let anchor_is_work = if branch == "low_coverage" {
            Value::Null
        } else {
            let category = result
                .safe_evidence_category
                .as_deref()
                .expect("a card that is not low coverage names a category");
            Value::from(i64::from(
                ANCHOR_CATEGORIES
                    .iter()
                    .any(|work| work.eq_ignore_ascii_case(category)),
            ))
        };
        assert_eq!(
            &anchor_is_work, &expect["card_anchor_is_work"],
            "{name}: card_anchor_is_work"
        );

        // The shares, from the gate's own test of evidence and its own anchor.
        let confident = observations
            .iter()
            .filter(|observation| {
                is_confident(
                    &observation.category,
                    observation.classification_status,
                    observation.classification_confidence,
                )
            })
            .filter_map(|observation| {
                let ended = observation.ended_at?;
                Some((
                    observation.category.as_str(),
                    positive_seconds(ended - observation.occurred_at),
                ))
            })
            .collect::<Vec<_>>();
        let total = confident.iter().map(|(_, seconds)| seconds).sum::<u32>();
        let anchor = work_anchor(&observations);
        if let Some(anchor) = anchor.as_deref() {
            assert!(
                WORK_CATEGORIES.contains(&anchor),
                "{name}: the work anchor {anchor} is not a work category"
            );
        }
        let anchor_seconds = confident
            .iter()
            .filter(|(category, _)| Some(*category) == anchor.as_deref())
            .map(|(_, seconds)| seconds)
            .sum::<u32>();
        let feed_seconds = confident
            .iter()
            .filter(|(category, _)| {
                FEED_CATEGORIES
                    .iter()
                    .any(|feed| feed.eq_ignore_ascii_case(category))
            })
            .map(|(_, seconds)| seconds)
            .sum::<u32>();
        let want_work = expect["work_anchor_share"].as_f64().expect("work share");
        let want_feed = expect["feed_share"].as_f64().expect("feed share");
        assert!(
            (share(anchor_seconds, total) - want_work).abs() < 1e-9,
            "{name}: work share {} against {want_work}",
            share(anchor_seconds, total)
        );
        assert!(
            (share(feed_seconds, total) - want_feed).abs() < 1e-9,
            "{name}: feed share {} against {want_feed}",
            share(feed_seconds, total)
        );
    }
}
