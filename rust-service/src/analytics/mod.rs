// ANALYTICS MODULE — FEATURE-FLAGGED STUB
// DO NOT ACTIVATE IN MVP BUILDS.
// This module is a compile-time stub only. All types and functions
// have no-op implementations guarded by the `local_analytics` feature flag.
// Activating this in default builds violates the architectural constraint
// of keeping the default client lightweight.
#[cfg(feature = "local_analytics")]
pub mod engine;
