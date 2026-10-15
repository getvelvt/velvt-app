//! The state machine of design §10.4, over the checks of §6.1.
//!
//! One account at a time: the one the service is signed in to. Its
//! `vault_state` row is the trust anchor (kept across Log Out, Turn Off Sync,
//! removal and an account switch) and the session (cleared on each). The
//! plaintext document exists only in memory; the row keeps the last copy
//! opened, still sealed.
//!
//! Rules that hold on every path (I6): a copy that did not open and validate
//! is never overwritten; a 404 never creates a vault on its own; a new key is
//! adopted only through a verified rotation, or after a reset §6.1 accepts
//! and only from this Mac's own Keychain or a typed code; a key an item this
//! Mac holds supersedes or retires is never adopted; and no server claim
//! alone deletes or rewrites a Keychain item. A typed code is taken only in a
//! state that offers one, and only for a vault the pass would adopt (I3).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::auth::{retry_after_delay, HttpClient};
use crate::persistence::{VaultRuleLinkRecord, VaultStateRecord, VaultStateRepo};
use crate::vault::crypto::statement::SignedRotation;
use crate::vault::crypto::statement::{CreateStatement, PutStatement};
use crate::vault::crypto::{
    sha256, BlobName, DocumentContext, KeyId, PersonSalt, UserId, VaultKey, WritePublicKey,
};
use crate::vault::doc::{
    self, DecodedDocument, DeviceName, JoinedVia, NewerSchemaView, PrefsDocument, ServerDeviceId,
    SyncDeviceId, SCHEMA_V1,
};
use crate::vault::merge::{collect_garbage, merge, merge_older_copy, Hlc, HlcClock};

use super::api::{
    ApiError, BlobFetch, CreateBody, PutBody, ServerDevice, ServerReset, VaultApi, VaultView,
    WriteOutcome,
};
use super::devices::{self, DeviceRow};
use super::host::{self, HostIdentity};
use super::keys::{KeyInventory, KeyStoreError, RetiredBy, VaultKeyStore};
use super::mirror::{self, HeldSummary, OwnEntry};
use super::notices::{JoinRoute, LocalTime, NoticeEvent, NoticeQueue, SyncNotice};
use super::verify::{
    judge_blob, judge_vault, Anchor, BlobJudgement, Inconsistency, SeenReset, Trusted,
    VaultJudgement,
};
use super::{
    BlockActivity, SyncClock, SyncRouting, SyncState, CONFLICT_ATTEMPTS, CONFLICT_BACKOFF_MS,
    RETIRED_KEY_GRACE_MS,
};

/// A host binding made while the host UUID could not be read is this UUID's:
/// the first launch that can read it binds the row for real.
const UNKNOWN_HOST: [u8; 16] = [0; 16];

/// An acknowledged notice's row is kept this long, so a reset still pending
/// is not posted again after a restart, then deleted.
const ACKNOWLEDGED_NOTICE_KEPT_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Who deleted the account (J10, §8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountDeletion {
    /// This Mac's own Delete Account: every key item held for the account
    /// is deleted, retired ones included, and the row with its anchor.
    ThisMac,
    /// velvt-core said the account is gone (a deletion made elsewhere):
    /// the row with its anchor and the session tables go, and every
    /// Keychain item stays. No server claim alone deletes an item (I6), and
    /// a stray `K` opens nothing once the account's data is erased (§8.6).
    Elsewhere,
}

/// Everything the engine talks to.
pub struct SyncDeps {
    /// The authenticated client: `AuthManager` in the service, so every
    /// request carries this device's token and passes the egress ledger.
    pub http: Arc<dyn HttpClient>,
    pub keys: Arc<dyn VaultKeyStore>,
    pub repo: Arc<dyn VaultStateRepo>,
    pub host: Arc<dyn HostIdentity>,
    pub clock: Arc<dyn SyncClock>,
    pub blocks: Arc<dyn BlockActivity>,
    pub routing: Arc<SyncRouting>,
    /// The longest wait between compare-and-swap attempts; each waits a
    /// random part of it.
    pub conflict_jitter: Duration,
}

/// What the device list shows (§9.2 `sync_state`): every line Rust-authored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub state: SyncState,
    pub status_line: String,
    /// The sync key's 12 digits, the same on every Mac holding it (J4).
    pub fingerprint: Option<String>,
    pub devices: Vec<DeviceRow>,
    pub held: HeldSummary,
    pub pending_reset_effective_at_ms: Option<i64>,
    pub notices: Vec<SyncNotice>,
    pub last_synced_at_ms: Option<i64>,
    pub error_code: Option<String>,
    /// Whether "Enter a Code" may be shown. Never in a state where the server
    /// could be steering the person to type an attacker's code (§6.1).
    pub offers_code_entry: bool,
    /// The copy is at 2,000 live rules and this Mac's last write left some of
    /// its own out (§4.1). The status line says so; no rule is named.
    pub rules_over_cap: bool,
}

pub struct SyncEngine {
    api: VaultApi,
    deps: SyncDeps,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    session: Option<Session>,
    notices: NoticeQueue,
}

/// The signed-in account, and what this run knows of its vault.
struct Session {
    user: UserId,
    server_device_id: Option<ServerDeviceId>,
    /// `None` until sync has been used for this account on this Mac.
    record: Option<VaultStateRecord>,
    key: Option<VaultKey>,
    /// The server's copy as this Mac last opened it.
    base: Option<PrefsDocument>,
    /// `base` with this Mac's edits.
    working: Option<PrefsDocument>,
    newer: Option<NewerSchemaView>,
    clock: Option<HlcClock>,
    devices: Vec<ServerDevice>,
    default_name: Option<DeviceName>,
    joined_via: JoinedVia,
    pending_reset: Option<ServerReset>,
    /// When the vault was made, for J3's dated line.
    vault_created_at_ms: Option<i64>,
    /// A vault exists and its key is in this Mac's Keychain, while sync is
    /// off here: J2's card.
    available_elsewhere: bool,
    /// The key of a create whose answer never arrived. The vault may exist
    /// under it, so its item is kept, and a 404 lets this Mac try the create
    /// again with the same key rather than leave it waiting on itself.
    pending_create: Option<VaultKey>,
    /// Holds the vault's requests after a 412 storm or a 429, never the
    /// device list.
    backoff_until_ms: Option<i64>,
    last_synced_ms: Option<i64>,
    /// This Mac's last write had rules the copy had no room for (§4.1).
    rules_over_cap: bool,
    /// A line that outlives the row it describes ("Sync was turned off from
    /// another of your Macs.").
    line: Option<&'static str>,
}

impl Session {
    fn new(user: UserId, server_device_id: Option<ServerDeviceId>) -> Self {
        Self {
            user,
            server_device_id,
            record: None,
            key: None,
            base: None,
            working: None,
            newer: None,
            clock: None,
            devices: Vec::new(),
            default_name: None,
            joined_via: JoinedVia::IcloudKeychain,
            pending_reset: None,
            vault_created_at_ms: None,
            available_elsewhere: false,
            pending_create: None,
            backoff_until_ms: None,
            last_synced_ms: None,
            rules_over_cap: false,
            line: None,
        }
    }

    fn state(&self) -> SyncState {
        let state = self
            .record
            .as_ref()
            .and_then(|record| SyncState::parse(&record.state))
            .unwrap_or(SyncState::Off);
        if state == SyncState::Off && self.available_elsewhere {
            SyncState::AvailableElsewhere
        } else {
            state
        }
    }

    fn set_state(&mut self, state: SyncState, error: Option<&str>) {
        if let Some(record) = self.record.as_mut() {
            record.state = state.as_str().to_owned();
            record.last_error = error.map(str::to_owned);
        }
    }

    fn this_mac(&self) -> Option<SyncDeviceId> {
        self.record
            .as_ref()
            .and_then(|record| SyncDeviceId::parse(&record.sync_device_id))
    }

    fn anchor(&self) -> Anchor {
        self.record.as_ref().map(anchor_of).unwrap_or_default()
    }

    /// Forgets the copy and the key, keeping the row's anchor.
    fn drop_copy(&mut self) {
        self.key = None;
        self.forget_document();
        self.pending_reset = None;
    }

    /// Forgets the document this Mac opened, before trust moves to another
    /// vault: only this Mac's own registers enter the new one, never another
    /// Mac's entries or rules from the old.
    fn forget_document(&mut self) {
        self.base = None;
        self.working = None;
        self.newer = None;
        self.clock = None;
        self.rules_over_cap = false;
    }

    /// A Mac's name in the open copy, for a notice that names it.
    fn device_name(&self, sync_device_id: &str) -> Option<String> {
        let id = SyncDeviceId::parse(sync_device_id)?;
        self.working
            .as_ref()?
            .devices
            .get(&id)
            .map(|entry| entry.name.value.as_str().to_owned())
    }

    fn hlc_clock(&mut self) -> Option<&mut HlcClock> {
        if self.clock.is_none() {
            let mut clock = HlcClock::new(self.this_mac()?);
            if let Some(last) = self
                .record
                .as_ref()
                .and_then(|record| record.last_hlc.as_deref())
                .and_then(Hlc::parse)
            {
                clock.observe(&last);
            }
            self.clock = Some(clock);
        }
        let clock = self.clock.as_mut()?;
        for document in [&self.base, &self.working].into_iter().flatten() {
            clock.observe_document(document);
        }
        Some(clock)
    }
}

