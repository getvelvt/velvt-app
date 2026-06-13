//! Local raw-to-abstract event transformation interfaces.
//!
//! This module owns stable local identifiers, abstraction labels, and category
//! assignment. It does not own IPC transport, persistence, upload, analytics,
//! or UI behavior.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ipc::RawEvent;

/// Converts local-only raw events into privacy-safe events.
pub trait AbstractionEngine {
    /// Abstracts one raw event before it can enter the upload path.
    fn abstract_event(&self, event: &RawEvent) -> Result<AbstractedEvent, AbstractionError>;
}

/// Handles one supported abstraction type.
pub trait AbstractionTypeHandler {
    /// Produces an abstract label for a raw event.
    fn abstract_label(&self, event: &RawEvent) -> Result<String, AbstractionError>;
}

/// Assigns a coarse privacy-safe category.
pub trait CategoryRuleSet {
    /// Categorizes an abstract label.
    fn categorize(&self, abstract_label: &str) -> Category;
}

/// Privacy-safe event produced before persistence or upload batching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractedEvent {
    /// Stable identifier for the source event.
    pub event_id: Uuid,
    /// UTC time at which the event occurred.
    pub occurred_at: DateTime<Utc>,
    /// Stable local hash that cannot reveal the raw source value.
    pub stable_local_id: String,
    /// Privacy-safe abstract activity label.
    pub abstract_label: String,
    /// Coarse privacy-safe activity category.
    pub category: Category,
}

/// Coarse privacy-safe activity category.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Document-related activity.
    Document,
    /// Communication activity.
    Communication,
    /// Reference or research activity.
    Reference,
    /// Passive consumption activity.
    PassiveConsumption,
    /// Focused work activity.
    FocusWork,
    /// System activity.
    System,
    /// Activity with no recognized category.
    Unclassified,
}

/// Errors produced during local abstraction.
#[derive(Debug, thiserror::Error)]
pub enum AbstractionError {
    /// No supported abstraction handler accepted the event.
    #[error("unsupported abstraction type")]
    UnsupportedType,
    /// Stable identifier generation failed.
    #[error("stable identifier generation failed")]
    StableId,
}
