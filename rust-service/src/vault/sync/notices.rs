//! Security notices (design J6): the copy, and the rule that none is posted
//! during a work block.
//!
//! Clicking a notification switches apps, and the drift gate counts app
//! switches, so a notice that came due mid-block would change drift evidence
//! (§11). Rust holds each one until the block ends, as it holds the
//! needs-a-category reminder; the device list shows it at once. Each is
//! posted once and listed until acknowledged.
//!
//! The queue outlives a restart: the engine keeps each notice's
//! [`NoticeEvent`] in `sync_notice` (migration 0046) when it is queued, with
//! when it was posted and acknowledged, so one held by a work block when the
//! service restarts is still posted when the block ends. The row holds ids
//! and times only. The copy is written when the notice is shown, and a Mac's
//! name is read then from the synced copy, so no name reaches the table.
//!
//! All copy is written here (I7). It names no site, app or window, and passes
//! `scripts/check_banned_copy.py`.

use chrono::{DateTime, FixedOffset, Utc};

use crate::persistence::SyncNoticeRecord;

/// The fourth notification kind's variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    NewSignIn,
    MacJoinedSync,
    ThisMacRemoved,
    SyncKeyChanged,
    ResetRequested,
    ResetCompleted,
    ServerInconsistent,
}

impl NoticeKind {
    pub const ALL: [Self; 7] = [
        Self::NewSignIn,
        Self::MacJoinedSync,
        Self::ThisMacRemoved,
        Self::SyncKeyChanged,
        Self::ResetRequested,
        Self::ResetCompleted,
        Self::ServerInconsistent,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewSignIn => "new_sign_in",
            Self::MacJoinedSync => "mac_joined_sync",
            Self::ThisMacRemoved => "this_mac_removed",
            Self::SyncKeyChanged => "sync_key_changed",
            Self::ResetRequested => "reset_requested",
            Self::ResetCompleted => "reset_completed",
            Self::ServerInconsistent => "server_inconsistent",
        }
    }
}

/// What a notice offers. Swift writes the button labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeAction {
    CancelReset,
    SetUpSync,
    HowThisWorks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncNotice {
    /// Stable for one event, so a notice is never queued twice.
    pub notice_id: String,
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    pub actions: Vec<NoticeAction>,
}

/// How a Mac got the sync key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinRoute {
    ICloudKeychain,
    SetupCode,
}

/// Local time for notice copy: `10:42` and `18 Oct`.
#[derive(Debug, Clone, Copy)]
pub struct LocalTime {
    pub utc_offset_seconds: i32,
}

impl LocalTime {
    fn at(&self, at_ms: i64) -> Option<DateTime<FixedOffset>> {
        let offset =
            FixedOffset::east_opt(self.utc_offset_seconds).or_else(|| FixedOffset::east_opt(0))?;
        DateTime::<Utc>::from_timestamp_millis(at_ms).map(|time| time.with_timezone(&offset))
    }

    pub fn clock(&self, at_ms: i64) -> String {
        self.at(at_ms)
            .map(|time| time.format("%H:%M").to_string())
            .unwrap_or_default()
    }

    pub fn day(&self, at_ms: i64) -> String {
        self.at(at_ms)
            .map(|time| time.format("%-d %b").to_string())
            .unwrap_or_default()
    }
}

const REMOVE_IT: &str = "Remove it in Settings › Your Macs.";

/// What a notice is about: ids and times, never a name or other content.
/// This is what `sync_notice` keeps; [`NoticeEvent::render`] writes the copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeEvent {
    NewSignIn {
        server_device_id: String,
        at_ms: i64,
    },
    /// The Mac's name is read from the synced copy when the notice is shown.
    MacJoinedSync {
        sync_device_id: String,
        route: JoinRoute,
        at_ms: i64,
    },
    ThisMacRemoved {
        at_ms: i64,
    },
    SyncKeyChanged {
        new_key_id: String,
        adopted: bool,
        at_ms: i64,
    },
    ResetRequested {
        reset_id: String,
        requested_at_ms: i64,
        effective_at_ms: i64,
    },
    ResetCompleted {
        reset_id: String,
        effective_at_ms: i64,
        while_offline: bool,
    },
    /// `discriminator` is the inconsistency's code and the trusted key id.
    ServerInconsistent {
        discriminator: String,
    },
}

