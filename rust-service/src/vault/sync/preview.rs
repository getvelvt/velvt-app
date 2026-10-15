//! The document this Mac would seal, in plaintext, for
//! `velvt-service --dry-run-egress --vault` (design §10.4 "Egress").
//!
//! A tester can see exactly what would sync before turning sync on. The
//! document is built from this Mac's own tables by the same functions the
//! engine writes with ([`mirror`](super::mirror)), so what is printed is what
//! the engine would add to the copy. Three values do not exist until sync is
//! on, and are shown as placeholders: `person_salt` (made when the vault is
//! created), this Mac's device id at Velvt (Rust holds the session in memory
//! only), and its name (the computer name the app reports).

use serde_json::Value;

use crate::persistence::{PersistenceError, VaultStateRepo};
use crate::vault::crypto::PersonSalt;
use crate::vault::doc::{JoinedVia, PrefsDocument, ServerDeviceId, SyncDeviceId};
use crate::vault::merge::HlcClock;

use super::mirror::{self, OwnEntry};

pub const SALT_PLACEHOLDER: &str =
    "<32 random bytes, made when sync is turned on; on disk only inside the encrypted copy>";
pub const DEVICE_PLACEHOLDER: &str = "<this Mac's device id at Velvt>";
pub const NAME_PLACEHOLDER: &str = "<this Mac's name, which you can change>";

/// What the dry run prints about sync.
#[derive(Debug, Clone, PartialEq)]
pub struct VaultPreview {
    /// The document, with the placeholders above.
    pub document: Value,
    /// Accounts with sync on on this Mac. Always 0 in a build that cannot
    /// turn it on.
    pub accounts_syncing: usize,
}

/// A database from before migration 0046 has no sync tables; it previews as
/// one that never used sync.
pub fn preview(repo: &dyn VaultStateRepo, now_ms: u64) -> Result<VaultPreview, PersistenceError> {
    let states = repo.vault_states().unwrap_or_default();
    let this_mac = states
        .first()
        .and_then(|state| SyncDeviceId::parse(&state.sync_device_id))
        .unwrap_or_else(SyncDeviceId::generate);
    let mut document = PrefsDocument::new(PersonSalt::from_bytes([0; 32]));
    let mut clock = HlcClock::new(this_mac);
    let own = OwnEntry {
        sync_device_id: this_mac,
        server_device_id: ServerDeviceId::parse("00000000-0000-0000-0000-000000000000")
            .expect("a canonical UUID"),
        joined_via: JoinedVia::Created,
        default_name: None,
    };
    let settings = repo.local_sync_settings()?;
    let links = repo.rule_links().unwrap_or_default();
    let _ = mirror::write_own_entry(&mut document, &own, &mut clock, now_ms);
    let _ = mirror::mirror_settings(&mut document, &settings, &mut clock, now_ms);
    mirror::mirror_rule_links(&mut document, &links);
    let mut value = serde_json::to_value(&document)?;
    value["person_salt"] = Value::String(SALT_PLACEHOLDER.into());
    if let Some(devices) = value["devices"].as_object_mut() {
        for entry in devices.values_mut() {
            entry["server_device_id"]["value"] = Value::String(DEVICE_PLACEHOLDER.into());
            entry["name"]["value"] = Value::String(NAME_PLACEHOLDER.into());
        }
    }
    Ok(VaultPreview {
        document: value,
        accounts_syncing: states.iter().filter(|state| state.enabled).count(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::{preview, DEVICE_PLACEHOLDER, NAME_PLACEHOLDER, SALT_PLACEHOLDER};
    use crate::persistence::{SqlitePersistence, VelvtQuietHours};
    use crate::vault::test_support::NOW_MS;

    #[test]
    fn the_preview_is_this_mac_s_own_registers_with_placeholders() {
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        persistence
            .focus_repo()
            .set_quiet_hours(&VelvtQuietHours {
                start_local_minutes: 1320,
                end_local_minutes: 420,
                rule_version: 1,
                configured_at: DateTime::from_timestamp(1_760_000_000, 0).unwrap(),
            })
            .unwrap();
        let shown = preview(persistence.vault_state_repo().as_ref(), NOW_MS).unwrap();
        assert_eq!(shown.accounts_syncing, 0);
        let document = &shown.document;
        assert_eq!(document["schema"], 1);
        assert_eq!(document["person_salt"], SALT_PLACEHOLDER);
        let entry = document["devices"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        assert_eq!(entry["server_device_id"]["value"], DEVICE_PLACEHOLDER);
        assert_eq!(entry["name"]["value"], NAME_PLACEHOLDER);
        assert_eq!(
            document["settings"]["quiet_hours"]["value"]["start_local_minutes"],
            1320
        );
        assert!(document["settings"].get("invitations_enabled").is_none());
        assert_eq!(document["rules"], serde_json::json!({}));
    }
}
