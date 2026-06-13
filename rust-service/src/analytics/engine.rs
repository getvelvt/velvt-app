//! Compile-time-only analytics placeholder.
//!
//! This module has zero runtime effect and must not be enabled in MVP builds.

/// Deferred local analytics engine placeholder.
pub struct AnalyticsEngine;

impl AnalyticsEngine {
    /// Creates the feature-gated placeholder.
    pub fn new() -> Self {
        todo!("local analytics is deferred beyond MVP")
    }
}