impl NoticeEvent {
    pub fn kind(&self) -> NoticeKind {
        match self {
            Self::NewSignIn { .. } => NoticeKind::NewSignIn,
            Self::MacJoinedSync { .. } => NoticeKind::MacJoinedSync,
            Self::ThisMacRemoved { .. } => NoticeKind::ThisMacRemoved,
            Self::SyncKeyChanged { .. } => NoticeKind::SyncKeyChanged,
            Self::ResetRequested { .. } => NoticeKind::ResetRequested,
            Self::ResetCompleted { .. } => NoticeKind::ResetCompleted,
            Self::ServerInconsistent { .. } => NoticeKind::ServerInconsistent,
        }
    }

    /// The id the notice is about, if any: a velvt-core device id, a sync
    /// device id, a key id, a reset id, or the inconsistency's discriminator.
    fn subject(&self) -> Option<&str> {
        match self {
            Self::NewSignIn {
                server_device_id, ..
            } => Some(server_device_id),
            Self::MacJoinedSync { sync_device_id, .. } => Some(sync_device_id),
            Self::ThisMacRemoved { .. } => None,
            Self::SyncKeyChanged { new_key_id, .. } => Some(new_key_id),
            Self::ResetRequested { reset_id, .. } | Self::ResetCompleted { reset_id, .. } => {
                Some(reset_id)
            }
            Self::ServerInconsistent { discriminator } => Some(discriminator),
        }
    }

    /// Stable for one event, so a notice is never queued twice.
    pub fn notice_id(&self) -> String {
        match self {
            Self::ThisMacRemoved { at_ms } => format!("this_mac_removed:{at_ms}"),
            other => format!(
                "{}:{}",
                other.kind().as_str(),
                other.subject().unwrap_or_default()
            ),
        }
    }

    /// The copy. `name` is the Mac's name in the synced copy, for
    /// `mac_joined_sync`; without one the copy says "A Mac".
    pub fn render(&self, name: Option<&str>, time: LocalTime) -> SyncNotice {
        let notice_id = self.notice_id();
        let kind = self.kind();
        let (title, body, actions) = match self {
            Self::NewSignIn { at_ms, .. } => (
                "New sign-in to your Velvt account",
                format!(
                    "A Mac signed in at {}. If this wasn't you, change your password and remove it in Settings › Your Macs.",
                    time.clock(*at_ms)
                ),
                Vec::new(),
            ),
            Self::MacJoinedSync { route, at_ms, .. } => {
                let route = match route {
                    JoinRoute::ICloudKeychain => "using your iCloud Keychain",
                    JoinRoute::SetupCode => "using a setup code",
                };
                let who = name.map_or_else(|| "A Mac".to_owned(), |name| format!("“{name}”"));
                (
                    "A Mac joined your sync",
                    format!(
                        "{who} turned on sync at {} {route}. Not you? {REMOVE_IT}",
                        time.clock(*at_ms)
                    ),
                    Vec::new(),
                )
            }
            // velvt-core does not say which Mac removed this one, and the
            // document is no longer served to it, so the copy names no Mac.
            Self::ThisMacRemoved { .. } => (
                "This Mac was removed",
                "This Mac was removed from your Velvt account. Its own categories and settings stay on this Mac. Sign in to keep using Velvt.".to_owned(),
                Vec::new(),
            ),
            // A rotation statement names no Mac (§5.3), so this one says
            // "another of your Macs".
            Self::SyncKeyChanged { adopted, at_ms, .. } => {
                let then = if *adopted {
                    "This Mac picked up the new key."
                } else {
                    "Add this Mac with a code from one of your other Macs."
                };
                (
                    "Your sync key changed",
                    format!(
                        "Another of your Macs changed the sync key at {}. {then}",
                        time.clock(*at_ms)
                    ),
                    Vec::new(),
                )
            }
            Self::ResetRequested {
                requested_at_ms,
                effective_at_ms,
                ..
            } => (
                "Someone asked to reset your synced copy",
                format!(
                    "A reset of your synced copy was requested at {} without your sync key. It happens on {} at {} unless you cancel. If this wasn't you, cancel it and change your password.",
                    time.clock(*requested_at_ms),
                    time.day(*effective_at_ms),
                    time.clock(*effective_at_ms),
                ),
                vec![NoticeAction::CancelReset],
            ),
            Self::ResetCompleted {
                effective_at_ms,
                while_offline,
                ..
            } => {
                let when = if *while_offline {
                    format!(
                        "Your synced copy was reset on {} after a 3-day notice, while this Mac couldn't reach Velvt.",
                        time.day(*effective_at_ms)
                    )
                } else {
                    format!(
                        "Your synced copy was reset on {} after a 3-day notice.",
                        time.day(*effective_at_ms)
                    )
                };
                (
                    "Your synced copy was reset",
                    format!("{when} This Mac kept its own choices."),
                    vec![NoticeAction::SetUpSync],
                )
            }
            // Never offers code entry or Delete Synced Data (§6.1).
            Self::ServerInconsistent { .. } => (
                "Sync paused to keep your data safe",
                "Velvt's server showed this Mac a change to your synced copy that none of your Macs signed. Nothing on this Mac changed. Sync is paused.".to_owned(),
                vec![NoticeAction::HowThisWorks],
            ),
        };
        SyncNotice {
            notice_id,
            kind,
            title: title.into(),
            body,
            actions,
        }
    }

