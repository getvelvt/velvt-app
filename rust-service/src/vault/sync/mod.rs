//! Sync v1's engine: keeping the `prefs` document in step with velvt-core
//! (design §6.1, §10, §14; PR A3).
//!
//! **Unreachable in this build.** Nothing in the shipped path constructs a
//! [`SyncEngine`]. The messages that would turn sync on, hand Rust the key
//! from the Keychain and show the device list are PR A4's protocol (the one
//! after 38), and
//! until then no vault request is ever sent and no `vault_state` row is ever
//! written.
//!
//! - [`api`]: velvt-core's `/v1/vault` routes and `GET /v1/devices`, as C2
//!   and C1 answer them.
//! - [`verify`]: §6.1, pure: what a Mac believes of the server.
//! - [`keys`]: the Keychain, behind a trait (Swift's `SyncKeyStore` is A4).
//! - [`mirror`]: what this Mac writes into the document, and the one register
//!   it applies (its daily-insight switch). Quiet hours, the invitations
//!   switch and category rules are held (§11).
//! - [`devices`]: the new-sign-in diff and the device list's rows.
//! - [`notices`]: security notices, held until a work block ends (J6).
//! - [`host`]: the Migration Assistant check (J8).
//! - [`preview`]: the document this Mac would seal, for the egress dry run.
//! - [`schedule`]: the triggers of §10.4.

pub mod api;
pub mod devices;
mod engine;
pub mod host;
pub mod keys;
pub mod mirror;
pub mod notices;
pub mod preview;
pub mod schedule;
pub mod verify;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::delivery::InsightNotificationRoute;
use crate::persistence::FocusRepo;

pub use engine::{AccountDeletion, SyncDeps, SyncEngine, SyncSnapshot};

/// How often a syncing Mac fetches (§10.4).
pub const POLL_INTERVAL_MS: i64 = 10 * 60 * 1_000;
/// A request for the device list fetches only if the last fetch is older.
pub const STATE_REQUEST_MIN_INTERVAL_MS: i64 = 60 * 1_000;
/// A local change is written this long after the last one.
pub const WRITE_DEBOUNCE_MS: i64 = 5 * 1_000;
/// Compare-and-swap attempts before backing off (§10.4 Write step 4).
pub const CONFLICT_ATTEMPTS: u32 = 3;
pub const CONFLICT_BACKOFF_MS: i64 = 10 * 60 * 1_000;
/// A key item retired by a verified rotation is deleted this long after the
/// item that replaced it was made (§5.6 step 7).
pub const RETIRED_KEY_GRACE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// The engine's state (`vault_state.state`, §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Off,
    AvailableElsewhere,
    WaitingForKey,
    Syncing,
    WaitingForRotatedKey,
    ResetPending,
    ResetCompleted,
    MissingOnServer,
    ServerCopyUnreadable,
    ServerInconsistent,
    ReadOnlyNewerSchema,
    CloneDetected,
    Error,
}

impl SyncState {
    pub const ALL: [Self; 13] = [
        Self::Off,
        Self::AvailableElsewhere,
        Self::WaitingForKey,
        Self::Syncing,
        Self::WaitingForRotatedKey,
        Self::ResetPending,
        Self::ResetCompleted,
        Self::MissingOnServer,
        Self::ServerCopyUnreadable,
        Self::ServerInconsistent,
        Self::ReadOnlyNewerSchema,
        Self::CloneDetected,
        Self::Error,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::AvailableElsewhere => "available_elsewhere",
            Self::WaitingForKey => "waiting_for_key",
            Self::Syncing => "syncing",
            Self::WaitingForRotatedKey => "waiting_for_rotated_key",
            Self::ResetPending => "reset_pending",
            Self::ResetCompleted => "reset_completed",
            Self::MissingOnServer => "missing_on_server",
            Self::ServerCopyUnreadable => "server_copy_unreadable",
            Self::ServerInconsistent => "server_inconsistent",
            Self::ReadOnlyNewerSchema => "read_only_newer_schema",
            Self::CloneDetected => "clone_detected",
            Self::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }

    /// Whether the person may be offered "Enter a Code". Never when the
    /// server could be steering them to type an attacker's code (§6.1): a
    /// copy that cannot be opened, a key change nothing verified, or a vault
    /// that vanished.
    pub fn offers_code_entry(self) -> bool {
        matches!(
            self,
            Self::WaitingForKey | Self::WaitingForRotatedKey | Self::ResetCompleted
        )
    }
}

/// The clock the engine reads, and the offset its copy is written in.
pub trait SyncClock: Send + Sync {
    fn now_ms(&self) -> i64;
    fn utc_offset_seconds(&self) -> i32 {
        0
    }
}

/// The system clock, with the UTC offset the app last reported
/// (`focus_observer_state`), so notice times read in local time.
pub struct SystemClock {
    focus: Option<Arc<dyn FocusRepo>>,
}

impl SystemClock {
    pub fn new(focus: Option<Arc<dyn FocusRepo>>) -> Self {
        Self { focus }
    }
}

impl SyncClock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn utc_offset_seconds(&self) -> i32 {
        self.focus
            .as_ref()
            .and_then(|focus| focus.utc_offset_seconds().ok().flatten())
            .unwrap_or(0)
    }
}

/// Whether a work block is running: security notices wait for it to end.
pub trait BlockActivity: Send + Sync {
    fn block_active(&self) -> bool;
}

impl BlockActivity for crate::work_block::WorkBlockManager {
    /// An unreadable block state holds the notice: a late notice costs
    /// nothing, one posted mid-block changes drift evidence.
    fn block_active(&self) -> bool {
        self.has_active_block().unwrap_or(true)
    }
}

/// For a service with no work blocks, and for tests.
pub struct NoBlocks;

impl BlockActivity for NoBlocks {
    fn block_active(&self) -> bool {
        false
    }
}

/// The applied routing register (§10.1): whether this Mac posts the daily
/// insight notification. On until a document this Mac opened says otherwise,
/// and on again whenever the session ends.
#[derive(Debug)]
pub struct SyncRouting {
    daily_insight: AtomicBool,
}

impl Default for SyncRouting {
    fn default() -> Self {
        Self {
            daily_insight: AtomicBool::new(true),
        }
    }
}

impl SyncRouting {
    pub(crate) fn set_daily_insight(&self, on: bool) {
        self.daily_insight.store(on, Ordering::Release);
    }

    pub(crate) fn reset(&self) {
        self.set_daily_insight(true);
    }
}

impl InsightNotificationRoute for SyncRouting {
    fn daily_insight_notifications(&self) -> bool {
        self.daily_insight.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::SyncState;

    /// The engine's states are exactly migration 0046's CHECK list.
    #[test]
    fn every_state_round_trips_and_matches_the_migration() {
        let migration = include_str!("../../../migrations/0046_vault_state.sql");
        for state in SyncState::ALL {
            assert_eq!(SyncState::parse(state.as_str()), Some(state));
            assert!(
                migration.contains(&format!("'{}'", state.as_str())),
                "{state:?}"
            );
        }
        assert!(SyncState::ALL
            .iter()
            .filter(|state| state.offers_code_entry())
            .all(|state| !matches!(
                state,
                SyncState::ServerInconsistent
                    | SyncState::ServerCopyUnreadable
                    | SyncState::MissingOnServer
            )));
    }
}
