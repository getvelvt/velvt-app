//! Ready-to-display insight delivery interfaces.
//!
//! This module owns fetching and sending fully formed insight and history
//! payloads. It does not generate insight text, schedule notifications, or
//! render UI.

#![allow(async_fn_in_trait)]

use chrono::NaiveDate;

use crate::ipc::{HistoryPayload, InsightPayload};

/// Fetches ready-to-display daily insights.
pub trait InsightFetcher {
    /// Fetches an insight for one date.
    async fn fetch_insight(&self, date: NaiveDate) -> Result<InsightPayload, DeliveryError>;
}

/// Fetches ready-to-display history payloads.
pub trait HistoryFetcher {
    /// Fetches the requested number of history days.
    async fn fetch_history(&self, days: u32) -> Result<HistoryPayload, DeliveryError>;
}

/// Delivers ready-to-display payloads to the Swift client.
pub trait DeliveryService {
    /// Sends a daily insight over IPC.
    async fn deliver_insight(&self, insight: &InsightPayload) -> Result<(), DeliveryError>;

    /// Sends a history payload over IPC.
    async fn deliver_history(&self, history: &HistoryPayload) -> Result<(), DeliveryError>;
}

/// Errors produced while fetching or delivering payloads.
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    /// Fetching a ready-to-display payload failed.
    #[error("delivery payload fetch failed")]
    Fetch,
    /// Sending a ready-to-display payload failed.
    #[error("delivery payload send failed")]
    Send,
}