fn anchor_of(record: &VaultStateRecord) -> Anchor {
    let trusted = record
        .write_pk
        .as_deref()
        .and_then(|bytes| WritePublicKey::from_slice(bytes).ok())
        .map(Trusted::of)
        .filter(|trusted| record.key_id.as_deref() == Some(trusted.key_id.to_hex().as_str()));
    Anchor {
        trusted,
        highest_seen_version: record.highest_seen_version,
        highest_seen_sha256: record
            .highest_seen_sha256
            .as_deref()
            .and_then(|bytes| bytes.try_into().ok()),
        fetched_at_ms: record.fetched_at_ms,
        seen_reset_request_id: record.seen_reset_request_id.clone(),
    }
}

fn set_trusted(record: &mut VaultStateRecord, trusted: Trusted) {
    record.key_id = Some(trusted.key_id.to_hex());
    record.write_pk = Some(trusted.write_pk.as_bytes().to_vec());
}

/// A new vault: its versions start again, so the rollback guard does too.
fn replace_trust(record: &mut VaultStateRecord, trusted: Trusted) {
    set_trusted(record, trusted);
    record.highest_seen_version = 0;
    record.highest_seen_sha256 = None;
    record.seen_reset_request_id = None;
    record.server_version = None;
    record.ciphertext = None;
}

fn max_hlc(document: &PrefsDocument) -> Option<Hlc> {
    let devices = document.devices.values().flat_map(|entry| {
        [
            &entry.server_device_id.hlc,
            &entry.name.hlc,
            &entry.notify_daily_insight.hlc,
            &entry.removed.hlc,
        ]
    });
    let rules = document.rules.values().map(|rule| &rule.hlc);
    let settings = [
        document
            .settings
            .invitations_enabled
            .as_ref()
            .map(|register| &register.hlc),
        document
            .settings
            .quiet_hours
            .as_ref()
            .map(|register| &register.hlc),
    ];
    devices
        .chain(rules)
        .chain(settings.into_iter().flatten())
        .max()
        .cloned()
}

/// Why a pass stopped early. The state is already set.
struct Stop;

type Step = Result<(), Stop>;

impl SyncEngine {
    pub fn new(deps: SyncDeps) -> Self {
        Self {
            api: VaultApi::new(Arc::clone(&deps.http)),
            deps,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn routing(&self) -> Arc<SyncRouting> {
        Arc::clone(&self.deps.routing)
    }

    fn now(&self) -> i64 {
        self.deps.clock.now_ms()
    }

    fn time(&self) -> LocalTime {
        LocalTime {
            utc_offset_seconds: self.deps.clock.utc_offset_seconds(),
        }
    }

    /// Whether the row is on disk as the session has it (or there is none).
    fn save(&self, session: &Session) -> bool {
        let Some(record) = &session.record else {
            return true;
        };
        match self.deps.repo.save_vault_state(record) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(
                    error_code = "vault_state_save_failed",
                    error = %error,
                    "the sync state could not be saved"
                );
                false
            }
        }
    }

    /// Queues a notice and keeps it in `sync_notice`. `name` is the joined
    /// Mac's, read from the copy that showed it.
    fn queue(
        &self,
        session: &Session,
        notices: &mut NoticeQueue,
        event: NoticeEvent,
        name: Option<&str>,
    ) {
        let notice = event.render(name, self.time());
        if let Some(record) = notices.push(session.user.as_str(), event, notice, self.now()) {
            self.keep_notice(&record);
        }
    }

    fn keep_notice(&self, record: &crate::persistence::SyncNoticeRecord) {
        if let Err(error) = self.deps.repo.save_sync_notice(record) {
            tracing::error!(error_code = "sync_notice_save_failed", error = %error, "a security notice was not kept on disk");
        }
    }

    /// The notices an earlier run of the service queued for this account.
    fn restore_notices(&self, session: &Session, notices: &mut NoticeQueue) {
        let cutoff = self.now().saturating_sub(ACKNOWLEDGED_NOTICE_KEPT_MS);
        if let Err(error) = self.deps.repo.prune_sync_notices(cutoff) {
            tracing::warn!(error_code = "sync_notice_prune_failed", error = %error, "old acknowledged notices were not deleted");
        }
        let records = match self.deps.repo.sync_notices(session.user.as_str()) {
            Ok(records) => records,
            Err(error) => {
                tracing::error!(error_code = "sync_notice_load_failed", error = %error, "kept security notices could not be read");
                return;
            }
        };
        for record in records {
            let Some(event) = NoticeEvent::from_record(&record) else {
                continue;
            };
            let name = match &event {
                NoticeEvent::MacJoinedSync { sync_device_id, .. } => {
                    session.device_name(sync_device_id)
                }
                _ => None,
            };
            let notice = event.render(name.as_deref(), self.time());
            notices.restore(&record, event, notice);
        }
    }

    // --- Account events --------------------------------------------------------

    /// The service is signed in to `user`: at launch, after a sign-in, and
    /// after an account switch. Every other account's session ends; this
    /// account's row is checked against this Mac's host binding, and a
    /// session left on is resumed from the cached copy.
    pub async fn session_started(
        &self,
        user: UserId,
        server_device_id: Option<ServerDeviceId>,
    ) -> SyncSnapshot {
        let mut inner = self.inner.lock().await;
        let switched = inner
            .session
            .as_ref()
            .is_some_and(|session| session.user != user);
        if switched {
            inner.notices = NoticeQueue::default();
        }
        if let Err(error) = self.deps.repo.clear_sessions(Some(user.as_str())) {
            tracing::error!(error_code = "vault_session_clear_failed", error = %error, "other accounts' sync sessions were not cleared");
        }
        // Another account's notices ended with its session.
        if let Err(error) = self.deps.repo.delete_sync_notices(Some(user.as_str())) {
            tracing::error!(error_code = "sync_notice_clear_failed", error = %error, "other accounts' security notices were not cleared");
        }
        let mut session = Session::new(user, server_device_id);
        session.record = self
            .deps
            .repo
            .vault_state(session.user.as_str())
            .ok()
            .flatten();
        self.check_host_binding(&mut session);
        self.resume(&mut session).await;
        self.restore_notices(&session, &mut inner.notices);
        self.deps
            .routing
            .set_daily_insight(self.daily_insight(&session));
        let snapshot = self.snapshot_of(&session, &inner.notices);
        inner.session = Some(session);
        snapshot
    }

    /// Log Out (J7): the session ends and `K` leaves memory. The anchor stays,
    /// and no Keychain item is touched.
    pub async fn log_out(&self) {
        let mut inner = self.inner.lock().await;
        if let Some(mut session) = inner.session.take() {
            if let Some(record) = session.record.as_mut() {
                record.clear_session();
            }
            self.save(&session);
        }
        if let Err(error) = self.deps.repo.clear_sessions(None) {
            tracing::error!(error_code = "vault_session_clear_failed", error = %error, "the sync session was not cleared at log out");
        }
        if let Err(error) = self.deps.repo.delete_sync_notices(None) {
            tracing::error!(error_code = "sync_notice_clear_failed", error = %error, "security notices were not cleared at log out");
        }
        inner.notices = NoticeQueue::default();
        self.deps.routing.reset();
    }

    /// `user`'s account was deleted (J10, §8.6), on this Mac or, as the auth
    /// layer learned from velvt-core, elsewhere. The row goes with its
    /// anchor, and with it the session tables and the account's notices.
    /// Keychain items go only on this Mac's own Delete Account
    /// ([`AccountDeletion`]).
    pub async fn account_deleted(&self, user: &UserId, deletion: AccountDeletion) {
        let mut inner = self.inner.lock().await;
        if inner
            .session
            .as_ref()
            .is_some_and(|session| &session.user == user)
        {
            inner.session = None;
            inner.notices = NoticeQueue::default();
            self.deps.routing.reset();
        }
        if deletion == AccountDeletion::ThisMac {
            match self.deps.keys.inventory(user).await {
                Ok(inventory) => {
                    for held in &inventory.keys {
                        let _ = self.deps.keys.delete(user, held.key_id).await;
                    }
                }
                Err(error) => tracing::warn!(
                    error_code = error.code(),
                    "the sync key items were not deleted with the account"
                ),
            }
        }
        if let Err(error) = self.deps.repo.delete_vault_state(user.as_str()) {
            tracing::error!(error_code = "vault_state_delete_failed", error = %error, "the sync state was not deleted with the account");
        }
    }

    /// `403 device_revoked`: another Mac removed this one (J5, §5.7). The
    /// session ends and the anchor stays, so a sign-in back is checked
    /// against what this Mac trusted.
    async fn removed_from_account(&self, session: &mut Session, notices: &mut NoticeQueue) {
        if let Some(record) = session.record.as_mut() {
            record.clear_session();
        }
        session.drop_copy();
        self.save(session);
        let _ = self.deps.repo.clear_sessions(None);
        let at_ms = self.now();
        self.queue(
            session,
            notices,
            NoticeEvent::ThisMacRemoved { at_ms },
            None,
        );
        self.deps.routing.reset();
    }

    /// `401 account_deleted`. `AuthManager` answers every request so once it
    /// has been told, whichever account's session the request was for, so
    /// the answer alone does not say this account is gone. The session ends
    /// as at Log Out, keeping the anchor and every Keychain item; the auth
    /// layer, which knows whose account it was, calls [`Self::account_deleted`]
    /// (PR A4).
    fn session_refused(&self, session: &mut Session) {
        if let Some(record) = session.record.as_mut() {
            record.clear_session();
        }
        session.drop_copy();
        self.save(session);
        let _ = self.deps.repo.clear_sessions(None);
        self.deps.routing.reset();
    }

