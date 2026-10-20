//! The account's devices: the new-sign-in diff over `GET /v1/devices`, and
//! the rows of the device list (design J4, §10.4).
//!
//! `GET /v1/devices` gives server facts (id, status, when last seen, the app
//! version); the document gives names and routing. Rust joins them and writes
//! every line; Swift renders the rows.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, FixedOffset};

use crate::persistence::KnownDeviceRecord;
use crate::vault::doc::{PrefsDocument, SyncDeviceId};

use super::api::ServerDevice;

/// "Active now" means seen within this, velvt-core's `last_seen_at` write
/// interval (#41).
pub const ACTIVE_NOW_MS: i64 = 15 * 60 * 1_000;

/// What one `GET /v1/devices` changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceDiff {
    /// Rows to add to `sync_known_device`.
    pub to_record: Vec<KnownDeviceRecord>,
    /// Devices that signed in since this Mac last listed them: one
    /// `new_sign_in` each.
    pub signed_in: Vec<ServerDevice>,
}

/// The first list of a session seeds `sync_known_device` without alerting:
/// otherwise turning sync on would post one notice for every device already
/// on the account (F27). After that, an active device this Mac has not seen,
/// and that is not this Mac, posts once. A revoked device it has not seen is
/// recorded without one.
pub fn diff(known: &[KnownDeviceRecord], listed: &[ServerDevice], now_ms: i64) -> DeviceDiff {
    let seeding = known.is_empty();
    let known: BTreeSet<&str> = known
        .iter()
        .map(|device| device.server_device_id.as_str())
        .collect();
    let mut diff = DeviceDiff::default();
    for device in listed {
        if known.contains(device.id.as_str()) {
            continue;
        }
        let alert = !seeding && device.active && !device.is_current;
        diff.to_record.push(KnownDeviceRecord {
            server_device_id: device.id.clone(),
            first_seen_at_ms: now_ms,
            alerted_at_ms: alert.then_some(now_ms),
        });
        if alert {
            diff.signed_in.push(device.clone());
        }
    }
    diff
}

/// When a device was last seen, in J4's buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastSeen {
    ActiveNow,
    Today,
    Yesterday,
    DaysAgo(u32),
    OverAMonthAgo,
}

impl LastSeen {
    pub fn of(seen_ms: i64, now_ms: i64, utc_offset_seconds: i32) -> Self {
        if now_ms.saturating_sub(seen_ms) <= ACTIVE_NOW_MS {
            return Self::ActiveNow;
        }
        let local_day = |at_ms: i64| {
            let offset = FixedOffset::east_opt(utc_offset_seconds)
                .unwrap_or_else(|| FixedOffset::east_opt(0).expect("zero offset"));
            DateTime::from_timestamp_millis(at_ms)
                .map(|time| time.with_timezone(&offset).date_naive().num_days_from_ce())
                .unwrap_or_default()
        };
        match local_day(now_ms) - local_day(seen_ms) {
            days if days <= 0 => Self::Today,
            1 => Self::Yesterday,
            days @ 2..=30 => Self::DaysAgo(days.unsigned_abs()),
            _ => Self::OverAMonthAgo,
        }
    }

    pub fn phrase(self) -> String {
        match self {
            Self::ActiveNow => "Active now".into(),
            Self::Today => "Active today".into(),
            Self::Yesterday => "Active yesterday".into(),
            Self::DaysAgo(days) => format!("Active {days} days ago"),
            Self::OverAMonthAgo => "Active over a month ago".into(),
        }
    }
}

/// One row of the device list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub sync_device_id: Option<SyncDeviceId>,
    pub server_device_id: Option<String>,
    pub name: Option<String>,
    pub is_this_mac: bool,
    /// The row's Rust-authored line, such as "Active today · Syncing".
    pub status: String,
    pub last_seen: Option<LastSeen>,
    pub can_remove: bool,
    pub notify_daily_insight: Option<bool>,
}