    /// `sync_notice`'s columns: `at` is the time the copy names first,
    /// `until_at` the second (a reset's effective time), `flag` the one
    /// switch the copy reads (adopted, while offline, joined by code).
    fn columns(&self) -> (Option<String>, i64, Option<i64>, bool) {
        let subject = self.subject().map(str::to_owned);
        match self {
            Self::NewSignIn { at_ms, .. } | Self::ThisMacRemoved { at_ms } => {
                (subject, *at_ms, None, false)
            }
            Self::MacJoinedSync { route, at_ms, .. } => {
                (subject, *at_ms, None, *route == JoinRoute::SetupCode)
            }
            Self::SyncKeyChanged { adopted, at_ms, .. } => (subject, *at_ms, None, *adopted),
            Self::ResetRequested {
                requested_at_ms,
                effective_at_ms,
                ..
            } => (subject, *requested_at_ms, Some(*effective_at_ms), false),
            Self::ResetCompleted {
                effective_at_ms,
                while_offline,
                ..
            } => (subject, *effective_at_ms, None, *while_offline),
            Self::ServerInconsistent { .. } => (subject, 0, None, false),
        }
    }

    /// The event a `sync_notice` row keeps, or `None` for a row this build
    /// cannot read.
    pub fn from_record(record: &SyncNoticeRecord) -> Option<Self> {
        let subject = || record.subject.clone();
        let event = match NoticeKind::parse(&record.kind)? {
            NoticeKind::NewSignIn => Self::NewSignIn {
                server_device_id: subject()?,
                at_ms: record.at_ms,
            },
            NoticeKind::MacJoinedSync => Self::MacJoinedSync {
                sync_device_id: subject()?,
                route: if record.flag {
                    JoinRoute::SetupCode
                } else {
                    JoinRoute::ICloudKeychain
                },
                at_ms: record.at_ms,
            },
            NoticeKind::ThisMacRemoved => Self::ThisMacRemoved {
                at_ms: record.at_ms,
            },
            NoticeKind::SyncKeyChanged => Self::SyncKeyChanged {
                new_key_id: subject()?,
                adopted: record.flag,
                at_ms: record.at_ms,
            },
            NoticeKind::ResetRequested => Self::ResetRequested {
                reset_id: subject()?,
                requested_at_ms: record.at_ms,
                effective_at_ms: record.until_ms?,
            },
            NoticeKind::ResetCompleted => Self::ResetCompleted {
                reset_id: subject()?,
                effective_at_ms: record.at_ms,
                while_offline: record.flag,
            },
            NoticeKind::ServerInconsistent => Self::ServerInconsistent {
                discriminator: subject()?,
            },
        };
        (event.notice_id() == record.notice_id).then_some(event)
    }
}