    // --- The person's commands (protocol 38, A4) ------------------------------

    /// Settings › Your Macs › Keep my choices in sync (J1), J2's [Turn On],
    /// and [Set Up Sync] after a reset. `default_name` is the computer name
    /// Swift reports.
    pub async fn turn_on(&self, default_name: Option<DeviceName>) -> SyncSnapshot {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_mut() else {
            return not_signed_in();
        };
        if session.record.is_none() {
            match self.new_record(session) {
                Some(record) => session.record = Some(record),
                None => return self.snapshot_of(session, &inner.notices),
            }
        }
        session.default_name = default_name;
        session.line = None;
        session.available_elsewhere = false;
        if let Some(record) = session.record.as_mut() {
            record.enabled = true;
            if record.state == SyncState::CloneDetected.as_str() {
                record.state = SyncState::Off.as_str().to_owned();
            }
        }
        session.backoff_until_ms = None;
        self.pass(session, &mut inner.notices, true).await;
        self.snapshot_of(session, &inner.notices)
    }

    /// Turn Off Sync on This Mac (J4): this Mac's entry is marked removed, the
    /// session ends, and the anchor and the Keychain item stay (deleting a
    /// synchronizable item deletes it from every Mac).
    pub async fn turn_off(&self) -> SyncSnapshot {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_mut() else {
            return not_signed_in();
        };
        let now = self.now();
        let syncing = matches!(
            session.state(),
            SyncState::Syncing | SyncState::ResetPending
        );
        if let (true, Some(this_mac), Some(mut working)) =
            (syncing, session.this_mac(), session.working.clone())
        {
            let marked = session.hlc_clock().is_some_and(|clock| {
                mirror::mark_removed(&mut working, this_mac, clock, now as u64).unwrap_or(false)
            });
            if marked {
                session.working = Some(working);
                let _ = self.write(session, &mut inner.notices, false).await;
            }
        }
        if let Some(record) = session.record.as_mut() {
            record.clear_session();
        }
        session.drop_copy();
        self.save(session);
        let _ = self.deps.repo.clear_sessions(None);
        self.deps.routing.reset();
        self.snapshot_of(session, &inner.notices)
    }

    /// One pass of fetch, verify, merge and write: the 10-minute poll, wake,
    /// a key arriving in the Keychain, and 5 s after a local change.
    pub async fn sync_now(&self) -> SyncSnapshot {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_mut() else {
            return not_signed_in();
        };
        self.pass(session, &mut inner.notices, false).await;
        self.snapshot_of(session, &inner.notices)
    }

    /// J2: with sync off here, whether the account has a vault whose key is
    /// already in this Mac's Keychain, for the one-time "Turn on here?" card.
    /// Writes no row. The card can show only if the Keychain holds a live
    /// key for the account, so without one nothing is asked of velvt-core
    /// (`egress::ENDPOINTS`).
    pub async fn probe_available(&self) -> SyncSnapshot {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_mut() else {
            return not_signed_in();
        };
        if session.record.as_ref().is_some_and(|record| record.enabled) {
            return self.snapshot_of(session, &inner.notices);
        }
        let Ok(inventory) = self.deps.keys.inventory(&session.user).await else {
            return self.snapshot_of(session, &inner.notices);
        };
        if inventory.live().next().is_none() {
            session.available_elsewhere = false;
            return self.snapshot_of(session, &inner.notices);
        }
        let Ok(view) = self.api.get_vault(&session.user).await else {
            return self.snapshot_of(session, &inner.notices);
        };
        session.available_elsewhere = matches!(
            view,
            VaultView::Present { key_id, .. }
                if inventory.holds_live(key_id) && !inventory.retires(key_id)
        );
        self.snapshot_of(session, &inner.notices)
    }

    /// Renames any Mac in the document (J4 [Rename]). Written on the next
    /// pass.
    pub async fn rename_device(&self, id: SyncDeviceId, name: DeviceName) -> SyncSnapshot {
        self.edit(|document, clock, now| {
            let Some(entry) = document.devices.get_mut(&id) else {
                return Ok(false);
            };
            if entry.name.value == name {
                return Ok(false);
            }
            entry.name = doc::Register {
                value: name.clone(),
                hlc: clock.tick(now)?,
            };
            Ok(true)
        })
        .await
    }

    /// A Mac's daily-insight notification switch (J4).
    pub async fn set_daily_insight(&self, id: SyncDeviceId, on: bool) -> SyncSnapshot {
        self.edit(|document, clock, now| {
            let Some(entry) = document.devices.get_mut(&id) else {
                return Ok(false);
            };
            if entry.notify_daily_insight.value == on {
                return Ok(false);
            }
            entry.notify_daily_insight = doc::Register {
                value: on,
                hlc: clock.tick(now)?,
            };
            Ok(true)
        })
        .await
    }

    async fn edit(
        &self,
        change: impl FnOnce(
            &mut PrefsDocument,
            &mut HlcClock,
            u64,
        ) -> Result<bool, crate::vault::merge::HlcError>,
    ) -> SyncSnapshot {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_mut() else {
            return not_signed_in();
        };
        let now = self.now() as u64;
        if let (None, Some(mut working)) = (&session.newer, session.working.clone()) {
            let changed = session
                .hlc_clock()
                .is_some_and(|clock| matches!(change(&mut working, clock, now), Ok(true)));
            if changed {
                session.working = Some(working);
                self.deps
                    .routing
                    .set_daily_insight(self.daily_insight(session));
            }
        }
        self.snapshot_of(session, &inner.notices)
    }

    /// A key delivered by a typed setup code (§7.1 step 7; PR A5 claims and
    /// opens the code).
    ///
    /// Whether a code may be entered is Rust's judgement (I3), not the
    /// window's: only in a state that offers one (waiting for the key or the
    /// rotated key, or after a reset §6.1 accepted), or before sync is on
    /// here. Never where the server could be steering the person to type an
    /// attacker's code (`server_inconsistent`, an unreadable copy, a vault
    /// that vanished): there the code changes nothing.
    ///
    /// If the server's vault has another key, the code wins: this Mac trusts
    /// the typed key (an anchor already on it keeps its rollback guard),
    /// pauses (`server_inconsistent`), and never adopts the served key.
    ///
    /// If the server's vault is the typed key's, it is adopted only as a pass
    /// would adopt it: the key this Mac already trusts, or nothing trusted
    /// yet, or a rotation the trusted key signed, or a new vault after a
    /// reset §6.1 accepts. A code does not turn a key change no Mac signed
    /// into one (T5, T13). Only then is the key stored, synchronizable, with
    /// `supersedes` when it is a rotation's and the old item marked
    /// `retired_by` (§5.5), so the rotation markers exist on this Apple ID
    /// too (T14).
    pub async fn adopt_delivered_key(&self, key: VaultKey) -> SyncSnapshot {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_mut() else {
            return not_signed_in();
        };
        let state = session.state();
        if !(state.offers_code_entry()
            || matches!(state, SyncState::Off | SyncState::AvailableElsewhere))
        {
            tracing::info!(
                error_code = "setup_code_not_offered",
                "a setup code was refused: this Mac's sync state offers no code entry"
            );
            return self.snapshot_of(session, &inner.notices);
        }
        if session.record.is_none() {
            match self.new_record(session) {
                Some(record) => session.record = Some(record),
                None => return self.snapshot_of(session, &inner.notices),
            }
        }
        let user = session.user.clone();
        let typed = Trusted::of(*key.write_public_key());
        let inventory = match self.deps.keys.inventory(&user).await {
            Ok(inventory) => inventory,
            Err(error) => {
                session.set_state(SyncState::Error, Some(error.code()));
                return self.snapshot_of(session, &inner.notices);
            }
        };
        let view = match self.api.get_vault(&user).await {
            Ok(view) => view,
            Err(error) => {
                self.api_failure(session, &mut inner.notices, error).await;
                return self.snapshot_of(session, &inner.notices);
            }
        };
        let anchor = session.anchor();
        if let Some(record) = session.record.as_mut() {
            record.enabled = true;
        }
        let served_is_typed = matches!(
            &view,
            VaultView::Present { key_id, write_pk, .. }
                if *key_id == typed.key_id && *write_pk == typed.write_pk
        );
        if !served_is_typed {
            if anchor.trusted != Some(typed) {
                session.forget_document();
                if let Some(record) = session.record.as_mut() {
                    replace_trust(record, typed);
                }
            }
            session.key = Some(key);
            self.inconsistent(
                session,
                &mut inner.notices,
                Inconsistency::DeliveredKeyMismatch,
            );
            self.save(session);
            return self.snapshot_of(session, &inner.notices);
        }

        let roots = match self.trust_roots(&user, &anchor, &inventory, &view).await {
            Ok(roots) => roots,
            Err(error) => {
                session.set_state(SyncState::Error, Some(error.code()));
                return self.snapshot_of(session, &inner.notices);
            }
        };
        let judged = judge_vault(&view, &anchor, &inventory, &roots, &user, self.now());
        let adoption = match judged.judgement {
            VaultJudgement::Current => Adoption::Keep,
            VaultJudgement::Untrusted { .. } => Adoption::Fresh,
            VaultJudgement::Rotated { path, to, .. } if to == typed => Adoption::Rotation(path),
            VaultJudgement::ResetCompleted {
                new_vault: Some(new),
                ..
            } if new == typed => Adoption::NewVault { deleted: None },
            VaultJudgement::Deleted { key_id } => Adoption::NewVault {
                deleted: Some(key_id),
            },
            VaultJudgement::Inconsistent(reason) => {
                self.inconsistent(session, &mut inner.notices, reason);
                self.save(session);
                return self.snapshot_of(session, &inner.notices);
            }
            _ => {
                self.inconsistent(
                    session,
                    &mut inner.notices,
                    Inconsistency::UnverifiedKeyChange,
                );
                self.save(session);
                return self.snapshot_of(session, &inner.notices);
            }
        };

        // The Keychain only now, after the served vault verified against the
        // typed key, and before any trust changes.
        let supersedes = match &adoption {
            Adoption::Rotation(path) => path.last().map(|step| step.statement.old_key_id),
            _ => None,
        };
        if inventory.get(typed.key_id).is_none() {
            if let Err(error) = self.deps.keys.store(&user, &key, supersedes).await {
                session.set_state(SyncState::Error, Some(error.code()));
                self.save(session);
                return self.snapshot_of(session, &inner.notices);
            }
        }
        let Some(record) = session.record.as_mut() else {
            return not_signed_in();
        };
        match adoption {
            Adoption::Keep => {}
            Adoption::Fresh => {
                replace_trust(record, typed);
                session.forget_document();
            }
            Adoption::Rotation(path) => {
                set_trusted(record, typed);
                // §5.6 step 6, as on a Mac that found K' in its Keychain.
                for step in &path {
                    let _ = self
                        .deps
                        .keys
                        .retire(&user, step.statement.old_key_id, step.statement.new_key_id)
                        .await;
                }
            }
            Adoption::NewVault { deleted } => {
                replace_trust(record, typed);
                session.drop_copy();
                // A deletion its own key signed: this Mac's item for it goes,
                // as the pass would delete it (J9).
                if let Some(deleted) = deleted.filter(|deleted| inventory.get(*deleted).is_some()) {
                    let _ = self.deps.keys.delete(&user, deleted).await;
                }
            }
        }
        session.key = Some(key);
        session.joined_via = JoinedVia::Code;
        session.line = None;
        self.pass(session, &mut inner.notices, false).await;
        self.snapshot_of(session, &inner.notices)
    }

