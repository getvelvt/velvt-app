//! This Mac's registers: what it writes into the document, and the one thing
//! it applies from it (design §10.1, §10.4 "Write" step 1, §11).
//!
//! Written: this Mac's own entry, its rule links (PR A6 makes them), and its
//! quiet hours and invitations switch when they are newer than the
//! document's. **Held**: quiet hours, the invitations switch and every rule
//! are stored in the document and never applied from it, so no synced value
//! reaches a drift, initiation, demotion, reminder or digest input on any Mac
//! (§11). Nothing in this module writes a local policy table.
//!
//! Applied: Mac names (to the device list) and this Mac's daily-insight
//! switch ([`daily_insight_here`]).

use crate::persistence::{LocalSyncSettings, VaultRuleLinkRecord};
use crate::vault::crypto::{PersonKey, PersonKeyKind};
use crate::vault::doc::{
    Category, DeviceEntry, DeviceName, DocumentError, JoinedVia, PrefsDocument, QuietHours,
    Register, RuleKey, RuleRegister, ServerDeviceId, SyncDeviceId, LAST_LOCAL_MINUTE,
};
use crate::vault::merge::{Hlc, HlcClock, HlcError};

/// The name a Mac gets when Swift reported none.
pub const FALLBACK_NAME: &str = "Mac";

