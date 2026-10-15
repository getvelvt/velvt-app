//! Security notices (design J6): the copy, and the rule that none is posted
//! during a work block.
//!
//! Clicking a notification switches apps, and the drift gate counts app
//! switches, so a notice that came due mid-block would change drift evidence
//! (§11). Rust holds each one until the block ends, as it holds the
//! needs-a-category reminder; the device list shows it at once. Each is
//! posted once and listed until acknowledged.
//!
//! All copy is written here (I7). It names no site, app or window, and passes
//! `scripts/check_banned_copy.py`.

use chrono::{DateTime, FixedOffset, Utc};

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

pub fn new_sign_in(server_device_id: &str, at_ms: i64, time: LocalTime) -> SyncNotice {
    SyncNotice {
        notice_id: format!("new_sign_in:{server_device_id}"),
        kind: NoticeKind::NewSignIn,
        title: "New sign-in to your Velvt account".into(),
        body: format!(
            "A Mac signed in at {}. If this wasn't you, change your password and remove it in Settings › Your Macs.",
            time.clock(at_ms)
        ),
        actions: Vec::new(),
    }
}

pub fn mac_joined_sync(
    sync_device_id: &str,
    name: &str,
    route: JoinRoute,
    at_ms: i64,
    time: LocalTime,
) -> SyncNotice {
    let route = match route {
        JoinRoute::ICloudKeychain => "using your iCloud Keychain",
        JoinRoute::SetupCode => "using a setup code",
    };
    SyncNotice {
        notice_id: format!("mac_joined_sync:{sync_device_id}"),
        kind: NoticeKind::MacJoinedSync,
        title: "A Mac joined your sync".into(),
        body: format!(
            "“{name}” turned on sync at {} {route}. Not you? {REMOVE_IT}",
            time.clock(at_ms)
        ),
        actions: Vec::new(),
    }
}

/// velvt-core does not say which Mac removed this one, and the document is no
/// longer served to it, so the copy names no Mac.
pub fn this_mac_removed(at_ms: i64) -> SyncNotice {
    SyncNotice {
        notice_id: format!("this_mac_removed:{at_ms}"),
        kind: NoticeKind::ThisMacRemoved,
        title: "This Mac was removed".into(),
        body: "This Mac was removed from your Velvt account. Its own categories and settings stay on this Mac. Sign in to keep using Velvt.".into(),
        actions: Vec::new(),
    }
}

/// A rotation statement names no Mac (§5.3), so this one says "another of your
/// Macs".
pub fn sync_key_changed(
    new_key_id: &str,
    adopted: bool,
    at_ms: i64,
    time: LocalTime,
) -> SyncNotice {
    let then = if adopted {
        "This Mac picked up the new key."
    } else {
        "Add this Mac with a code from one of your other Macs."
    };
    SyncNotice {
        notice_id: format!("sync_key_changed:{new_key_id}"),
        kind: NoticeKind::SyncKeyChanged,
        title: "Your sync key changed".into(),
        body: format!(
            "Another of your Macs changed the sync key at {}. {then}",
            time.clock(at_ms)
        ),
        actions: Vec::new(),
    }
}

pub fn reset_requested(
    reset_id: &str,
    requested_at_ms: i64,
    effective_at_ms: i64,
    time: LocalTime,
) -> SyncNotice {
    SyncNotice {
        notice_id: format!("reset_requested:{reset_id}"),
        kind: NoticeKind::ResetRequested,
        title: "Someone asked to reset your synced copy".into(),
        body: format!(
            "A reset of your synced copy was requested at {} without your sync key. It happens on {} at {} unless you cancel. If this wasn't you, cancel it and change your password.",
            time.clock(requested_at_ms),
            time.day(effective_at_ms),
            time.clock(effective_at_ms),
        ),
        actions: vec![NoticeAction::CancelReset],
    }
}

pub fn reset_completed(
    reset_id: &str,
    effective_at_ms: i64,
    while_offline: bool,
    time: LocalTime,
) -> SyncNotice {
    let when = if while_offline {
        format!(
            "Your synced copy was reset on {} after a 3-day notice, while this Mac couldn't reach Velvt.",
            time.day(effective_at_ms)
        )
    } else {
        format!(
            "Your synced copy was reset on {} after a 3-day notice.",
            time.day(effective_at_ms)
        )
    };
    SyncNotice {
        notice_id: format!("reset_completed:{reset_id}"),
        kind: NoticeKind::ResetCompleted,
        title: "Your synced copy was reset".into(),
        body: format!("{when} This Mac kept its own choices."),
        actions: vec![NoticeAction::SetUpSync],
    }
}

/// Never offers code entry or Delete Synced Data (§6.1).
pub fn server_inconsistent(discriminator: &str) -> SyncNotice {
    SyncNotice {
        notice_id: format!("server_inconsistent:{discriminator}"),
        kind: NoticeKind::ServerInconsistent,
        title: "Sync paused to keep your data safe".into(),
        body: "Velvt's server showed this Mac a change to your synced copy that none of your Macs signed. Nothing on this Mac changed. Sync is paused.".into(),
        actions: vec![NoticeAction::HowThisWorks],
    }
}

#[derive(Debug, Clone)]
struct Queued {
    notice: SyncNotice,
    posted: bool,
    acknowledged: bool,
}

/// The notices this run of the service has produced.
#[derive(Debug, Clone, Default)]
pub struct NoticeQueue {
    queued: Vec<Queued>,
}

impl NoticeQueue {
    /// Queues `notice` unless one with its id already is.
    pub fn push(&mut self, notice: SyncNotice) {
        if self
            .queued
            .iter()
            .all(|queued| queued.notice.notice_id != notice.notice_id)
        {
            self.queued.push(Queued {
                notice,
                posted: false,
                acknowledged: false,
            });
        }
    }

    /// The notices due to be posted, each once. None while a work block is
    /// active: they wait for it to end (J6, §11.3).
    pub fn take_due(&mut self, block_active: bool) -> Vec<SyncNotice> {
        if block_active {
            return Vec::new();
        }
        self.queued
            .iter_mut()
            .filter(|queued| !queued.posted && !queued.acknowledged)
            .map(|queued| {
                queued.posted = true;
                queued.notice.clone()
            })
            .collect()
    }

    pub fn acknowledge(&mut self, notice_id: &str) {
        for queued in &mut self.queued {
            if queued.notice.notice_id == notice_id {
                queued.acknowledged = true;
            }
        }
    }

    /// What the device list shows: every notice not acknowledged, posted or
    /// held.
    pub fn listed(&self) -> Vec<SyncNotice> {
        self.queued
            .iter()
            .filter(|queued| !queued.acknowledged)
            .map(|queued| queued.notice.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        mac_joined_sync, new_sign_in, reset_completed, reset_requested, server_inconsistent,
        sync_key_changed, this_mac_removed, JoinRoute, LocalTime, NoticeQueue,
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
        queue.push(this_mac_removed(AT));
        queue.push(this_mac_removed(AT));
        assert!(queue.take_due(true).is_empty(), "held during a block");
        assert_eq!(queue.listed().len(), 1, "but listed at once");
        assert_eq!(queue.take_due(false).len(), 1);
        assert!(queue.take_due(false).is_empty(), "posted once");
        assert_eq!(queue.listed().len(), 1, "listed until acknowledged");
        queue.acknowledge(&this_mac_removed(AT).notice_id);
        assert!(queue.listed().is_empty());
    }
}