    /// The notices due to be posted now, each once: none while a work block
    /// is active (J6, §11.3). Each is recorded as posted in `sync_notice`, so
    /// a restart neither loses one held nor posts one again.
    pub async fn take_due_notices(&self) -> Vec<SyncNotice> {
        let block_active = self.deps.blocks.block_active();
        let now = self.now();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_ref() else {
            return Vec::new();
        };
        let (due, records) = inner
            .notices
            .take_due(session.user.as_str(), block_active, now);
        for record in &records {
            self.keep_notice(record);
        }
        due
    }

    pub async fn acknowledge_notice(&self, notice_id: &str) {
        let now = self.now();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        let Some(session) = inner.session.as_ref() else {
            return;
        };
        if let Some(record) = inner
            .notices
            .acknowledge(session.user.as_str(), notice_id, now)
        {
            self.keep_notice(&record);
        }
    }

    pub async fn snapshot(&self) -> SyncSnapshot {
        let inner = self.inner.lock().await;
        match &inner.session {
            Some(session) => self.snapshot_of(session, &inner.notices),
            None => not_signed_in(),
        }
    }

    // --- Launch ----------------------------------------------------------------

    fn new_record(&self, session: &mut Session) -> Option<VaultStateRecord> {
        let host = self.deps.host.host_uuid().unwrap_or(UNKNOWN_HOST);
        let (salt, binding) = match host::new_binding(&host) {
            Ok(binding) => binding,
            Err(_) => {
                session.line = None;
                return None;
            }
        };
        Some(VaultStateRecord {
            user_id: session.user.as_str().to_owned(),
            sync_device_id: SyncDeviceId::generate().to_string(),
            host_binding_salt: salt.to_vec(),
            host_binding: binding.to_vec(),
            key_id: None,
            write_pk: None,
            highest_seen_version: 0,
            highest_seen_sha256: None,
            fetched_at_ms: None,
            seen_reset_request_id: None,
            enabled: false,
            server_version: None,
            ciphertext: None,
            last_hlc: None,
            state: SyncState::Off.as_str().to_owned(),
            last_error: None,
        })
    }

    /// J8: a row copied here from another Mac takes a new `sync_device_id`
    /// and turns sync off on this Mac. The source Mac's entry stays its own.
    fn check_host_binding(&self, session: &mut Session) {
        let Some(host) = self.deps.host.host_uuid() else {
            return;
        };
        let Some(record) = session.record.as_mut() else {
            return;
        };
        if host::matches(&record.host_binding_salt, &record.host_binding, &host) {
            return;
        }
        let Ok((salt, binding)) = host::new_binding(&host) else {
            return;
        };
        let unbound = host::matches(
            &record.host_binding_salt,
            &record.host_binding,
            &UNKNOWN_HOST,
        );
        record.host_binding_salt = salt.to_vec();
        record.host_binding = binding.to_vec();
        if !unbound {
            record.sync_device_id = SyncDeviceId::generate().to_string();
            record.clear_session();
            record.state = SyncState::CloneDetected.as_str().to_owned();
        }
        self.save(session);
    }

    /// Opens the cached copy of a session left on.
    async fn resume(&self, session: &mut Session) {
        let Some(record) = session.record.as_ref() else {
            return;
        };
        if !record.enabled {
            return;
        }
        let anchor = anchor_of(record);
        let Some(trusted) = anchor.trusted else {
            return;
        };
        if self.load_key(session, trusted.key_id).await.is_err() {
            self.save(session);
            return;
        }
        self.open_cached_copy(session);
    }

    /// The copy at `highest_seen_version`, kept sealed in the row, as the
    /// base when this run has none: at launch, and on the first pass that
    /// has the key when the Keychain could not give it at launch. Without
    /// it, an older copy served then would become the base, and its
    /// rolled-back registers would be written at highest + 1 (T4).
    fn open_cached_copy(&self, session: &mut Session) {
        if session.base.is_some() || session.newer.is_some() {
            return;
        }
        let Some(record) = session.record.as_ref() else {
            return;
        };
        let Some(trusted) = anchor_of(record).trusted else {
            return;
        };
        let (Some(ciphertext), Some(key)) = (record.ciphertext.as_ref(), session.key.as_ref())
        else {
            return;
        };
        let name = BlobName::prefs();
        let context = DocumentContext {
            user_id: &session.user,
            name: &name,
            key_id: trusted.key_id,
            schema: SCHEMA_V1,
            version: record.highest_seen_version,
        };
        if let Ok(DecodedDocument::Current(document)) = doc::open(key, &context, ciphertext) {
            session.working = Some(document.clone());
            session.base = Some(document);
        }
    }