/// What this Mac writes about itself.
#[derive(Debug, Clone)]
pub struct OwnEntry {
    pub sync_device_id: SyncDeviceId,
    pub server_device_id: ServerDeviceId,
    /// How this Mac got `K`, for a first entry. Written once (§10.3).
    pub joined_via: JoinedVia,
    /// The computer name Swift reported, for a first entry only: after that
    /// the name is a register any Mac may rename.
    pub default_name: Option<DeviceName>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MirrorError {
    #[error(transparent)]
    Clock(#[from] HlcError),
    #[error(transparent)]
    Document(#[from] DocumentError),
}

/// Writes this Mac's entry into `document`: a new entry when it has none,
/// otherwise its server device id when that changed (a cap eviction gives a
/// Mac a new one, §14) and `removed: false` when it rejoins after Turn Off
/// Sync. Returns whether anything changed.
pub fn write_own_entry(
    document: &mut PrefsDocument,
    own: &OwnEntry,
    clock: &mut HlcClock,
    now_ms: u64,
) -> Result<bool, MirrorError> {
    match document.devices.get(&own.sync_device_id).cloned() {
        None => {
            let hlc = clock.tick(now_ms)?;
            let name = own
                .default_name
                .clone()
                .or_else(|| DeviceName::new(FALLBACK_NAME))
                .ok_or(DocumentError::OutOfRange("name"))?;
            document.set_device(
                own.sync_device_id,
                DeviceEntry {
                    joined_via: own.joined_via,
                    joined_at_ms: hlc.wall_ms(),
                    server_device_id: Register {
                        value: own.server_device_id,
                        hlc,
                    },
                    name: Register { value: name, hlc },
                    notify_daily_insight: Register { value: true, hlc },
                    removed: Register { value: false, hlc },
                },
            )?;
            Ok(true)
        }
        Some(mut entry) => {
            let mut changed = false;
            if entry.server_device_id.value != own.server_device_id {
                entry.server_device_id = Register {
                    value: own.server_device_id,
                    hlc: clock.tick(now_ms)?,
                };
                changed = true;
            }
            if entry.removed.value {
                entry.removed = Register {
                    value: false,
                    hlc: clock.tick(now_ms)?,
                };
                changed = true;
            }
            if changed {
                document.set_device(own.sync_device_id, entry)?;
            }
            Ok(changed)
        }
    }
}

/// Marks this Mac's entry removed (Turn Off Sync on This Mac, J4).
pub fn mark_removed(
    document: &mut PrefsDocument,
    sync_device_id: SyncDeviceId,
    clock: &mut HlcClock,
    now_ms: u64,
) -> Result<bool, MirrorError> {
    let Some(entry) = document.devices.get_mut(&sync_device_id) else {
        return Ok(false);
    };
    if entry.removed.value {
        return Ok(false);
    }
    entry.removed = Register {
        value: true,
        hlc: clock.tick(now_ms)?,
    };
    Ok(true)
}

/// Writes this Mac's quiet hours and invitations switch into the document's
/// registers when the local value was set after the register was written and
/// differs from it (§10.4 Write step 1). A Mac that never set one writes
/// nothing. Held: the document's value is never applied (§10.1).
pub fn mirror_settings(
    document: &mut PrefsDocument,
    local: &LocalSyncSettings,
    clock: &mut HlcClock,
    now_ms: u64,
) -> Result<bool, MirrorError> {
    let mut changed = false;
    if let Some(quiet_hours) = &local.quiet_hours {
        let value = quiet_hours_value(quiet_hours);
        let set_at = millis(quiet_hours.configured_at.timestamp_millis());
        if newer_and_different(document.settings.quiet_hours.as_ref(), &value, set_at) {
            document.settings.quiet_hours = Some(Register {
                value,
                hlc: clock.tick(now_ms)?,
            });
            changed = true;
        }
    }
    if let Some((enabled, updated_at)) = local.invitations {
        let set_at = millis(updated_at.timestamp_millis());
        if newer_and_different(
            document.settings.invitations_enabled.as_ref(),
            &enabled,
            set_at,
        ) {
            document.settings.invitations_enabled = Some(Register {
                value: enabled,
                hlc: clock.tick(now_ms)?,
            });
            changed = true;
        }
    }
    Ok(changed)
}

fn millis(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn newer_and_different<T: PartialEq>(
    register: Option<&Register<T>>,
    value: &T,
    set_at_ms: u64,
) -> bool {
    match register {
        None => true,
        Some(register) => set_at_ms > register.hlc.wall_ms() && register.value != *value,
    }
}

/// `velvt_quiet_hours` within the document's bounds, or `None` (off) for a
/// row the document could not hold.
fn quiet_hours_value(local: &crate::persistence::VelvtQuietHours) -> Option<QuietHours> {
    let minutes = |value: u32| {
        u16::try_from(value)
            .ok()
            .filter(|minutes| *minutes <= LAST_LOCAL_MINUTE)
    };
    Some(QuietHours {
        start_local_minutes: minutes(local.start_local_minutes)?,
        end_local_minutes: minutes(local.end_local_minutes)?,
        rule_version: local.rule_version.max(1),
    })
}

/// Writes each rule link whose register is newer than the document's. A link
/// that does not parse is skipped, never written. Returns the rule keys the
/// document had no room for (past 2,000 live rules, §4.1).
pub fn mirror_rule_links(
    document: &mut PrefsDocument,
    links: &[VaultRuleLinkRecord],
) -> Vec<RuleKey> {
    let mut over_cap = Vec::new();
    for link in links {
        let Some((key, register)) = rule_from_link(link) else {
            continue;
        };
        let newer = document
            .rules
            .get(&key)
            .is_none_or(|existing| register.hlc > existing.hlc);
        if newer && document.set_rule(key, register).is_err() {
            over_cap.push(key);
        }
    }
    over_cap
}

fn rule_from_link(link: &VaultRuleLinkRecord) -> Option<(RuleKey, RuleRegister)> {
    let kind = PersonKeyKind::from_prefix(&link.local_kind)?;
    let key = PersonKey::from_hex(&link.person_key)?;
    Some((
        RuleKey { kind, key },
        RuleRegister {
            category: Category::parse(&link.category)?,
            deleted: link.deleted,
            hlc: Hlc::parse(&link.hlc)?,
        },
    ))
}

/// The one register this Mac applies (§10.1): its own `notify_daily_insight`.
/// `true` when the document has no entry for it.
pub fn daily_insight_here(document: &PrefsDocument, this_mac: SyncDeviceId) -> bool {
    document
        .devices
        .get(&this_mac)
        .is_none_or(|entry| entry.removed.value || entry.notify_daily_insight.value)
}

/// What the document holds that this Mac does not apply, for the device
/// list's line (J2): live rules by kind, and which held settings are set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldSummary {
    pub site_rules: usize,
    pub app_rules: usize,
    pub settings: Vec<&'static str>,
}

pub fn held_summary(document: &PrefsDocument) -> HeldSummary {
    let live = |kind: PersonKeyKind| {
        document
            .rules
            .iter()
            .filter(|(key, rule)| key.kind == kind && !rule.deleted)
            .count()
    };
    let mut settings = Vec::new();
    if document.settings.quiet_hours.is_some() {
        settings.push("quiet_hours");
    }
    if document.settings.invitations_enabled.is_some() {
        settings.push("invitations_enabled");
    }
    HeldSummary {
        site_rules: live(PersonKeyKind::Site),
        app_rules: live(PersonKeyKind::Bundle) + live(PersonKeyKind::App),
        settings,
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::{
        daily_insight_here, held_summary, mark_removed, mirror_rule_links, mirror_settings,
        write_own_entry, OwnEntry,
    };
    use crate::persistence::{LocalSyncSettings, VaultRuleLinkRecord, VelvtQuietHours};
    use crate::vault::doc::{JoinedVia, PrefsDocument, QuietHours};
    use crate::vault::merge::HlcClock;
    use crate::vault::test_support::{
        device, hlc, name, salt, sample_document, server_device, NOW_MS,
    };

    fn own(seed: u8) -> OwnEntry {
        OwnEntry {
            sync_device_id: device(seed),
            server_device_id: server_device(seed),
            joined_via: JoinedVia::IcloudKeychain,
            default_name: Some(name("Mac mini")),
        }
    }

    #[test]
    fn a_mac_writes_its_entry_once_and_after_that_only_what_changed() {
        let mut document = PrefsDocument::new(salt(1));
        let mut clock = HlcClock::new(device(3));
        assert!(write_own_entry(&mut document, &own(3), &mut clock, NOW_MS).unwrap());
        let entry = &document.devices[&device(3)];
        assert_eq!(entry.name.value.as_str(), "Mac mini");
        assert_eq!(entry.joined_via, JoinedVia::IcloudKeychain);
        assert!(entry.notify_daily_insight.value && !entry.removed.value);
        assert!(!write_own_entry(&mut document, &own(3), &mut clock, NOW_MS).unwrap());

        let mut renamed = own(3);
        renamed.default_name = Some(name("Other"));
        renamed.server_device_id = server_device(9);
        assert!(write_own_entry(&mut document, &renamed, &mut clock, NOW_MS).unwrap());
        let entry = &document.devices[&device(3)];
        assert_eq!(
            entry.name.value.as_str(),
            "Mac mini",
            "the name is a register now"
        );
        assert_eq!(entry.server_device_id.value, server_device(9));

        assert!(mark_removed(&mut document, device(3), &mut clock, NOW_MS).unwrap());
        assert!(document.devices[&device(3)].removed.value);
        assert!(write_own_entry(&mut document, &renamed, &mut clock, NOW_MS).unwrap());
        assert!(!document.devices[&device(3)].removed.value, "rejoined");
    }

    fn quiet(configured_at: i64) -> VelvtQuietHours {
        VelvtQuietHours {
            start_local_minutes: 60,
            end_local_minutes: 120,
            rule_version: 1,
            configured_at: DateTime::from_timestamp(configured_at, 0).unwrap(),
        }
    }

    #[test]
    fn a_local_setting_is_written_only_when_newer_than_the_register_and_different() {
        let mut document = sample_document();
        let mut clock = HlcClock::new(device(3));
        clock.observe_document(&document);
        let register_at = (NOW_MS / 1_000) as i64;
        let older = LocalSyncSettings {
            quiet_hours: Some(quiet(register_at - 10)),
            invitations: Some((true, DateTime::from_timestamp(register_at - 10, 0).unwrap())),
        };
        assert!(!mirror_settings(&mut document, &older, &mut clock, NOW_MS).unwrap());

        let newer = LocalSyncSettings {
            quiet_hours: Some(quiet(register_at + 10)),
            invitations: Some((true, DateTime::from_timestamp(register_at + 10, 0).unwrap())),
        };
        assert!(mirror_settings(&mut document, &newer, &mut clock, NOW_MS + 20_000).unwrap());
        assert_eq!(
            document.settings.quiet_hours.as_ref().unwrap().value,
            Some(QuietHours {
                start_local_minutes: 60,
                end_local_minutes: 120,
                rule_version: 1
            })
        );
        assert!(
            document
                .settings
                .invitations_enabled
                .as_ref()
                .unwrap()
                .value
        );
        assert!(!mirror_settings(&mut document, &newer, &mut clock, NOW_MS + 30_000).unwrap());
        assert!(!mirror_settings(
            &mut document,
            &LocalSyncSettings::default(),
            &mut clock,
            NOW_MS
        )
        .unwrap());
    }

    #[test]
    fn rule_links_are_written_by_hlc_and_bad_links_are_skipped() {
        let mut document = PrefsDocument::new(salt(1));
        let link = |kind: &str, key: char, at: u64| VaultRuleLinkRecord {
            local_kind: kind.into(),
            local_key_hash: "0".repeat(64),
            person_key: key.to_string().repeat(64),
            category: "FOCUS_WORK".into(),
            deleted: false,
            hlc: hlc(at, 0, 3).to_string(),
        };
        let mut bad = link("site", 'b', NOW_MS);
        bad.category = "NOT_A_CATEGORY".into();
        let over = mirror_rule_links(
            &mut document,
            &[link("site", 'a', NOW_MS), bad, link("nope", 'c', NOW_MS)],
        );
        assert!(over.is_empty());
        assert_eq!(document.rules.len(), 1);
        let mut older = link("site", 'a', NOW_MS - 1);
        older.deleted = true;
        mirror_rule_links(&mut document, &[older]);
        assert!(!document.rules.values().next().unwrap().deleted);
        assert_eq!(held_summary(&document).site_rules, 1);
    }

    #[test]
    fn the_daily_insight_switch_is_this_mac_s_own_register() {
        let mut document = sample_document();
        assert!(daily_insight_here(&document, device(1)));
        document
            .devices
            .get_mut(&device(1))
            .unwrap()
            .notify_daily_insight
            .value = false;
        assert!(!daily_insight_here(&document, device(1)));
        assert!(daily_insight_here(&document, device(2)));
        assert!(
            daily_insight_here(&document, device(9)),
            "no entry: unchanged"
        );
    }
}
