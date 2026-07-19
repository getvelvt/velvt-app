//! Outbound payload shaping and validation.
//!
//! `ValidatedPayload<T>` is the only type accepted by the push transport.
//! Construction requires passing through `ValidatePayload::validate_fields`,
//! which catches invariant violations before they reach the wire.
//!
//! Payload shaping (cache entry → wire DTO) happens exclusively in this module.
//! Adding a new push type means adding a `ValidatePayload` impl and a shaper
//! function here — no changes to the transport or cache layers.

use serde::Serialize;
use velvt_shared_types::{
    CacheEmpty, HistoryPayload, InsightPayload, LocalDashboardSnapshot, LocalEarlySignalStatus,
    PrivacyViolationAlert,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("required field is empty: {field}")]
    EmptyField { field: &'static str },
    #[error("field value is out of range: {field}")]
    OutOfRange { field: &'static str },
    #[error("payload could not be serialized")]
    Serialization,
}

// ---------------------------------------------------------------------------
// ValidatePayload trait
// ---------------------------------------------------------------------------

/// Domain invariant checker for outbound wire DTOs.
pub trait ValidatePayload: Serialize + Sized {
    /// Human-readable type name used in structured log entries (never payload content).
    const TYPE_NAME: &'static str;

    /// Returns `Ok(())` when all domain invariants are satisfied.
    fn validate_fields(&self) -> Result<(), ValidationError>;
}

// ---------------------------------------------------------------------------
// ValidatedPayload<T> newtype
// ---------------------------------------------------------------------------

/// An outbound DTO that has passed field validation.
///
/// Only constructible via `ValidatedPayload::new`, which calls
/// `ValidatePayload::validate_fields` and a serialisation round-trip check.
///
/// # Compile-time enforcement
///
/// The IPC transport only ever sends messages that arrived through this
/// wrapper.  Two mechanisms make it impossible to bypass:
///
/// 1. **`PushQueue::enqueue` is `pub(super)`** — only `PushAdapter` (same
///    module) can write to the queue.  Integration test code and the IPC
///    connection layer receive only read access (`try_pop`, `notify`).
///
/// 2. **`ValidatedPayload::new` is the only constructor** — there is no
///    `impl From<T> for ValidatedPayload<T>` and the inner field is private,
///    so a caller cannot construct one without calling `validate_fields`.
///
/// The following would fail to compile because `enqueue` is not visible
/// outside the `delivery` module:
///
/// ```compile_fail
/// use velvt_service::delivery::PushQueue;
/// use velvt_shared_types::{CacheEmpty, ServerMessage};
/// async fn bypass(queue: &PushQueue) {
///     // error[E0624]: method `enqueue` is private
///     queue.enqueue(ServerMessage::CacheEmpty(CacheEmpty {
///         payload_type: "history_payload".into(),
///     })).await;
/// }
/// ```
#[derive(Debug)]
pub struct ValidatedPayload<T>(T);

impl<T: ValidatePayload> ValidatedPayload<T> {
    /// Validates `inner` and wraps it. Returns `Err` on any invariant violation.
    pub fn new(inner: T) -> Result<Self, ValidationError> {
        serde_json::to_value(&inner).map_err(|_| ValidationError::Serialization)?;
        inner.validate_fields()?;
        Ok(Self(inner))
    }

    /// Consumes the wrapper and returns the validated value.
    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn type_name() -> &'static str {
        T::TYPE_NAME
    }
}

// ---------------------------------------------------------------------------
// ValidatePayload implementations
// ---------------------------------------------------------------------------

impl ValidatePayload for InsightPayload {
    const TYPE_NAME: &'static str = "insight_payload";

    fn validate_fields(&self) -> Result<(), ValidationError> {
        if self.text.is_empty() {
            return Err(ValidationError::EmptyField { field: "text" });
        }
        if self.evidence.observation.is_empty() {
            return Err(ValidationError::EmptyField {
                field: "evidence.observation",
            });
        }
        if self.evidence.comparison.is_empty() {
            return Err(ValidationError::EmptyField {
                field: "evidence.comparison",
            });
        }
        if self.evidence.suggested_action.is_empty() {
            return Err(ValidationError::EmptyField {
                field: "evidence.suggested_action",
            });
        }
        Ok(())
    }
}

impl ValidatePayload for HistoryPayload {
    const TYPE_NAME: &'static str = "history_payload";

    fn validate_fields(&self) -> Result<(), ValidationError> {
        if self.days == 0 {
            return Err(ValidationError::OutOfRange { field: "days" });
        }
        Ok(())
    }
}

impl ValidatePayload for PrivacyViolationAlert {
    const TYPE_NAME: &'static str = "privacy_violation_alert";

    fn validate_fields(&self) -> Result<(), ValidationError> {
        if self.code.is_empty() {
            return Err(ValidationError::EmptyField { field: "code" });
        }
        if self.message.is_empty() {
            return Err(ValidationError::EmptyField { field: "message" });
        }
        Ok(())
    }
}

impl ValidatePayload for CacheEmpty {
    const TYPE_NAME: &'static str = "cache_empty";

    fn validate_fields(&self) -> Result<(), ValidationError> {
        if self.payload_type.is_empty() {
            return Err(ValidationError::EmptyField {
                field: "payload_type",
            });
        }
        Ok(())
    }
}

impl ValidatePayload for LocalDashboardSnapshot {
    const TYPE_NAME: &'static str = "local_dashboard";