    /// §6.1's trusted key for a Mac with no anchor: every key its Keychain
    /// holds live. Read only when it is needed, since an item carries no
    /// public key: with an anchor, or when the served key is one the
    /// Keychain holds live, the inventory alone answers.
    async fn trust_roots(
        &self,
        user: &UserId,
        anchor: &Anchor,
        inventory: &KeyInventory,
        view: &VaultView,
    ) -> Result<Vec<Trusted>, KeyStoreError> {
        if anchor.trusted.is_some() {
            return Ok(Vec::new());
        }
        if let VaultView::Present { key_id, .. } = view {
            if inventory.holds_live(*key_id) {
                return Ok(Vec::new());
            }
        }
        let mut roots = Vec::new();
        for held in inventory.live() {
            match self.deps.keys.load(user, held.key_id).await {
                Ok(key) if key.key_id() == held.key_id => {
                    roots.push(Trusted::of(*key.write_public_key()));
                }
                // Gone since the inventory, or not the key its item names:
                // it opens nothing, so it vouches for nothing.
                Ok(_) | Err(KeyStoreError::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(roots)
    }

    /// `K` for `key_id` into memory. A missing item leaves the Mac waiting
    /// for it (§14: "Keychain item deleted by the person").
    async fn load_key(&self, session: &mut Session, key_id: KeyId) -> Step {
        if session
            .key
            .as_ref()
            .is_some_and(|key| key.key_id() == key_id)
        {
            return Ok(());
        }
        match self.deps.keys.load(&session.user, key_id).await {
            Ok(key) if key.key_id() == key_id => {
                session.key = Some(key);
                Ok(())
            }
            Ok(_) | Err(KeyStoreError::NotFound) => {
                session.key = None;
                session.set_state(SyncState::WaitingForKey, None);
                Err(Stop)
            }
            Err(error) => {
                session.set_state(SyncState::Error, Some(error.code()));
                Err(Stop)
            }
        }
    }

    // --- The pass ---------------------------------------------------------------

    async fn pass(&self, session: &mut Session, notices: &mut NoticeQueue, turning_on: bool) {
        let now = self.now();
        if !session.record.as_ref().is_some_and(|record| record.enabled) {
            return;
        }
        let backing_off = !turning_on && session.backoff_until_ms.is_some_and(|until| now < until);
        if !backing_off {
            let _ = self.pass_steps(session, notices, now, turning_on).await;
        }
        // §10.4: with sync on, every pass lists the account's devices, in
        // whatever state the copy is (waiting for a key, paused, read-only,
        // missing, reset, backing off), so no state hides a new sign-in.
        if lists_devices(session) {
            self.refresh_devices(session, notices, now).await;
        }
        self.deps
            .routing
            .set_daily_insight(self.daily_insight(session));
        if !self.save(session) {
            session.set_state(SyncState::Error, Some(STATE_NOT_SAVED));
        }
    }

    async fn pass_steps(
        &self,
        session: &mut Session,
        notices: &mut NoticeQueue,
        now: i64,
        turning_on: bool,
    ) -> Step {
        let state_before = session.state();
        let user = session.user.clone();
        let mut inventory = match self.deps.keys.inventory(&user).await {
            Ok(inventory) => inventory,
            Err(error) => {
                session.set_state(SyncState::Error, Some(error.code()));
                return Err(Stop);
            }
        };
        let view = match self.api.get_vault(&user).await {
            Ok(view) => view,
            Err(error) => {
                self.api_failure(session, notices, error).await;
                return Err(Stop);
            }
        };
        // §14: a create this run sent and never heard back about lost the
        // race if the vault is under another key. Its item is an orphan, and
        // goes (its own key id only, §5.5); kept, it would stand as a key
        // this Mac trusts against the winner's vault.
        if let (Some(pending), VaultView::Present { key_id, .. }) = (&session.pending_create, &view)
        {
            let orphan = pending.key_id();
            if *key_id != orphan {
                let _ = self.deps.keys.delete(&user, orphan).await;
                inventory.keys.retain(|held| held.key_id != orphan);
                session.pending_create = None;
            }
        }
        let anchor = session.anchor();
        let roots = match self.trust_roots(&user, &anchor, &inventory, &view).await {
            Ok(roots) => roots,
            Err(error) => {
                session.set_state(SyncState::Error, Some(error.code()));
                return Err(Stop);
            }
        };
        let judged = judge_vault(&view, &anchor, &inventory, &roots, &user, now);
        {
            let record = session.record.as_mut().ok_or(Stop)?;
            record.fetched_at_ms = Some(now);
            match &judged.seen_reset {
                SeenReset::Set(id) => record.seen_reset_request_id = Some(id.clone()),
                SeenReset::Clear => record.seen_reset_request_id = None,
                SeenReset::Keep => {}
            }
        }
        // What this fetch showed is on disk before anything is written to
        // velvt-core: a Mac that cannot keep its anchor does not act on it.
        if !self.save(session) {
            session.set_state(SyncState::Error, Some(STATE_NOT_SAVED));
            return Err(Stop);
        }
        // A pending reset this Mac believes, on every pass that shows it:
        // queued once, and kept in `sync_notice` until posted.
        if let Some(pending) = &judged.pending_reset {
            self.queue(
                session,
                notices,
                NoticeEvent::ResetRequested {
                    reset_id: pending.id.clone(),
                    requested_at_ms: pending.requested_at_ms,
                    effective_at_ms: pending.effective_at_ms,
                },
                None,
            );
        }
        session.pending_reset = judged.pending_reset.clone();
        session.vault_created_at_ms = match &view {
            VaultView::Present { created_at_ms, .. } => Some(*created_at_ms),
            VaultView::Missing { .. } => None,
        };
        self.retire_expired_items(&user, &inventory, now).await;

        match judged.judgement {
            VaultJudgement::Current => self.sync_copy(session, notices, now).await,
            VaultJudgement::Untrusted { key_id, write_pk } => {
                if !inventory.holds_live(key_id) {
                    session.set_state(SyncState::WaitingForKey, None);
                    return Err(Stop);
                }
                self.load_key(session, key_id).await?;
                if session
                    .key
                    .as_ref()
                    .is_none_or(|key| *key.write_public_key() != write_pk)
                {
                    self.inconsistent(session, notices, Inconsistency::WriteKeyChanged);
                    return Err(Stop);
                }
                let own_create = session
                    .pending_create
                    .take_if(|pending| pending.key_id() == key_id)
                    .is_some();
                session.forget_document();
                let record = session.record.as_mut().ok_or(Stop)?;
                replace_trust(record, Trusted { key_id, write_pk });
                session.joined_via = if own_create {
                    JoinedVia::Created
                } else {
                    JoinedVia::IcloudKeychain
                };
                self.sync_copy(session, notices, now).await
            }
            VaultJudgement::Rotated { path, from, to } => {
                if !inventory.holds_live(to.key_id) {
                    // A Mac with no anchor checked the rotation from a key it
                    // holds: that key is what it trusts while it waits.
                    if anchor.trusted.is_none() {
                        let record = session.record.as_mut().ok_or(Stop)?;
                        set_trusted(record, from);
                    }
                    self.queue(
                        session,
                        notices,
                        NoticeEvent::SyncKeyChanged {
                            new_key_id: to.key_id.to_hex(),
                            adopted: false,
                            at_ms: now,
                        },
                        None,
                    );
                    session.set_state(SyncState::WaitingForRotatedKey, None);
                    return Err(Stop);
                }
                self.load_key(session, to.key_id).await?;
                if session
                    .key
                    .as_ref()
                    .is_none_or(|key| *key.write_public_key() != to.write_pk)
                {
                    self.inconsistent(session, notices, Inconsistency::WriteKeyChanged);
                    return Err(Stop);
                }
                // §5.6 step 6, on every Mac that verifies the rotation: the old
                // item is marked retired. Idempotent.
                for step in &path {
                    let _ = self
                        .deps
                        .keys
                        .retire(&user, step.statement.old_key_id, step.statement.new_key_id)
                        .await;
                }
                let record = session.record.as_mut().ok_or(Stop)?;
                set_trusted(record, to);
                self.queue(
                    session,
                    notices,
                    NoticeEvent::SyncKeyChanged {
                        new_key_id: to.key_id.to_hex(),
                        adopted: true,
                        at_ms: now,
                    },
                    None,
                );
                self.sync_copy(session, notices, now).await
            }
            VaultJudgement::Deleted { key_id } => {
                // J9: the deletion verified under the key this Mac trusts. Its
                // own item for that key goes (a no-op on the deleting Mac's
                // Apple ID), and so does the row.
                if inventory.get(key_id).is_some() {
                    let _ = self.deps.keys.delete(&user, key_id).await;
                }
                let _ = self.deps.repo.delete_vault_state(user.as_str());
                session.record = None;
                session.drop_copy();
                session.line = Some("Sync was turned off from another of your Macs.");
                Err(Stop)
            }
            VaultJudgement::ResetCompleted {
                reset_id,
                key_id: reset_key,
                while_offline,
                new_vault,
            } => {
                if let Some(completed) = view.facts().completed_reset.as_ref() {
                    self.queue(
                        session,
                        notices,
                        NoticeEvent::ResetCompleted {
                            reset_id,
                            effective_at_ms: completed.effective_at_ms,
                            while_offline,
                        },
                        None,
                    );
                }
                session.drop_copy();
                let pending_create = session.pending_create.as_ref().map(VaultKey::key_id);
                match new_vault {
                    Some(new) if inventory.holds_live(new.key_id) => {
                        self.load_key(session, new.key_id).await?;
                        if session
                            .key
                            .as_ref()
                            .is_none_or(|key| *key.write_public_key() != new.write_pk)
                        {
                            self.inconsistent(session, notices, Inconsistency::WriteKeyChanged);
                            return Err(Stop);
                        }
                        let record = session.record.as_mut().ok_or(Stop)?;
                        replace_trust(record, new);
                        self.sync_copy(session, notices, now).await
                    }
                    // [Set Up Sync] on a Mac already showing the reset: §5.5's
                    // one exception, an item the verified reset names, does
                    // not block creating the new vault. Nor does the item of
                    // this Mac's own create that got no answer or was
                    // refused: that create is sent again under its key.
                    None if (pending_create.is_some()
                        || (turning_on && state_before == SyncState::ResetCompleted))
                        && inventory.live().all(|held| {
                            held.key_id == reset_key || Some(held.key_id) == pending_create
                        }) =>
                    {
                        self.create(session, notices, now).await
                    }
                    _ => {
                        session.set_state(SyncState::ResetCompleted, None);
                        Err(Stop)
                    }
                }
            }
            VaultJudgement::NoVault if turning_on => self.create(session, notices, now).await,
            VaultJudgement::NoVault => {
                session.set_state(SyncState::Off, None);
                Err(Stop)
            }
            // A create this run sent and never heard back about: its item is
            // the only one, and no vault was ever seen. Sent again under the
            // same key.
            VaultJudgement::MissingOnServer
                if anchor.highest_seen_version == 0
                    && session.pending_create.as_ref().is_some_and(|pending| {
                        inventory.live().all(|held| held.key_id == pending.key_id())
                    }) =>
            {
                self.create(session, notices, now).await
            }
            VaultJudgement::MissingOnServer => {
                session.set_state(SyncState::MissingOnServer, None);
                Err(Stop)
            }
            VaultJudgement::Inconsistent(reason) => {
                self.inconsistent(session, notices, reason);
                Err(Stop)
            }
        }
    }

    /// §5.6 step 7: a retired item goes 30 days after the item that replaced
    /// it was made.
    async fn retire_expired_items(&self, user: &UserId, inventory: &KeyInventory, now: i64) {
        for held in &inventory.keys {
            let Some(RetiredBy::Key(successor)) = held.retired_by else {
                continue;
            };
            let expired = inventory
                .get(successor)
                .and_then(|next| next.created_at_ms)
                .is_some_and(|made| now.saturating_sub(made) > RETIRED_KEY_GRACE_MS);
            if expired {
                let _ = self.deps.keys.delete(user, held.key_id).await;
            }
        }
    }

    fn inconsistent(
        &self,
        session: &mut Session,
        notices: &mut NoticeQueue,
        reason: Inconsistency,
    ) {
        session.set_state(SyncState::ServerInconsistent, Some(reason.code()));
        let discriminator = format!(
            "{}:{}",
            reason.code(),
            session
                .anchor()
                .trusted
                .map(|trusted| trusted.key_id.to_hex())
                .unwrap_or_default()
        );
        self.queue(
            session,
            notices,
            NoticeEvent::ServerInconsistent { discriminator },
            None,
        );
    }

    async fn api_failure(&self, session: &mut Session, notices: &mut NoticeQueue, error: ApiError) {
        match error {
            ApiError::DeviceRevoked => self.removed_from_account(session, notices).await,
            ApiError::AccountDeleted => self.session_refused(session),
            ApiError::RateLimited { ref retry_after } => {
                let wait = retry_after
                    .as_deref()
                    .and_then(retry_after_delay)
                    .map_or(CONFLICT_BACKOFF_MS, |wait| wait.as_millis() as i64);
                session.backoff_until_ms = Some(self.now().saturating_add(wait));
                if let Some(record) = session.record.as_mut() {
                    record.last_error = Some(error.code().to_owned());
                }
            }
            // §14: velvt-core down or the vault switched off: sync pauses
            // silently, and the state stays what it was.
            other => {
                if let Some(record) = session.record.as_mut() {
                    record.last_error = Some(other.code().to_owned());
                }
            }
        }
    }

    // --- The copy ----------------------------------------------------------------

    /// Fetches, verifies and opens the copy, and writes this Mac's registers.
    async fn sync_copy(&self, session: &mut Session, notices: &mut NoticeQueue, now: i64) -> Step {
        let trusted = session.anchor().trusted.ok_or(Stop)?;
        self.load_key(session, trusted.key_id).await?;
        self.open_cached_copy(session);
        self.fetch_copy(session, notices).await?;
        if session.newer.is_some() {
            session.set_state(SyncState::ReadOnlyNewerSchema, None);
            return Ok(());
        }
        self.write(session, notices, true).await?;
        let state = if session.pending_reset.is_some() {
            SyncState::ResetPending
        } else {
            SyncState::Syncing
        };
        session.set_state(state, None);
        session.last_synced_ms = Some(now);
        Ok(())
    }

    async fn fetch_copy(&self, session: &mut Session, notices: &mut NoticeQueue) -> Step {
        let anchor = session.anchor();
        let trusted = anchor.trusted.ok_or(Stop)?;
        let cached = session.base.is_some() || session.newer.is_some();
        let if_none_match = cached
            .then(|| {
                session
                    .record
                    .as_ref()
                    .and_then(|record| record.server_version)
            })
            .flatten();
        let blob = match self.api.get_blob(&session.user, if_none_match).await {
            Ok(BlobFetch::NotModified) => return Ok(()),
            Ok(BlobFetch::Blob(blob)) => blob,
            Ok(BlobFetch::VaultMissing(_) | BlobFetch::BlobMissing) => {
                // Gone since GET /v1/vault: the next pass judges the 404.
                if let Some(record) = session.record.as_mut() {
                    record.last_error = Some("vault_missing".into());
                }
                return Err(Stop);
            }
            Err(error) => {
                self.api_failure(session, notices, error).await;
                return Err(Stop);
            }
        };
        let judgement = judge_blob(&blob, &session.user, &trusted, &anchor);
        let older = match judgement {
            BlobJudgement::Inconsistent(reason) => {
                self.inconsistent(session, notices, reason);
                return Err(Stop);
            }
            BlobJudgement::Older => true,
            BlobJudgement::Current { .. } => false,
        };
        let key = session.key.as_ref().ok_or(Stop)?;
        let name = BlobName::prefs();
        let context = DocumentContext {
            user_id: &session.user,
            name: &name,
            key_id: trusted.key_id,
            schema: blob.schema,
            version: blob.version,
        };
        let opened = match doc::open(key, &context, &blob.ciphertext) {
            Ok(opened) => opened,
            Err(_) => {
                // Never overwritten, and no code or delete prompt (§6.1, T6).
                session.set_state(SyncState::ServerCopyUnreadable, Some("copy_unreadable"));
                return Err(Stop);
            }
        };
        let digest = sha256(&blob.ciphertext);
        match opened {
            DecodedDocument::NewerSchema(view) => {
                // Read-only (§4.1): names and routing are read, nothing is
                // written, and the cache keeps no copy it could not write back.
                let record = session.record.as_mut().ok_or(Stop)?;
                if !older {
                    record.highest_seen_version = blob.version;
                    record.highest_seen_sha256 = Some(digest.to_vec());
                    record.ciphertext = None;
                }
                record.server_version = Some(blob.version);
                session.newer = Some(view);
                Ok(())
            }
            DecodedDocument::Current(document) if older => {
                // §6.1, F4: merged, never taken as current; the next write goes
                // at highest_seen_version + 1. A Mac whose session no longer has
                // the newer copy takes this one as its base (T4's residual).
                let record = session.record.as_mut().ok_or(Stop)?;
                record.server_version = Some(blob.version);
                session.base = Some(match &session.base {
                    Some(base) => merge_older_copy(base, &document),
                    None => document.clone(),
                });
                session.working = Some(match &session.working {
                    Some(working) => merge_older_copy(working, &document),
                    None => document,
                });
                session.newer = None;
                Ok(())
            }
            DecodedDocument::Current(document) => {
                self.notice_joins(session, &document, notices);
                let record = session.record.as_mut().ok_or(Stop)?;
                record.highest_seen_version = blob.version;
                record.highest_seen_sha256 = Some(digest.to_vec());
                record.server_version = Some(blob.version);
                record.ciphertext = Some(blob.ciphertext);
                let merged = match &session.working {
                    Some(working) => merge(&document, working),
                    None => merge(&document, &document),
                };
                if merged.person_salt_replaced {
                    let _ = self.deps.repo.clear_rule_links();
                }
                session.working = Some(merged.document);
                session.base = Some(document);
                session.newer = None;
                Ok(())
            }
        }
    }

    /// `mac_joined_sync` for each Mac a newly opened copy has that the last
    /// one did not (§10.4).
    fn notice_joins(&self, session: &Session, document: &PrefsDocument, notices: &mut NoticeQueue) {
        let Some(previous) = session.base.as_ref() else {
            return;
        };
        let this_mac = session.this_mac();
        for (id, entry) in &document.devices {
            if Some(*id) == this_mac || entry.removed.value || previous.devices.contains_key(id) {
                continue;
            }
            let route = match entry.joined_via {
                JoinedVia::Code => JoinRoute::SetupCode,
                JoinedVia::IcloudKeychain => JoinRoute::ICloudKeychain,
                JoinedVia::Created => continue,
            };
            self.queue(
                session,
                notices,
                NoticeEvent::MacJoinedSync {
                    sync_device_id: id.to_string(),
                    route,
                    at_ms: entry.joined_at_ms as i64,
                },
                Some(entry.name.value.as_str()),
            );
        }
    }

    /// §10.4 Write: this Mac's registers into the last opened copy, then a
    /// compare-and-swap at `max(n, highest_seen_version) + 1`. On 412 it
    /// fetches, merges and tries again, three times, then backs off for 10
    /// minutes.
    async fn write(
        &self,
        session: &mut Session,
        notices: &mut NoticeQueue,
        own_registers: bool,
    ) -> Step {
        if session.newer.is_some() {
            return Ok(());
        }
        let now = self.now();
        if own_registers {
            self.write_own_registers(session, now as u64)?;
        }
        for attempt in 0..CONFLICT_ATTEMPTS {
            if attempt > 0 {
                self.fetch_copy(session, notices).await?;
                if own_registers {
                    self.write_own_registers(session, now as u64)?;
                }
            }
            match self.put_once(session, notices, now).await? {
                PutResult::Done => return Ok(()),
                PutResult::Conflict => {
                    let jitter = self.deps.conflict_jitter;
                    if !jitter.is_zero() {
                        let wait = jitter.mul_f64(f64::from(random_fraction()));
                        tokio::time::sleep(wait).await;
                    }
                }
            }
        }
        session.backoff_until_ms = Some(now.saturating_add(CONFLICT_BACKOFF_MS));
        if let Some(record) = session.record.as_mut() {
            record.last_error = Some("write_conflicts".into());
        }
        Err(Stop)
    }

    fn write_own_registers(&self, session: &mut Session, now: u64) -> Step {
        let own = match (session.this_mac(), session.server_device_id) {
            (Some(sync_device_id), Some(server_device_id)) => Some(OwnEntry {
                sync_device_id,
                server_device_id,
                joined_via: session.joined_via,
                default_name: session.default_name.clone(),
            }),
            _ => None,
        };
        let settings = self.deps.repo.local_sync_settings().unwrap_or_default();
        let links = self.deps.repo.rule_links().unwrap_or_default();
        let mut working = session.working.clone().ok_or(Stop)?;
        let clock = session.hlc_clock().ok_or(Stop)?;
        let result = (|| {
            if let Some(own) = &own {
                mirror::write_own_entry(&mut working, own, clock, now)?;
            }
            mirror::mirror_settings(&mut working, &settings, clock, now)?;
            Ok::<_, mirror::MirrorError>(mirror::mirror_rule_links(&mut working, &links))
        })();
        match result {
            Ok(over_cap) => {
                session.working = Some(working);
                session.rules_over_cap = !over_cap.is_empty();
                Ok(())
            }
            Err(mirror::MirrorError::Document(doc::DocumentError::TooManyLiveDevices)) => {
                session.set_state(SyncState::Error, Some("too_many_macs"));
                Err(Stop)
            }
            Err(_) => {
                session.set_state(SyncState::Error, Some("document_write_refused"));
                Err(Stop)
            }
        }
    }

    async fn put_once(
        &self,
        session: &mut Session,
        notices: &mut NoticeQueue,
        now: i64,
    ) -> Result<PutResult, Stop> {
        let (Some(base), Some(working)) = (session.base.clone(), session.working.clone()) else {
            return Err(Stop);
        };
        let record = session.record.as_ref().ok_or(Stop)?;
        let trusted = anchor_of(record).trusted.ok_or(Stop)?;
        let server_version = record.server_version.unwrap_or(0);
        let highest = record.highest_seen_version;
        let merged = merge(&base, &working);
        if session
            .this_mac()
            .is_some_and(|this_mac| merged.devices_over_cap.contains(&this_mac))
        {
            session.set_state(SyncState::Error, Some("too_many_macs"));
            return Err(Stop);
        }
        if !merged.rules_over_cap.is_empty() {
            session.rules_over_cap = true;
        }
        let mut document = merged.document;
        collect_garbage(&mut document, now as u64);
        if document == base && server_version >= highest {
            session.working = Some(document);
            return Ok(PutResult::Done);
        }
        let version = server_version.max(highest).saturating_add(1);
        let key = session.key.as_ref().ok_or(Stop)?;
        let name = BlobName::prefs();
        let context = DocumentContext {
            user_id: &session.user,
            name: &name,
            key_id: trusted.key_id,
            schema: SCHEMA_V1,
            version,
        };
        let Ok(sealed) = doc::seal(key, &context, &document) else {
            session.set_state(SyncState::Error, Some("document_write_refused"));
            return Err(Stop);
        };
        let digest = sha256(&sealed);
        let put = PutStatement {
            user_id: session.user.clone(),
            name,
            key_id: trusted.key_id,
            schema: SCHEMA_V1,
            base_version: server_version,
            new_version: version,
            ciphertext_sha256: digest,
        };
        let Ok(put_signature) = key.sign_statement(&put) else {
            session.set_state(SyncState::Error, Some("signing_failed"));
            return Err(Stop);
        };
        let body = PutBody {
            key_id: trusted.key_id,
            schema: SCHEMA_V1,
            new_version: version,
            ciphertext: &sealed,
            put_signature,
        };
        let outcome = match self
            .api
            .put_blob(&session.user, server_version, &body)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                self.api_failure(session, notices, error).await;
                return Err(Stop);
            }
        };
        match outcome {
            WriteOutcome::Stored { version: stored } if stored == version => {
                // Only the tombstones this copy carries: a rule deleted while
                // the PUT was in flight has a newer clock value and waits for
                // the next write.
                let pushed: Vec<VaultRuleLinkRecord> = self
                    .deps
                    .repo
                    .rule_links()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|link| mirror::tombstone_settled(&document, link, now as u64))
                    .collect();
                let _ = self.deps.repo.delete_pushed_tombstones(&pushed);
                let last_hlc = max_hlc(&document).map(|hlc| hlc.to_string());
                let record = session.record.as_mut().ok_or(Stop)?;
                record.ciphertext = Some(sealed);
                record.server_version = Some(version);
                record.highest_seen_version = version;
                record.highest_seen_sha256 = Some(digest.to_vec());
                if last_hlc.is_some() {
                    record.last_hlc = last_hlc;
                }
                session.base = Some(document.clone());
                session.working = Some(document);
                Ok(PutResult::Done)
            }
            WriteOutcome::Conflict { .. } => Ok(PutResult::Conflict),
            WriteOutcome::KeyMismatch | WriteOutcome::VaultMissing(_) => {
                // The key changed or the vault went: the next pass's GET
                // /v1/vault judges it.
                if let Some(record) = session.record.as_mut() {
                    record.last_error = Some("vault_changed".into());
                }
                Err(Stop)
            }
            WriteOutcome::Unauthorized => {
                session.set_state(SyncState::Error, Some("vault_write_unauthorized"));
                Err(Stop)
            }
            WriteOutcome::Stored { .. }
            | WriteOutcome::Exists
            | WriteOutcome::ResetPending
            | WriteOutcome::Refused { .. } => {
                session.set_state(SyncState::Error, Some("vault_write_refused"));
                Err(Stop)
            }
        }
    }

