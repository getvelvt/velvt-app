//! Sync v1's engine against an in-memory velvt-core that follows C2's
//! contract and can lie (design §16.1): the malicious-server suite, and the
//! two-Mac integration test.
//!
//! Each "Mac" is a `SyncEngine` over its own SQLite database. Two Macs on one
//! Apple ID share a `MemoryKeyStore`, as iCloud Keychain would give them one
//! key item; a Mac on another Apple ID has its own. Every request goes
//! through the `HttpClient` the service gives the engine, and the fake checks
//! every signature as velvt-core does.

mod fake_core;
mod harness;
mod malicious;
mod two_macs;

use velvt_service::vault::sync::SyncState;

/// 2025-10-15T00:00:00Z.
pub const START_MS: i64 = 1_760_486_400_000;
pub const MINUTE: i64 = 60_000;
pub const HOUR: i64 = 60 * MINUTE;
pub const DAY: i64 = 24 * HOUR;

pub fn assert_state(snapshot: &velvt_service::vault::sync::SyncSnapshot, state: SyncState) {
    assert_eq!(
        snapshot.state, state,
        "{} ({:?})",
        snapshot.status_line, snapshot.error_code
    );
}
