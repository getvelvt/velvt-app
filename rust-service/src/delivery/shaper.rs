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
use velvt_shared_types::{CacheEmpty, HistoryPayload, InsightPayload, PrivacyViolationAlert};

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
/// This makes it a compile-time contract: the transport layer can only accept
/// a `ValidatedPayload<T>`, never a raw DTO.
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
    })
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