    /// §10.4 create: `K`, `W` and `person_salt`, stored in the Keychain first,
    /// then `POST /v1/vault`. Never while a live item exists (§5.5), except
    /// the one a verified reset names.
    async fn create(&self, session: &mut Session, notices: &mut NoticeQueue, now: i64) -> Step {
        let (Some(this_mac), Some(server_device_id)) =
            (session.this_mac(), session.server_device_id)
        else {
            session.set_state(SyncState::Error, Some("no_device_id"));
            return Err(Stop);
        };
        let Ok(salt) = PersonSalt::generate() else {
            session.set_state(SyncState::Error, Some("random_unavailable"));
            return Err(Stop);
        };
        let retrying = session.pending_create.is_some();
        let key = match session.pending_create.take() {
            Some(key) => key,
            None => match VaultKey::generate() {
                Ok(key) => key,
                Err(_) => {
                    session.set_state(SyncState::Error, Some("random_unavailable"));
                    return Err(Stop);
                }
            },
        };
        // Keychain first, server second (§5.5): no vault ever exists whose key
        // was not saved.
        if let Err(error) = self.deps.keys.store(&session.user, &key, None).await {
            if retrying {
                session.pending_create = Some(key);
            }
            session.set_state(SyncState::Error, Some(error.code()));
            return Err(Stop);
        }
        // From here the item is kept whatever happens, as for a create whose
        // answer never arrives: §5.5 deletes an item only on its closed list,
        // and of a create's own item only once it lost the race (409
        // `vault_exists`, the vault under another key). The next pass sends
        // the create again under this key, or finds the vault made under it.

        // Rules mirrored under another vault's salt name nothing in this one.
        let _ = self.deps.repo.clear_rule_links();
        let mut document = PrefsDocument::new(salt);
        let mut clock = HlcClock::new(this_mac);
        let settings = self.deps.repo.local_sync_settings().unwrap_or_default();
        let own = OwnEntry {
            sync_device_id: this_mac,
            server_device_id,
            joined_via: JoinedVia::Created,
            default_name: session.default_name.clone(),
        };
        let built =
            mirror::write_own_entry(&mut document, &own, &mut clock, now as u64).and_then(|_| {
                mirror::mirror_settings(&mut document, &settings, &mut clock, now as u64)
            });
        let trusted = Trusted::of(*key.write_public_key());
        let name = BlobName::prefs();
        let context = DocumentContext {
            user_id: &session.user,
            name: &name,
            key_id: trusted.key_id,
            schema: SCHEMA_V1,
            version: 1,
        };
        let sealed = match built
            .ok()
            .and_then(|_| doc::seal(&key, &context, &document).ok())
        {
            Some(sealed) => sealed,
            None => {
                session.pending_create = Some(key);
                session.set_state(SyncState::Error, Some("document_write_refused"));
                return Err(Stop);
            }
        };
        let digest = sha256(&sealed);
        let create = CreateStatement {
            user_id: session.user.clone(),
            key_id: trusted.key_id,
            write_pk: trusted.write_pk,
            ciphertext_sha256: digest,
        };
        let put = PutStatement {
            user_id: session.user.clone(),
            name,
            key_id: trusted.key_id,
            schema: SCHEMA_V1,
            base_version: 0,
            new_version: 1,
            ciphertext_sha256: digest,
        };
        let (Ok(create_signature), Ok(put_signature)) =
            (key.sign_statement(&create), key.sign_statement(&put))
        else {
            session.pending_create = Some(key);
            session.set_state(SyncState::Error, Some("signing_failed"));
            return Err(Stop);
        };
        let body = CreateBody {
            key_id: trusted.key_id,
            write_pk: trusted.write_pk,
            schema: SCHEMA_V1,
            ciphertext: &sealed,
            create_signature,
            put_signature,
        };
        match self.api.create_vault(&session.user, &body).await {
            Ok(WriteOutcome::Stored { .. }) => {
                let last_hlc = max_hlc(&document).map(|hlc| hlc.to_string());
                let record = session.record.as_mut().ok_or(Stop)?;
                replace_trust(record, trusted);
                record.highest_seen_version = 1;
                record.highest_seen_sha256 = Some(digest.to_vec());
                record.server_version = Some(1);
                record.ciphertext = Some(sealed);
                record.last_hlc = last_hlc;
                session.key = Some(key);
                session.clock = Some(clock);
                session.base = Some(document.clone());
                session.working = Some(document);
                session.joined_via = JoinedVia::Created;
                session.set_state(SyncState::Syncing, None);
                session.last_synced_ms = Some(now);
                Ok(())
            }
            Ok(WriteOutcome::Exists) => {
                // Two Macs turned sync on at once (§14), or this Mac's own
                // earlier create landed after its answer was lost. Which one
                // the vault is decides whether this item is an orphan: it is
                // deleted (its own key id only) only once velvt-core shows
                // the vault under another key. If it shows this key, the
                // vault is this Mac's own, and is opened as any copy is.
                match self.api.get_vault(&session.user).await {
                    Ok(VaultView::Present {
                        key_id, write_pk, ..
                    }) if key_id == trusted.key_id => {
                        if write_pk != trusted.write_pk {
                            self.inconsistent(session, notices, Inconsistency::WriteKeyChanged);
                            return Err(Stop);
                        }
                        let record = session.record.as_mut().ok_or(Stop)?;
                        replace_trust(record, trusted);
                        session.forget_document();
                        session.key = Some(key);
                        session.joined_via = JoinedVia::Created;
                        self.sync_copy(session, notices, now).await
                    }
                    Ok(VaultView::Present { .. }) => {
                        let _ = self.deps.keys.delete(&session.user, trusted.key_id).await;
                        session.set_state(SyncState::WaitingForKey, None);
                        Err(Stop)
                    }
                    Ok(VaultView::Missing { .. }) | Err(_) => {
                        // Not known: kept, and resolved like a create that
                        // got no answer.
                        session.pending_create = Some(key);
                        session.set_state(SyncState::WaitingForKey, None);
                        Err(Stop)
                    }
                }
            }
            Ok(_) => {
                // Refused: no vault was made under this key, and velvt-core
                // did not say one exists under another (409 `vault_exists`).
                // No race was lost, so the item is no orphan (§5.5).
                session.pending_create = Some(key);
                session.set_state(SyncState::Error, Some("vault_create_refused"));
                Err(Stop)
            }
            Err(error) => {
                // The create may have landed: keep the item, and let the next
                // pass find the vault under it, or send it again.
                session.pending_create = Some(key);
                self.api_failure(session, notices, error).await;
                Err(Stop)
            }
        }
    }