pub fn new_sign_in(server_device_id: &str, at_ms: i64, time: LocalTime) -> SyncNotice {
    NoticeEvent::NewSignIn {
        server_device_id: server_device_id.to_owned(),
        at_ms,
    }
    .render(None, time)
}

pub fn mac_joined_sync(
    sync_device_id: &str,
    name: &str,
    route: JoinRoute,
    at_ms: i64,
    time: LocalTime,
) -> SyncNotice {
    NoticeEvent::MacJoinedSync {
        sync_device_id: sync_device_id.to_owned(),
        route,
        at_ms,
    }
    .render(Some(name), time)
}

pub fn this_mac_removed(at_ms: i64) -> SyncNotice {
    NoticeEvent::ThisMacRemoved { at_ms }.render(
        None,
        LocalTime {
            utc_offset_seconds: 0,
        },
    )
}

pub fn sync_key_changed(
    new_key_id: &str,
    adopted: bool,
    at_ms: i64,
    time: LocalTime,
) -> SyncNotice {
    NoticeEvent::SyncKeyChanged {
        new_key_id: new_key_id.to_owned(),
        adopted,
        at_ms,
    }
    .render(None, time)
}

pub fn reset_requested(
    reset_id: &str,
    requested_at_ms: i64,
    effective_at_ms: i64,
    time: LocalTime,
) -> SyncNotice {
    NoticeEvent::ResetRequested {
        reset_id: reset_id.to_owned(),
        requested_at_ms,
        effective_at_ms,
    }
    .render(None, time)
}

pub fn reset_completed(
    reset_id: &str,
    effective_at_ms: i64,
    while_offline: bool,
    time: LocalTime,
) -> SyncNotice {
    NoticeEvent::ResetCompleted {
        reset_id: reset_id.to_owned(),
        effective_at_ms,
        while_offline,
    }
    .render(None, time)
}

pub fn server_inconsistent(discriminator: &str) -> SyncNotice {
    NoticeEvent::ServerInconsistent {
        discriminator: discriminator.to_owned(),
    }
    .render(
        None,
        LocalTime {
            utc_offset_seconds: 0,
        },
    )
}

#[derive(Debug, Clone)]
struct Queued {
    event: NoticeEvent,
    notice: SyncNotice,
    queued_at_ms: i64,
    posted_at_ms: Option<i64>,
    acknowledged_at_ms: Option<i64>,
}

impl Queued {
    fn record(&self, user_id: &str) -> SyncNoticeRecord {
        let (subject, at_ms, until_ms, flag) = self.event.columns();
        SyncNoticeRecord {
            user_id: user_id.to_owned(),
            notice_id: self.notice.notice_id.clone(),
            kind: self.event.kind().as_str().to_owned(),
            subject,
            at_ms,
            until_ms,
            flag,
            queued_at_ms: self.queued_at_ms,
            posted_at_ms: self.posted_at_ms,
            acknowledged_at_ms: self.acknowledged_at_ms,
        }
    }
}

/// The notices owed to the signed-in account. The engine writes each
/// change through to `sync_notice` (the records these methods return).
#[derive(Debug, Clone, Default)]
pub struct NoticeQueue {
    queued: Vec<Queued>,
}

impl NoticeQueue {
    /// Queues `event`, rendered as `notice`, unless it already is (posted,
    /// held or acknowledged). Returns the row to keep when it is new.
    pub fn push(
        &mut self,
        user_id: &str,
        event: NoticeEvent,
        notice: SyncNotice,
        now_ms: i64,
    ) -> Option<SyncNoticeRecord> {
        if self
            .queued
            .iter()
            .any(|queued| queued.notice.notice_id == notice.notice_id)
        {
            return None;
        }
        let queued = Queued {
            event,
            notice,
            queued_at_ms: now_ms,
            posted_at_ms: None,
            acknowledged_at_ms: None,
        };
        let record = queued.record(user_id);
        self.queued.push(queued);
        Some(record)
    }