    fn validate_fields(&self) -> Result<(), ValidationError> {
        if self.window_end <= self.window_start {
            return Err(ValidationError::OutOfRange {
                field: "window_bounds",
            });
        }
        if self.segments.len() > 512
            || !self.switches_per_hour.is_finite()
            || self.switches_per_hour < 0.0
        {
            return Err(ValidationError::OutOfRange {
                field: "segments_or_switch_rate",
            });
        }
        for segment in &self.segments {
            if segment.started_at >= segment.ended_at
                || segment.started_at < self.window_start
                || segment.ended_at > self.window_end
                || segment.category.is_empty()
            {
                return Err(ValidationError::OutOfRange { field: "segment" });
            }
        }
        let signal = &self.early_signal;
        if signal.observed_through != self.generated_at
            || signal.observed_seconds > 3600
            || signal.focused_seconds > signal.observed_seconds
            || signal.longest_uninterrupted_seconds > signal.observed_seconds
        {
            return Err(ValidationError::OutOfRange {
                field: "early_signal",
            });
        }
        match signal.status {
            LocalEarlySignalStatus::Ready
                if signal.observation.as_deref().unwrap_or_default().is_empty()
                    || signal
                        .suggested_action
                        .as_deref()
                        .unwrap_or_default()
                        .is_empty()
                    || signal.required_seconds != 0
                    || signal.action_minutes == 0 =>
            {
                return Err(ValidationError::EmptyField {
                    field: "early_signal_ready_copy",
                });
            }
            LocalEarlySignalStatus::InsufficientEvidence
                if signal.observation.is_some()
                    || signal.suggested_action.is_some()
                    || signal.action_minutes != 0 =>
            {
                return Err(ValidationError::OutOfRange {
                    field: "early_signal_insufficient",
                });
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Shaper functions — sole conversion point from cache data to wire DTOs
// ---------------------------------------------------------------------------

pub fn shape_insight(
    payload: InsightPayload,
) -> Result<ValidatedPayload<InsightPayload>, ValidationError> {
    ValidatedPayload::new(payload)
}

pub fn shape_history(
    payload: HistoryPayload,
) -> Result<ValidatedPayload<HistoryPayload>, ValidationError> {
    ValidatedPayload::new(payload)
}

pub fn shape_privacy_alert(
    alert: PrivacyViolationAlert,
) -> Result<ValidatedPayload<PrivacyViolationAlert>, ValidationError> {
    ValidatedPayload::new(alert)
}

pub fn shape_cache_empty(
    payload_type: &str,
) -> Result<ValidatedPayload<CacheEmpty>, ValidationError> {
    ValidatedPayload::new(CacheEmpty {
        payload_type: payload_type.to_owned(),
        reason: None,
    })
}

pub fn shape_local_dashboard(
    payload: LocalDashboardSnapshot,
) -> Result<ValidatedPayload<LocalDashboardSnapshot>, ValidationError> {
    ValidatedPayload::new(payload)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use velvt_shared_types::ConfidenceLevel;

    fn valid_insight() -> InsightPayload {
        InsightPayload {
            date: Utc::now().date_naive(),
            text: "Focus was high this morning.".into(),
            evidence: Default::default(),
            confidence_level: ConfidenceLevel::High,
            low_confidence: false,
            generated_at: Utc::now(),
        }
    }

    fn valid_history() -> HistoryPayload {
        HistoryPayload {
            days: 7,
            summaries: vec![],
        }
    }

    #[test]
    fn valid_insight_passes() {
        assert!(shape_insight(valid_insight()).is_ok());
    }

    #[test]
    fn insight_empty_text_fails() {
        let mut p = valid_insight();
        p.text = String::new();
        let err = shape_insight(p).unwrap_err();
        assert!(matches!(err, ValidationError::EmptyField { field: "text" }));
    }

    #[test]
    fn valid_history_passes() {
        assert!(shape_history(valid_history()).is_ok());
    }

    #[test]
    fn history_zero_days_fails() {
        let p = HistoryPayload {
            days: 0,
            summaries: vec![],
        };
        let err = shape_history(p).unwrap_err();
        assert!(matches!(err, ValidationError::OutOfRange { field: "days" }));
    }

    #[test]
    fn valid_privacy_alert_passes() {
        let alert = PrivacyViolationAlert {
            code: "raw_field_rejected".into(),
            message: "safe diagnostic".into(),
        };
        assert!(shape_privacy_alert(alert).is_ok());
    }

    #[test]
    fn privacy_alert_empty_code_fails() {
        let alert = PrivacyViolationAlert {
            code: String::new(),
            message: "safe diagnostic".into(),
        };
        let err = shape_privacy_alert(alert).unwrap_err();
        assert!(matches!(err, ValidationError::EmptyField { field: "code" }));
    }

    #[test]
    fn privacy_alert_empty_message_fails() {
        let alert = PrivacyViolationAlert {
            code: "raw_field_rejected".into(),
            message: String::new(),
        };
        let err = shape_privacy_alert(alert).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::EmptyField { field: "message" }
        ));
    }

    #[test]
    fn valid_cache_empty_passes() {
        assert!(shape_cache_empty("insight_payload").is_ok());
        assert!(shape_cache_empty("history_payload").is_ok());
    }

    #[test]
    fn cache_empty_empty_type_fails() {
        let err = shape_cache_empty("").unwrap_err();
        assert!(matches!(
            err,
            ValidationError::EmptyField {
                field: "payload_type"
            }
        ));
    }

    #[test]
    fn validated_payload_into_inner_roundtrips() {
        let original = valid_insight();
        let text = original.text.clone();
        let validated = shape_insight(original).unwrap();
        assert_eq!(validated.into_inner().text, text);
    }

    #[test]
    fn validated_payload_type_name_is_correct() {
        assert_eq!(
            ValidatedPayload::<InsightPayload>::type_name(),
            "insight_payload"
        );
        assert_eq!(
            ValidatedPayload::<HistoryPayload>::type_name(),
            "history_payload"
        );
    }
}