    /// `GET /v1/devices`: the new-sign-in diff (§10.4) and the device list.
    async fn refresh_devices(&self, session: &mut Session, notices: &mut NoticeQueue, now: i64) {
        let listed = match self.api.list_devices(&session.user).await {
            Ok(listed) => listed,
            // The session ended, as on a vault request.
            Err(error @ (ApiError::DeviceRevoked | ApiError::AccountDeleted)) => {
                self.api_failure(session, notices, error).await;
                return;
            }
            // The list waits for the next pass. It never changes the copy's
            // state, and a list it cannot get is not a vault backoff.
            Err(_) => return,
        };
        let known = self.deps.repo.known_devices().unwrap_or_default();
        let diff = devices::diff(&known, &listed, now);
        if self.deps.repo.insert_known_devices(&diff.to_record).is_ok() {
            for device in &diff.signed_in {
                self.queue(
                    session,
                    notices,
                    NoticeEvent::NewSignIn {
                        server_device_id: device.id.clone(),
                        at_ms: device.created_at_ms,
                    },
                    None,
                );
            }
        }
        session.devices = listed;
    }

    fn daily_insight(&self, session: &Session) -> bool {
        if !session.record.as_ref().is_some_and(|record| record.enabled) {
            return true;
        }
        let Some(this_mac) = session.this_mac() else {
            return true;
        };
        if let Some(view) = &session.newer {
            return view
                .devices
                .get(&this_mac)
                .and_then(|device| device.notify_daily_insight.as_ref())
                .is_none_or(|register| register.value);
        }
        session
            .working
            .as_ref()
            .is_none_or(|document| mirror::daily_insight_here(document, this_mac))
    }