    /// A notice kept from an earlier run, rendered again now.
    pub fn restore(&mut self, record: &SyncNoticeRecord, event: NoticeEvent, notice: SyncNotice) {
        if self
            .queued
            .iter()
            .all(|queued| queued.notice.notice_id != record.notice_id)
        {
            self.queued.push(Queued {
                event,
                notice,
                queued_at_ms: record.queued_at_ms,
                posted_at_ms: record.posted_at_ms,
                acknowledged_at_ms: record.acknowledged_at_ms,
            });
        }
    }

    /// The notices due to be posted, each once, and the rows that record
    /// them as posted. None while a work block is active: they wait for it
    /// to end (J6, §11.3).
    pub fn take_due(
        &mut self,
        user_id: &str,
        block_active: bool,
        now_ms: i64,
    ) -> (Vec<SyncNotice>, Vec<SyncNoticeRecord>) {
        if block_active {
            return (Vec::new(), Vec::new());
        }
        self.queued
            .iter_mut()
            .filter(|queued| queued.posted_at_ms.is_none() && queued.acknowledged_at_ms.is_none())
            .map(|queued| {
                queued.posted_at_ms = Some(now_ms);
                (queued.notice.clone(), queued.record(user_id))
            })
            .unzip()
    }

    /// Returns the row to keep, if the notice was listed.
    pub fn acknowledge(
        &mut self,
        user_id: &str,
        notice_id: &str,
        now_ms: i64,
    ) -> Option<SyncNoticeRecord> {
        let queued = self.queued.iter_mut().find(|queued| {
            queued.notice.notice_id == notice_id && queued.acknowledged_at_ms.is_none()
        })?;
        queued.acknowledged_at_ms = Some(now_ms);
        Some(queued.record(user_id))
    }

