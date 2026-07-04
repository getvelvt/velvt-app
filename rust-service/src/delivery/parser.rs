//! Strict API response parsing for history and insight endpoints.
//!
//! Unknown fields are silently ignored (default serde behaviour — no
//! `deny_unknown_fields` on outer structs).  Missing *required* fields produce
//! a typed `ParseError` rather than a silent default value.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use velvt_shared_types::{
    ActivityProportion, ConfidenceLevel, DailySummary, HistoryStatus, InsightPayload,
};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("API response missing required field: {field}")]
    MissingField { field: &'static str },
    #[error("API response JSON is structurally invalid")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug)]
pub struct HistoryApiResponse {
    pub summaries: Vec<DailySummary>,
}

// All required fields are wrapped in Option<T> so that a missing field produces
// ParseError::MissingField instead of a serde deserialization error.

#[derive(Deserialize)]
struct RawHistoryResponse {
    summaries: Option<Vec<RawDailySummary>>,
}

#[derive(Deserialize)]
struct RawDailySummary {
    date: Option<NaiveDate>,
    status: Option<HistoryStatus>,
    event_count: Option<u64>,
    active_seconds: Option<u64>,
    confidence_level: Option<ConfidenceLevel>,
    baseline_status: Option<String>,
    baseline_comparison: Option<serde_json::Value>,
    type_proportions: Option<Vec<RawActivityProportion>>,
    // Optional score fields are genuinely nullable.
    focus_score: Option<f64>,
    fragmentation_score: Option<f64>,
}

#[derive(Deserialize)]
struct RawActivityProportion {
    category: Option<String>,
    seconds: Option<u64>,
    proportion: Option<f64>,
}

#[derive(Deserialize)]
struct RawInsightResponse {
    date: Option<NaiveDate>,
    text: Option<String>,
    confidence_level: Option<ConfidenceLevel>,
    low_confidence: Option<bool>,
    generated_at: Option<DateTime<Utc>>,
}

pub fn parse_history(value: serde_json::Value) -> Result<HistoryApiResponse, ParseError> {
    let raw: RawHistoryResponse = serde_json::from_value(value)?;
    let raw_summaries = raw
        .summaries
        .ok_or(ParseError::MissingField { field: "summaries" })?;

    let mut summaries = Vec::with_capacity(raw_summaries.len());
    for s in raw_summaries {
        summaries.push(DailySummary {
            date: s.date.ok_or(ParseError::MissingField {
                field: "summaries[].date",
            })?,
            status: s.status.ok_or(ParseError::MissingField {
                field: "summaries[].status",
            })?,
            event_count: s.event_count.ok_or(ParseError::MissingField {
                field: "summaries[].event_count",
            })?,
            active_seconds: s.active_seconds.ok_or(ParseError::MissingField {
                field: "summaries[].active_seconds",
            })?,
            confidence_level: s.confidence_level.ok_or(ParseError::MissingField {
                field: "summaries[].confidence_level",
            })?,
            baseline_status: s.baseline_status.unwrap_or_else(|| "unknown".into()),
            baseline_comparison: s
                .baseline_comparison
                .unwrap_or_else(|| serde_json::json!({ "status": "unknown" })),
            type_proportions: s
                .type_proportions
                .unwrap_or_default()
                .into_iter()
                .map(|item| ActivityProportion {
                    category: item.category.unwrap_or_else(|| "unclassified".into()),
                    seconds: item.seconds.unwrap_or(0),
                    proportion: item.proportion.unwrap_or(0.0),
                })
                .collect(),
            focus_score: s.focus_score,
            fragmentation_score: s.fragmentation_score,
        });
    }
    Ok(HistoryApiResponse { summaries })
}

pub fn parse_insight(value: serde_json::Value) -> Result<InsightPayload, ParseError> {
    let raw: RawInsightResponse = serde_json::from_value(value)?;
    Ok(InsightPayload {
        date: raw.date.ok_or(ParseError::MissingField { field: "date" })?,
        text: raw.text.ok_or(ParseError::MissingField { field: "text" })?,
        confidence_level: raw.confidence_level.ok_or(ParseError::MissingField {
            field: "confidence_level",
        })?,
        low_confidence: raw.low_confidence.ok_or(ParseError::MissingField {
            field: "low_confidence",
        })?,
        generated_at: raw.generated_at.ok_or(ParseError::MissingField {
            field: "generated_at",
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_history_json() -> serde_json::Value {
        json!({
            "summaries": [{
                "date": "2026-06-14",
                "status": "ready",
                "event_count": 42,
                "active_seconds": 3600,
                "confidence_level": "high",
                "focus_score": 0.8,
                "fragmentation_score": null,
                "unknown_future_field": "ignored"
            }],
            "another_unknown_field": true
        })
    }

    fn valid_insight_json() -> serde_json::Value {
        json!({
            "date": "2026-06-14",
            "text": "Today you spent most time on deep work.",
            "confidence_level": "medium",
            "low_confidence": false,
            "generated_at": "2026-06-14T10:00:00Z",
            "extra_field": "silently ignored"
        })
    }

    #[test]
    fn parse_history_valid() {
        let result = parse_history(valid_history_json()).unwrap();
        assert_eq!(result.summaries.len(), 1);
        let s = &result.summaries[0];
        assert_eq!(s.event_count, 42);
        assert_eq!(s.active_seconds, 3600);
        assert!(s.focus_score.is_some());
        assert!(s.fragmentation_score.is_none());
    }

    #[test]
    fn parse_history_unknown_fields_ignored() {
        // extra top-level and per-summary fields must not cause an error
        parse_history(valid_history_json()).expect("unknown fields should be ignored");
    }

    #[test]
    fn parse_history_missing_summaries_field() {
        let result = parse_history(json!({}));
        assert!(
            matches!(result, Err(ParseError::MissingField { field: "summaries" })),
            "expected MissingField(summaries), got {result:?}"
        );
    }

    #[test]
    fn parse_history_missing_required_summary_field() {
        let bad = json!({
            "summaries": [{
                "date": "2026-06-14",
                "status": "ready",
                // event_count missing
                "active_seconds": 3600,
                "confidence_level": "high"
            }]
        });
        let result = parse_history(bad);
        assert!(
            matches!(
                result,
                Err(ParseError::MissingField {
                    field: "summaries[].event_count"
                })
            ),
            "expected MissingField(event_count), got {result:?}"
        );
    }

    #[test]
    fn parse_history_invalid_json_structure() {
        let result = parse_history(json!("not an object"));
        assert!(matches!(result, Err(ParseError::Json(_))));
    }

    #[test]
    fn parse_insight_valid() {
        let result = parse_insight(valid_insight_json()).unwrap();
        assert_eq!(result.date.to_string(), "2026-06-14");
        assert!(!result.text.is_empty());
        assert!(!result.low_confidence);
    }

    #[test]
    fn parse_insight_unknown_fields_ignored() {
        parse_insight(valid_insight_json()).expect("unknown fields should be ignored");
    }

    #[test]
    fn parse_insight_missing_text_field() {
        let bad = json!({
            "date": "2026-06-14",
            // text missing
            "confidence_level": "high",
            "low_confidence": false,
            "generated_at": "2026-06-14T10:00:00Z"
        });
        let result = parse_insight(bad);
        assert!(
            matches!(result, Err(ParseError::MissingField { field: "text" })),
            "expected MissingField(text), got {result:?}"
        );
    }

    #[test]
    fn parse_insight_missing_generated_at() {
        let bad = json!({
            "date": "2026-06-14",
            "text": "some text",
            "confidence_level": "high",
            "low_confidence": false
            // generated_at missing
        });
        let result = parse_insight(bad);
        assert!(
            matches!(
                result,
                Err(ParseError::MissingField {
                    field: "generated_at"
                })
            ),
            "expected MissingField(generated_at), got {result:?}"
        );
    }

    #[test]
    fn parse_insight_empty_summaries_list_is_valid() {
        let result = parse_history(json!({ "summaries": [] })).unwrap();
        assert_eq!(result.summaries.len(), 0);
    }

    #[test]
    fn parse_history_no_data_status_is_valid() {
        // status: no_data is a first-class value meaning the server has no
        // events for that day.  It must parse to HistoryStatus::NoData and be
        // stored as a regular cache entry, not treated as an error.
        let json = json!({
            "summaries": [{
                "date": "2026-06-14",
                "status": "no_data",
                "event_count": 0,
                "active_seconds": 0,
                "confidence_level": "low"
            }]
        });
        let result = parse_history(json).unwrap();
        assert_eq!(result.summaries.len(), 1);
        assert_eq!(result.summaries[0].status, HistoryStatus::NoData);
        assert_eq!(result.summaries[0].event_count, 0);
    }
}