/// The rows: every Mac in the document that is not removed, then every
/// signed-in device the document does not name. Removing needs a server
/// device id, and is never offered for this Mac (Turn Off Sync is).
pub fn rows(
    document: Option<&PrefsDocument>,
    this_mac: SyncDeviceId,
    listed: &[ServerDevice],
    now_ms: i64,
    utc_offset_seconds: i32,
) -> Vec<DeviceRow> {
    let mut rows = Vec::new();
    let mut named = BTreeSet::new();
    for (id, entry) in document
        .map(|document| document.devices.iter())
        .into_iter()
        .flatten()
    {
        if entry.removed.value {
            continue;
        }
        let server_id = entry.server_device_id.value.to_string();
        let server = listed.iter().find(|device| device.id == server_id);
        named.insert(server_id.clone());
        let is_this_mac = *id == this_mac;
        let last_seen = server
            .and_then(|device| device.last_seen_at_ms)
            .map(|seen| LastSeen::of(seen, now_ms, utc_offset_seconds));
        let status = if is_this_mac {
            "This Mac · Syncing".to_owned()
        } else {
            match (server, last_seen) {
                (Some(device), _) if !device.active => "Signed out · Syncing paused".to_owned(),
                (_, Some(seen)) => format!("{} · Syncing", seen.phrase()),
                _ => "Syncing".to_owned(),
            }
        };
        rows.push(DeviceRow {
            sync_device_id: Some(*id),
            can_remove: !is_this_mac && server.is_some_and(|device| device.active),
            server_device_id: Some(server_id),
            name: Some(entry.name.value.as_str().to_owned()),
            is_this_mac,
            status,
            last_seen,
            notify_daily_insight: Some(entry.notify_daily_insight.value),
        });
    }
    for device in listed {
        if named.contains(&device.id) || !device.active {
            continue;
        }
        let last_seen = device
            .last_seen_at_ms
            .map(|seen| LastSeen::of(seen, now_ms, utc_offset_seconds));
        let status = match last_seen {
            Some(seen) => format!("{} · Not syncing", seen.phrase()),
            None => "Signed in · Not syncing".to_owned(),
        };
        rows.push(DeviceRow {
            sync_device_id: None,
            server_device_id: Some(device.id.clone()),
            name: None,
            is_this_mac: device.is_current,
            status: if device.is_current {
                "This Mac · Not syncing".to_owned()
            } else {
                status
            },
            last_seen,
            can_remove: !device.is_current,
            notify_daily_insight: None,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{diff, rows, LastSeen};
    use crate::persistence::KnownDeviceRecord;
    use crate::vault::sync::api::ServerDevice;
    use crate::vault::test_support::{device, sample_document, NOW_MS};

    const NOW: i64 = NOW_MS as i64;
    const DAY: i64 = 86_400_000;

    fn listed(id: &str, active: bool, is_current: bool) -> ServerDevice {
        ServerDevice {
            id: id.into(),
            active,
            created_at_ms: NOW - 40 * DAY,
            last_seen_at_ms: Some(NOW - 3 * DAY),
            client_version: "1.0.14".into(),
            is_current,
        }
    }

    #[test]
    fn the_first_list_seeds_without_alerting_and_later_sign_ins_alert_once() {
        let first = [listed("a", true, true), listed("b", true, false)];
        let seeded = diff(&[], &first, NOW);
        assert!(seeded.signed_in.is_empty());
        assert_eq!(seeded.to_record.len(), 2);
        assert!(seeded
            .to_record
            .iter()
            .all(|row| row.alerted_at_ms.is_none()));

        let later = [
            listed("a", true, true),
            listed("b", true, false),
            listed("c", true, false),
            listed("d", false, false),
        ];
        let next = diff(&seeded.to_record, &later, NOW + 1);
        assert_eq!(next.signed_in, vec![listed("c", true, false)]);
        assert_eq!(next.to_record.len(), 2, "c alerted, d recorded silently");
        assert_eq!(next.to_record[0].alerted_at_ms, Some(NOW + 1));
        assert_eq!(next.to_record[1].alerted_at_ms, None);

        let mut known: Vec<KnownDeviceRecord> = seeded.to_record;
        known.extend(next.to_record);
        assert_eq!(diff(&known, &later, NOW + 2), Default::default());
    }

    #[test]
    fn last_seen_uses_the_design_s_buckets_in_local_days() {
        assert_eq!(LastSeen::of(NOW - 60_000, NOW, 0), LastSeen::ActiveNow);
        assert_eq!(
            LastSeen::of(NOW + 3_600_000, NOW + 3_600_000 * 5, 0),
            LastSeen::Today
        );
        assert_eq!(LastSeen::of(NOW - DAY, NOW, 0), LastSeen::Yesterday);
        assert_eq!(LastSeen::of(NOW - 3 * DAY, NOW, 0), LastSeen::DaysAgo(3));
        assert_eq!(
            LastSeen::of(NOW - 45 * DAY, NOW, 0),
            LastSeen::OverAMonthAgo
        );
        // 00:00Z on the 15th is still the 14th in Toronto.
        assert_eq!(
            LastSeen::of(NOW - 2 * 3_600_000, NOW + 3_600_000, -4 * 3_600),
            LastSeen::Today
        );
    }

    #[test]
    fn rows_join_document_names_with_server_facts() {
        let document = sample_document();
        let ids: Vec<String> = document
            .devices
            .values()
            .map(|entry| entry.server_device_id.value.to_string())
            .collect();
        let listed = [
            listed(&ids[0], true, true),
            listed(&ids[1], true, false),
            listed("8f14e45f-ceea-467a-9575-1f2ab6c9d0e1", true, false),
        ];
        let rows = rows(Some(&document), device(1), &listed, NOW, 0);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].is_this_mac && !rows[0].can_remove);
        assert_eq!(rows[0].status, "This Mac · Syncing");
        assert_eq!(rows[1].status, "Active 3 days ago · Syncing");
        assert!(rows[1].can_remove);
        assert_eq!(rows[2].name, None);
        assert_eq!(rows[2].status, "Active 3 days ago · Not syncing");
    }
}