    // --- The snapshot ------------------------------------------------------------

    fn snapshot_of(&self, session: &Session, notices: &NoticeQueue) -> SyncSnapshot {
        let state = session.state();
        let error_code = session
            .record
            .as_ref()
            .and_then(|record| record.last_error.clone());
        let fingerprint = session
            .anchor()
            .trusted
            .filter(|_| session.key.is_some())
            .map(|trusted| trusted.write_pk.fingerprint().to_string());
        let devices = match session.this_mac() {
            Some(this_mac) => devices::rows(
                session.working.as_ref(),
                this_mac,
                &session.devices,
                self.now(),
                self.deps.clock.utc_offset_seconds(),
            ),
            None => Vec::new(),
        };
        let held = session
            .working
            .as_ref()
            .map(mirror::held_summary)
            .unwrap_or_default();
        SyncSnapshot {
            state,
            status_line: self.status_line(session, state, &held, error_code.as_deref()),
            fingerprint,
            devices,
            held,
            pending_reset_effective_at_ms: session
                .pending_reset
                .as_ref()
                .map(|reset| reset.effective_at_ms),
            notices: notices.listed(),
            last_synced_at_ms: session.last_synced_ms,
            error_code,
            offers_code_entry: state.offers_code_entry(),
            rules_over_cap: session.rules_over_cap,
        }
    }

    fn status_line(
        &self,
        session: &Session,
        state: SyncState,
        held: &HeldSummary,
        error_code: Option<&str>,
    ) -> String {
        if let Some(line) = session.line {
            return line.to_owned();
        }
        let line = self.state_line(session, state, held, error_code);
        if session.rules_over_cap && matches!(state, SyncState::Syncing | SyncState::ResetPending) {
            return format!("{line} {RULES_OVER_CAP_LINE}");
        }
        line
    }

    fn state_line(
        &self,
        session: &Session,
        state: SyncState,
        held: &HeldSummary,
        error_code: Option<&str>,
    ) -> String {
        let time = self.time();
        match state {
            SyncState::Off => "Sync is off on this Mac.".into(),
            SyncState::AvailableElsewhere => {
                "Your other Mac keeps your choices in sync. Turn on here?".into()
            }
            SyncState::WaitingForKey => {
                let made = session
                    .vault_created_at_ms
                    .map(|at| format!(" Sync was set up on your account on {}.", time.day(at)))
                    .unwrap_or_default();
                format!(
                    "Waiting for your sync key from iCloud Keychain. Or add this Mac with a code from a Mac that already syncs.{made}"
                )
            }
            SyncState::Syncing if held.site_rules + held.app_rules > 0 || !held.settings.is_empty() => {
                "Synced. Mac names and notification choices are here. Categories, quiet hours and the invitation setting from your other Macs are saved; this Mac uses its own for now.".into()
            }
            SyncState::Syncing => "Synced.".into(),
            SyncState::WaitingForRotatedKey => {
                "Another of your Macs changed the sync key. Add this Mac with a code from it.".into()
            }
            SyncState::ResetPending => match &session.pending_reset {
                Some(reset) => format!(
                    "A reset of your synced copy was requested. It happens on {} at {} unless you cancel.",
                    time.day(reset.effective_at_ms),
                    time.clock(reset.effective_at_ms)
                ),
                None => "A reset of your synced copy was requested.".into(),
            },
            SyncState::ResetCompleted => {
                "Your synced copy was reset. This Mac kept its own choices.".into()
            }
            SyncState::MissingOnServer => {
                "Your synced copy is not on Velvt's server. Nothing on this Mac changed.".into()
            }
            SyncState::ServerCopyUnreadable => {
                "Sync paused: the copy on Velvt's server can't be opened on this Mac. Nothing on this Mac changed.".into()
            }
            SyncState::ServerInconsistent => {
                "Sync paused to keep your data safe. Nothing on this Mac changed.".into()
            }
            SyncState::ReadOnlyNewerSchema => "Update Velvt on this Mac to keep syncing.".into(),
            SyncState::CloneDetected => {
                "This Mac was set up from another Mac's backup. Turn on sync here?".into()
            }
            SyncState::Error if error_code == Some("entitlement_missing") => {
                "This Velvt build can't use iCloud Keychain.".into()
            }
            SyncState::Error => "Sync is paused. Nothing on this Mac changed.".into(),
        }
    }
}

enum PutResult {
    Done,
    Conflict,
}

/// How a typed code's key is taken (`adopt_delivered_key`).
enum Adoption {
    /// The key this Mac already trusts.
    Keep,
    /// Nothing trusted yet: a new anchor.
    Fresh,
    /// A rotation the trusted key signed: the versions go on.
    Rotation(Vec<SignedRotation>),
    /// A new vault, after a reset §6.1 accepts or a deletion its key signed.
    NewVault { deleted: Option<KeyId> },
}

/// `vault_state.last_error` when the row could not be saved.
const STATE_NOT_SAVED: &str = "state_not_saved";

/// Added to the status line while this Mac's rules wait for room in the copy
/// (§4.1, `doc::MAX_LIVE_RULES`). It names no rule.
const RULES_OVER_CAP_LINE: &str =
    "Your synced copy is full at 2,000 categories, so new ones from this Mac stay on this Mac.";

/// Whether a pass lists the account's devices: sync is on here and the
/// session stands. A removal or a deleted account ends the session
/// (`enabled` goes), and a request refused for want of a sign-in would be
/// refused again.
fn lists_devices(session: &Session) -> bool {
    session.record.as_ref().is_some_and(|record| {
        record.enabled && record.last_error.as_deref() != Some(ApiError::NeedsReauth.code())
    })
}

/// A uniform fraction for jitter. Not a secret, so `SystemRandom` is plenty.
fn random_fraction() -> f32 {
    use ring::rand::SecureRandom;
    let mut byte = [0_u8; 1];
    let _ = ring::rand::SystemRandom::new().fill(&mut byte);
    f32::from(byte[0]) / 255.0
}

fn not_signed_in() -> SyncSnapshot {
    SyncSnapshot {
        state: SyncState::Off,
        status_line: "Sign in to use sync.".into(),
        fingerprint: None,
        devices: Vec::new(),
        held: HeldSummary::default(),
        pending_reset_effective_at_ms: None,
        notices: Vec::new(),
        last_synced_at_ms: None,
        error_code: Some("not_signed_in".into()),
        offers_code_entry: false,
        rules_over_cap: false,
    }
}