    /// What the device list shows: every notice not acknowledged, posted or
    /// held.
    pub fn listed(&self) -> Vec<SyncNotice> {
        self.queued
            .iter()
            .filter(|queued| queued.acknowledged_at_ms.is_none())
            .map(|queued| queued.notice.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        mac_joined_sync, new_sign_in, reset_completed, reset_requested, server_inconsistent,
        sync_key_changed, this_mac_removed, JoinRoute, LocalTime, NoticeEvent, NoticeQueue,
    };
    use crate::work_block::BANNED_COPY_TOKENS;

    const AT: i64 = 1_760_524_920_000; // 2025-10-15T10:42:00Z

    fn every_notice() -> Vec<super::SyncNotice> {
        let time = LocalTime {
            utc_offset_seconds: 0,
        };
        vec![
            new_sign_in("d", AT, time),
            mac_joined_sync("s", "Mac mini", JoinRoute::SetupCode, AT, time),
            mac_joined_sync("s", "Mac mini", JoinRoute::ICloudKeychain, AT, time),
            this_mac_removed(AT),
            sync_key_changed("k", true, AT, time),
            sync_key_changed("k", false, AT, time),
            reset_requested("r", AT, AT + 72 * 3_600_000, time),
            reset_completed("r", AT, true, time),
            reset_completed("r", AT, false, time),
            server_inconsistent("x"),
            NoticeEvent::MacJoinedSync {
                sync_device_id: "s".into(),
                route: JoinRoute::SetupCode,
                at_ms: AT,
            }
            .render(None, time),
        ]
    }

    #[test]
    fn the_copy_reads_the_design_s_times_and_names_no_mac_it_cannot_know() {
        let time = LocalTime {
            utc_offset_seconds: 0,
        };
        assert_eq!(
            mac_joined_sync("s", "Mac mini", JoinRoute::SetupCode, AT, time).body,
            "“Mac mini” turned on sync at 10:42 using a setup code. Not you? Remove it in Settings › Your Macs."
        );
        assert_eq!(
            reset_requested("r", AT, AT + 72 * 3_600_000, time).body,
            "A reset of your synced copy was requested at 10:42 without your sync key. It happens on 18 Oct at 10:42 unless you cancel. If this wasn't you, cancel it and change your password."
        );
        let toronto = LocalTime {
            utc_offset_seconds: -4 * 3_600,
        };
        assert!(new_sign_in("d", AT, toronto).body.contains("at 06:42."));
    }

    /// The rendered copy, not only the literals the script lexes: the
    /// product's honesty rule and the work-block voice list.
    #[test]
    fn no_notice_uses_banned_copy() {
        for notice in every_notice() {
            for text in [&notice.title, &notice.body] {
                let lower = text.to_lowercase();
                for banned in ["learn", "adapt", "predict", "smarter"]
                    .iter()
                    .chain(BANNED_COPY_TOKENS)
                {
                    assert!(!lower.contains(banned), "{banned:?} in {text:?}");
                }
            }
        }
    }

    #[test]
    fn a_notice_waits_for_the_block_to_end_and_is_posted_once() {
        let mut queue = NoticeQueue::default();
        let event = NoticeEvent::ThisMacRemoved { at_ms: AT };
        let first = queue.push("u", event.clone(), this_mac_removed(AT), AT);
        assert!(first.is_some_and(|record| record.posted_at_ms.is_none()));
        assert!(queue
            .push("u", event, this_mac_removed(AT), AT + 1)
            .is_none());
        assert!(
            queue.take_due("u", true, AT).0.is_empty(),
            "held during a block"
        );
        assert_eq!(queue.listed().len(), 1, "but listed at once");
        let (posted, records) = queue.take_due("u", false, AT + 5);
        assert_eq!(posted.len(), 1);
        assert_eq!(records[0].posted_at_ms, Some(AT + 5));
        assert!(
            queue.take_due("u", false, AT + 6).0.is_empty(),
            "posted once"
        );
        assert_eq!(queue.listed().len(), 1, "listed until acknowledged");
        assert!(queue
            .acknowledge("u", &this_mac_removed(AT).notice_id, AT + 7)
            .is_some());
        assert!(queue.listed().is_empty());
    }

    /// What `sync_notice` keeps comes back as the same notice, and holds no
    /// name: the joined Mac's name is read from the synced copy when shown.
    #[test]
    fn every_notice_survives_its_row_and_the_row_holds_no_name() {
        let time = LocalTime {
            utc_offset_seconds: 0,
        };
        let events = vec![
            NoticeEvent::NewSignIn {
                server_device_id: "8f14e45f-ceea-467a-9575-1f2ab6c9d0e1".into(),
                at_ms: AT,
            },
            NoticeEvent::MacJoinedSync {
                sync_device_id: "45c48cce-2e2d-4fbd-b6a0-3f0bd1b6f5e2".into(),
                route: JoinRoute::SetupCode,
                at_ms: AT,
            },
            NoticeEvent::ThisMacRemoved { at_ms: AT },
            NoticeEvent::SyncKeyChanged {
                new_key_id: "0123456789abcdef".into(),
                adopted: true,
                at_ms: AT,
            },
            NoticeEvent::ResetRequested {
                reset_id: "6f9619ff-8b86-d011-b42d-00c04fc964ff".into(),
                requested_at_ms: AT,
                effective_at_ms: AT + 72 * 3_600_000,
            },
            NoticeEvent::ResetCompleted {
                reset_id: "6f9619ff-8b86-d011-b42d-00c04fc964ff".into(),
                effective_at_ms: AT,
                while_offline: true,
            },
            NoticeEvent::ServerInconsistent {
                discriminator: "unseen_reset:0123456789abcdef".into(),
            },
        ];
        for event in events {
            let mut queue = NoticeQueue::default();
            let record = queue
                .push("u", event.clone(), event.render(Some("Mac mini"), time), AT)
                .unwrap();
            assert!(!format!("{record:?}").contains("Mac mini"));
            assert_eq!(NoticeEvent::from_record(&record), Some(event.clone()));
        }
        let joined = NoticeEvent::MacJoinedSync {
            sync_device_id: "s".into(),
            route: JoinRoute::ICloudKeychain,
            at_ms: AT,
        };
        assert_eq!(
            joined.render(None, time).body,
            "A Mac turned on sync at 10:42 using your iCloud Keychain. Not you? Remove it in Settings › Your Macs."
        );
    }
}
