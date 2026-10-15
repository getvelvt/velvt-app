//! Two Macs, one fake velvt-core (design §16.1 "Two-Mac integration test").

use chrono::DateTime;
use velvt_service::delivery::InsightNotificationRoute;
use velvt_service::persistence::{VaultRuleLinkRecord, VelvtQuietHours};
use velvt_service::vault::doc::{DeviceName, SyncDeviceId};
use velvt_service::vault::sync::keys::{KeyStoreError, MemoryKeyStore, VaultKeyStore};
use velvt_service::vault::sync::notices::NoticeKind;
use velvt_service::vault::sync::SyncState;

use super::fake_core::{stored_version, Clock, FakeCore, RESET_DELAY_MS};
use super::harness::{cancel_reset, delete_synced_data, live_key, open_copy, rotate, user, Mac};
use super::{assert_state, DAY, MINUTE, START_MS};

fn setup() -> (FakeCore, Clock) {
    let clock = Clock::at(START_MS);
    (FakeCore::new(clock.clone()), clock)
}

fn link(seed: char, category: &str, hlc_ms: i64, mac: &Mac) -> VaultRuleLinkRecord {
    let this_mac = mac.row().unwrap().sync_device_id;
    VaultRuleLinkRecord {
        local_kind: "site".into(),
        local_key_hash: seed.to_string().repeat(64),
        person_key: seed.to_string().repeat(64),
        category: category.into(),
        deleted: false,
        hlc: format!("{hlc_ms}.0000.{this_mac}"),
    }
}

fn kinds(snapshot: &velvt_service::vault::sync::SyncSnapshot) -> Vec<NoticeKind> {
    snapshot.notices.iter().map(|notice| notice.kind).collect()
}

/// J1, J2: the first Mac creates the vault; a second on the same Apple ID
/// finds the key in its Keychain and joins with one click; each sees the
/// other's name; the first is told a Mac joined.
#[tokio::test]
async fn turn_on_then_a_second_mac_joins_by_its_keychain() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;

    let on = studio.turn_on("Studio Mac").await;
    assert_state(&on, SyncState::Syncing);
    assert_eq!(stored_version(&core, person.as_str()), Some(1));
    assert_eq!(
        icloud.held(&person).len(),
        1,
        "Keychain first, server second"
    );
    let fingerprint = on.fingerprint.clone().unwrap();
    assert_eq!(fingerprint.len(), 14);

    let card = laptop.engine.probe_available().await;
    assert_state(&card, SyncState::AvailableElsewhere);

    clock.advance(MINUTE);
    let joined = laptop.turn_on("MacBook Pro").await;
    assert_state(&joined, SyncState::Syncing);
    assert_eq!(
        joined.fingerprint,
        Some(fingerprint),
        "the same digits on every Mac"
    );
    let names: Vec<_> = joined
        .devices
        .iter()
        .filter_map(|row| row.name.clone())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"Studio Mac".to_owned()) && names.contains(&"MacBook Pro".to_owned()));

    clock.advance(MINUTE);
    let studio_view = studio.sync().await;
    assert!(kinds(&studio_view).contains(&NoticeKind::MacJoinedSync));
    let joined_notice = studio_view
        .notices
        .iter()
        .find(|notice| notice.kind == NoticeKind::MacJoinedSync)
        .unwrap();
    assert!(joined_notice.body.contains("“MacBook Pro”"));
    assert!(joined_notice.body.contains("using your iCloud Keychain"));
    // The first list seeded the known devices: neither Mac's own sign-in, nor
    // the other's before sync was on, is a "new sign-in".
    assert!(!kinds(&studio_view).contains(&NoticeKind::NewSignIn));
    assert!(!kinds(&joined).contains(&NoticeKind::NewSignIn));
}

/// J3: a Mac on another Apple ID waits for the key, offers a code, and joins
/// with the key the code delivered (A5 claims and opens the code).
#[tokio::test]
async fn a_mac_on_another_apple_id_waits_then_joins_by_code() {
    let (core, clock) = setup();
    let person = user("person");
    let studio = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 1).await;
    let work = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 3).await;
    studio.turn_on("Studio Mac").await;

    let waiting = work.turn_on("Work Mac").await;
    assert_state(&waiting, SyncState::WaitingForKey);
    assert!(waiting.offers_code_entry);
    assert!(waiting
        .status_line
        .contains("Sync was set up on your account on 15 Oct"));
    assert_eq!(
        core.received()
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/v1/vault")
            .count(),
        1,
        "the waiting Mac never creates"
    );

    let key = live_key(&studio.keychain, &person).await;
    let joined = work.engine.adopt_delivered_key(key).await;
    assert_state(&joined, SyncState::Syncing);
    assert_eq!(
        work.keychain.held(&person).len(),
        1,
        "stored in its own Keychain"
    );

    clock.advance(MINUTE);
    let seen = studio.sync().await;
    let notice = seen
        .notices
        .iter()
        .find(|notice| notice.kind == NoticeKind::MacJoinedSync)
        .unwrap();
    assert!(
        notice.body.contains("using a setup code"),
        "{}",
        notice.body
    );
}

/// Concurrent rule edits on two Macs converge; a rename of the other Mac and
/// that Mac's own routing change both survive (per-field registers).
#[tokio::test]
async fn concurrent_edits_converge_and_both_field_changes_survive() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio.turn_on("Studio Mac").await;
    laptop.turn_on("MacBook Pro").await;
    studio.sync().await;

    let repo_a = studio.persistence.vault_state_repo();
    let repo_b = laptop.persistence.vault_state_repo();
    repo_a
        .upsert_rule_link(&link('a', "SOCIAL_FEED", START_MS + 10, &studio))
        .unwrap();
    repo_b
        .upsert_rule_link(&link('b', "FOCUS_WORK", START_MS + 11, &laptop))
        .unwrap();
    // Both Macs edit the same rule; the later HLC wins on both.
    repo_a
        .upsert_rule_link(&link('c', "REFERENCE", START_MS + 12, &studio))
        .unwrap();
    repo_b
        .upsert_rule_link(&link('c', "COMMUNICATION", START_MS + 13, &laptop))
        .unwrap();

    let laptop_id = SyncDeviceId::parse(&laptop.row().unwrap().sync_device_id).unwrap();
    studio
        .engine
        .rename_device(laptop_id, DeviceName::new("Kevin's MacBook").unwrap())
        .await;
    laptop.engine.set_daily_insight(laptop_id, false).await;

    clock.advance(MINUTE);
    studio.sync().await;
    laptop.sync().await; // 412 against Studio's write: fetch, merge, retry.
    studio.sync().await;

    let a = studio.sync().await;
    let b = laptop.sync().await;
    assert_eq!(a.held.site_rules, 3);
    assert_eq!(b.held.site_rules, 3);
    let key = live_key(&icloud, &person).await;
    let version = stored_version(&core, person.as_str()).unwrap();
    let ciphertext = core.with_account(person.as_str(), |account| {
        account.blob.clone().unwrap().ciphertext
    });
    let document = open_copy(&key, &person, version, &ciphertext);
    let c_rule = document
        .rules
        .iter()
        .find(|(key, _)| key.key.to_hex().starts_with('c'))
        .unwrap()
        .1;
    assert_eq!(c_rule.category.as_str(), "COMMUNICATION");
    let entry = &document.devices[&laptop_id];
    assert_eq!(entry.name.value.as_str(), "Kevin's MacBook");
    assert!(!entry.notify_daily_insight.value);
    // The routing register is applied on the laptop alone.
    assert!(!laptop.routing.daily_insight_notifications());
    assert!(studio.routing.daily_insight_notifications());
}

/// §11, §10.1: quiet hours, the invitations switch and category rules from
/// another Mac are held. After syncing a copy whose values differ, every
/// local policy table on the receiving Mac is exactly what it was.
#[tokio::test]
async fn held_values_never_reach_a_local_policy_table() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio
        .persistence
        .focus_repo()
        .set_quiet_hours(&VelvtQuietHours {
            start_local_minutes: 1320,
            end_local_minutes: 420,
            rule_version: 1,
            configured_at: DateTime::from_timestamp(START_MS / 1_000 - 60, 0).unwrap(),
        })
        .unwrap();
    studio
        .persistence
        .initiation_repo()
        .set_invitations_enabled(
            false,
            DateTime::from_timestamp(START_MS / 1_000 - 60, 0).unwrap(),
        )
        .unwrap();
    studio.turn_on("Studio Mac").await;
    studio
        .persistence
        .vault_state_repo()
        .upsert_rule_link(&link('d', "SOCIAL_FEED", START_MS + 1, &studio))
        .unwrap();
    studio.sync().await;

    let tables = |mac: &Mac| -> Vec<String> {
        vec![
            format!("{:?}", mac.persistence.focus_repo().quiet_hours().unwrap()),
            format!(
                "{:?}",
                mac.persistence
                    .initiation_repo()
                    .invitations_enabled()
                    .unwrap()
            ),
            format!(
                "{:?}",
                mac.persistence
                    .abstraction_map_repo()
                    .search_personal_overrides(None, 0, 100)
                    .unwrap()
            ),
        ]
    };
    let before = tables(&laptop);
    let joined = laptop.turn_on("MacBook Pro").await;
    assert_state(&joined, SyncState::Syncing);
    assert_eq!(joined.held.site_rules, 1);
    assert_eq!(
        joined.held.settings,
        vec!["quiet_hours", "invitations_enabled"]
    );
    assert!(joined.status_line.contains("this Mac uses its own for now"));
    assert_eq!(tables(&laptop), before, "nothing held was applied");
    assert_eq!(before[0], "None");
    assert_eq!(before[1], "true");
}

/// A security notice that comes due during a work block is posted only once
/// the block ends; the device list shows it at once (J6, §11.3).
#[tokio::test]
async fn a_notice_due_during_a_work_block_waits_for_the_block_to_end() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;
    assert!(studio.engine.take_due_notices().await.is_empty());

    studio.blocks.set(true);
    let _intruder = core.sign_in(person.as_str(), &uuid::Uuid::new_v4().to_string());
    clock.advance(10 * MINUTE);
    let listed = studio.sync().await;
    assert!(
        kinds(&listed).contains(&NoticeKind::NewSignIn),
        "listed at once"
    );
    assert!(
        studio.engine.take_due_notices().await.is_empty(),
        "held mid-block"
    );

    studio.blocks.set(false);
    let posted = studio.engine.take_due_notices().await;
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].kind, NoticeKind::NewSignIn);
    assert!(
        studio.engine.take_due_notices().await.is_empty(),
        "posted once"
    );
}

/// J5, §5.7: Mac A removes Mac B. B's next request is refused; B ends its
/// session, keeps its anchor, and says it was removed.
#[tokio::test]
async fn a_removed_mac_ends_its_session_and_keeps_its_anchor() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio.turn_on("Studio Mac").await;
    laptop.turn_on("MacBook Pro").await;
    let anchor = laptop.row().unwrap();

    core.remove_device(person.as_str(), &laptop.device.to_string());
    let removed = laptop.sync().await;
    assert_state(&removed, SyncState::Off);
    assert!(kinds(&removed).contains(&NoticeKind::ThisMacRemoved));
    let row = laptop.row().unwrap();
    assert!(!row.enabled && row.ciphertext.is_none());
    assert_eq!(row.key_id, anchor.key_id);
    assert_eq!(row.highest_seen_version, anchor.highest_seen_version);
    assert_eq!(row.sync_device_id, anchor.sync_device_id);
    assert_eq!(icloud.held(&person).len(), 1, "no Keychain item touched");
    assert!(laptop.routing.daily_insight_notifications());
}

/// §5.6: a signed rotation. A Mac on the same Apple ID finds K' in its
/// Keychain and adopts it silently, marking the old item retired; a Mac on
/// another Apple ID waits for the rotated key and may enter a code.
#[tokio::test]
async fn a_signed_rotation_is_adopted_where_the_new_key_arrives() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    let work = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 3).await;
    studio.turn_on("Studio Mac").await;
    laptop.turn_on("MacBook Pro").await;
    let old = live_key(&icloud, &person).await;
    work.engine
        .adopt_delivered_key(live_key(&icloud, &person).await)
        .await;

    icloud.set_clock_ms(clock.now());
    let new = velvt_service::vault::crypto::VaultKey::generate().unwrap();
    icloud
        .store(&person, &new, Some(old.key_id()))
        .await
        .unwrap();
    let answer = rotate(&studio.client(), &person, &old, &new).await;
    assert_eq!(answer.status, 200, "{:?}", answer.raw_body);

    clock.advance(MINUTE);
    let adopted = laptop.sync().await;
    assert_state(&adopted, SyncState::Syncing);
    assert_eq!(laptop.row().unwrap().key_id, Some(new.key_id().to_hex()));
    assert!(kinds(&adopted).contains(&NoticeKind::SyncKeyChanged));
    let old_item = icloud
        .held(&person)
        .into_iter()
        .find(|held| held.key_id == old.key_id())
        .unwrap();
    assert!(old_item.retired_by.is_some(), "retired, not deleted");

    let waiting = work.sync().await;
    assert_state(&waiting, SyncState::WaitingForRotatedKey);
    assert!(
        waiting.offers_code_entry,
        "a verified rotation makes a code safe"
    );
    let rejoined = work.engine.adopt_delivered_key(new).await;
    assert_state(&rejoined, SyncState::Syncing);

    // 30 days after K' was made, the retired item goes (§5.6 step 7).
    clock.advance(31 * DAY);
    icloud.set_clock_ms(clock.now());
    laptop.sync().await;
    assert!(icloud
        .held(&person)
        .iter()
        .all(|held| held.key_id != old.key_id()));
}

/// J9: Delete Synced Data on one Mac. The others verify the signed
/// tombstone, turn sync off quietly, and delete their own copy of that key's
/// item; a Mac on another Apple ID too.
#[tokio::test]
async fn a_signed_deletion_turns_sync_off_everywhere() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let work = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 3).await;
    studio.turn_on("Studio Mac").await;
    let key = live_key(&icloud, &person).await;
    work.engine
        .adopt_delivered_key(live_key(&icloud, &person).await)
        .await;
    assert_eq!(work.keychain.held(&person).len(), 1);

    let answer = delete_synced_data(&studio.client(), &person, &key, clock.now()).await;
    assert_eq!(answer.status, 204);
    clock.advance(MINUTE);
    let off = work.sync().await;
    assert_state(&off, SyncState::Off);
    assert_eq!(
        off.status_line,
        "Sync was turned off from another of your Macs."
    );
    assert!(
        work.row().is_none(),
        "the row goes with a verified deletion"
    );
    assert!(
        work.keychain.held(&person).is_empty(),
        "its own item for that key"
    );
    assert!(!kinds(&off).contains(&NoticeKind::ServerInconsistent));
}

/// J9, J6: a reset requested without the key is noticed by every syncing
/// Mac once, shown until it is cancelled, and forgotten after a signed
/// cancel.
#[tokio::test]
async fn a_reset_request_is_noticed_and_a_cancel_clears_it() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;

    let reset_id = core.request_reset(person.as_str());
    clock.advance(MINUTE);
    let pending = studio.sync().await;
    assert_state(&pending, SyncState::ResetPending);
    assert!(kinds(&pending).contains(&NoticeKind::ResetRequested));
    assert_eq!(
        pending.pending_reset_effective_at_ms,
        Some(START_MS + RESET_DELAY_MS)
    );
    assert_eq!(
        studio.row().unwrap().seen_reset_request_id,
        Some(reset_id.clone())
    );

    let key = live_key(&icloud, &person).await;
    assert_eq!(
        cancel_reset(&studio.client(), &person, &key, &reset_id)
            .await
            .status,
        204
    );
    let back = studio.sync().await;
    assert_state(&back, SyncState::Syncing);
    assert_eq!(studio.row().unwrap().seen_reset_request_id, None);
}

/// J9 without the key: a reset this Mac saw pending, and that completed 72
/// hours later, is accepted; [Set Up Sync] then makes a new vault under a new
/// key and replaces the anchor.
#[tokio::test]
async fn a_completed_reset_this_mac_saw_is_accepted_and_sync_can_start_over() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;
    let old = studio.row().unwrap().key_id;
    core.request_reset(person.as_str());
    studio.sync().await;

    clock.advance(RESET_DELAY_MS + MINUTE);
    let completed = studio.sync().await;
    assert_state(&completed, SyncState::ResetCompleted);
    assert!(kinds(&completed).contains(&NoticeKind::ResetCompleted));
    assert!(completed.offers_code_entry);

    let again = studio.turn_on("Studio Mac").await;
    assert_state(&again, SyncState::Syncing);
    let row = studio.row().unwrap();
    assert_ne!(row.key_id, old);
    assert_eq!(
        row.highest_seen_version, 1,
        "a new vault's versions start again"
    );
    assert_eq!(
        icloud.held(&person).len(),
        2,
        "no server claim deleted the old item"
    );
}

/// J7: Log Out keeps the anchor and the Keychain item; signing back in shows
/// the copy again with one click, under the same entry.
#[tokio::test]
async fn log_out_and_back_in_keeps_the_anchor_and_the_entry() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let mut studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;
    let before = studio.row().unwrap();

    studio.engine.log_out().await;
    let row = studio.row().unwrap();
    assert!(!row.enabled && row.ciphertext.is_none() && row.last_hlc.is_none());
    assert_eq!(row.key_id, before.key_id);
    assert_eq!(row.highest_seen_version, before.highest_seen_version);
    assert_eq!(
        icloud.held(&person).len(),
        1,
        "Log Out never deletes an item"
    );

    studio.sign_back_in().await;
    let on = studio.turn_on("Studio Mac").await;
    assert_state(&on, SyncState::Syncing);
    assert_eq!(studio.row().unwrap().sync_device_id, before.sync_device_id);
    assert_eq!(
        on.devices.iter().filter(|row| row.name.is_some()).count(),
        1,
        "one entry, not two"
    );
}

/// An account switch ends the other account's session and keeps its
/// anchor; each account has its own row.
#[tokio::test]
async fn an_account_switch_keeps_each_account_s_anchor_apart() {
    let (core, clock) = setup();
    let (first, second) = (user("first"), user("second"));
    let icloud = MemoryKeyStore::new();
    let mut mac = Mac::new(&core, &clock, &first, &icloud, 1).await;
    mac.turn_on("Mac").await;
    let first_row = mac.row().unwrap();

    mac.switch_account(&second).await;
    let repo = mac.persistence.vault_state_repo();
    let kept = repo.vault_state(first.as_str()).unwrap().unwrap();
    assert!(!kept.enabled && kept.ciphertext.is_none());
    assert_eq!(kept.key_id, first_row.key_id);
    assert!(
        repo.known_devices().unwrap().is_empty(),
        "the session tables go with it"
    );
    let second_on = mac.turn_on("Mac").await;
    assert_state(&second_on, SyncState::Syncing);
    assert_ne!(mac.row().unwrap().key_id, first_row.key_id);
    assert_eq!(repo.vault_states().unwrap().len(), 2);
}

/// J10: the account is deleted while this Mac is offline. When it next asks,
/// it deletes its rows and every key item for the account, retired ones too.
#[tokio::test]
async fn an_account_deleted_elsewhere_is_forgotten_here() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;
    let stale = velvt_service::vault::crypto::VaultKey::generate().unwrap();
    icloud.insert(
        &person,
        &stale,
        velvt_service::vault::sync::keys::HeldKey {
            key_id: stale.key_id(),
            supersedes: None,
            retired_by: Some(velvt_service::vault::sync::keys::RetiredBy::V2),
            created_at_ms: None,
        },
    );

    core.delete_account(person.as_str());
    clock.advance(DAY);
    studio.sync().await;
    assert!(studio.row().is_none());
    assert!(icloud.held(&person).is_empty());
}

/// J8: the database copied to another Mac (Migration Assistant) no longer
/// matches its host binding: that Mac takes a new sync_device_id and turns
/// sync off, and the source Mac's entry stays its own.
#[tokio::test]
async fn a_migration_assistant_copy_takes_a_new_identity() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let mut mac = Mac::new(&core, &clock, &person, &icloud, 1).await;
    mac.turn_on("Studio Mac").await;
    let before = mac.row().unwrap();

    mac.host.set([9; 16]);
    mac.restart().await;
    let card = mac.engine.snapshot().await;
    assert_state(&card, SyncState::CloneDetected);
    let row = mac.row().unwrap();
    assert_ne!(row.sync_device_id, before.sync_device_id);
    assert!(!row.enabled);
    assert_eq!(
        row.key_id, before.key_id,
        "the trust anchor is the person's"
    );

    let on = mac.turn_on("New Mac").await;
    assert_state(&on, SyncState::Syncing);
    assert_eq!(
        on.devices.iter().filter(|row| row.name.is_some()).count(),
        2
    );
}

/// Turn Off Sync on This Mac marks its entry removed for the other Macs, and
/// keeps the anchor and the Keychain item.
#[tokio::test]
async fn turning_sync_off_here_marks_this_mac_removed() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio.turn_on("Studio Mac").await;
    laptop.turn_on("MacBook Pro").await;

    let off = laptop.engine.turn_off().await;
    assert_state(&off, SyncState::Off);
    assert_eq!(icloud.held(&person).len(), 1);
    assert!(laptop.row().unwrap().key_id.is_some());
    let seen = studio.sync().await;
    assert_eq!(
        seen.devices.iter().filter(|row| row.name.is_some()).count(),
        1
    );
}

/// A restart resumes from the cached copy, sealed, and the HLC never goes
/// backwards.
#[tokio::test]
async fn a_restart_resumes_from_the_sealed_cache() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let mut studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;
    core.clear_received();
    studio.restart().await;
    let resumed = studio.engine.snapshot().await;
    assert_state(&resumed, SyncState::Syncing);
    assert_eq!(
        resumed
            .devices
            .iter()
            .filter(|row| row.name.is_some())
            .count(),
        1
    );
    assert!(
        core.received().is_empty(),
        "resuming reads the cache, not the server"
    );
    let synced = studio.sync().await;
    assert_state(&synced, SyncState::Syncing);
    assert!(
        core.received()
            .iter()
            .all(|request| request.method != "PUT"),
        "nothing changed, nothing written"
    );
}

/// §14: the Keychain unavailable, or a build without the entitlement, keeps
/// sync off with a reason, and nothing is created.
#[tokio::test]
async fn without_the_keychain_nothing_is_created() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    icloud.set_unavailable(Some(KeyStoreError::EntitlementMissing));
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let on = studio.turn_on("Studio Mac").await;
    assert_state(&on, SyncState::Error);
    assert_eq!(
        on.status_line,
        "This Velvt build can't use iCloud Keychain."
    );
    assert!(core
        .received()
        .iter()
        .all(|request| request.method == "GET" || request.path != "/v1/vault"));
    assert_eq!(stored_version(&core, person.as_str()), None);
}

/// §14: velvt-core with the vault switched off: sync pauses silently and the
/// state stays what it was.
#[tokio::test]
async fn a_server_with_the_vault_off_pauses_sync_silently() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    studio.turn_on("Studio Mac").await;
    core.set_enabled(false);
    let paused = studio.sync().await;
    assert_state(&paused, SyncState::Syncing);
    assert_eq!(paused.error_code.as_deref(), Some("feature_disabled"));
    assert!(paused.notices.is_empty());
}

/// Two Macs turn sync on at once (§14), before iCloud Keychain has carried
/// either key: one create wins, the other deletes only its own orphan item,
/// waits, and adopts the winner's vault when its key arrives.
#[tokio::test]
async fn two_macs_turning_on_at_once_end_with_one_vault() {
    let (core, clock) = setup();
    let person = user("person");
    let studio = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 1).await;
    let laptop = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 2).await;
    // The laptop asked GET /v1/vault before the studio's create landed.
    let once = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = std::sync::Arc::clone(&once);
    let laptop_device = laptop.device.to_string();
    core.lie(move |received, _| {
        (received.device == laptop_device
            && received.method == "GET"
            && received.path == "/v1/vault"
            && flag.swap(false, std::sync::atomic::Ordering::SeqCst))
        .then(|| super::fake_core::error(404, "vault_not_found", Some(serde_json::json!({}))))
    });
    studio.turn_on("Studio Mac").await;
    let lost = laptop.turn_on("MacBook Pro").await;
    assert_state(&lost, SyncState::WaitingForKey);
    assert!(
        laptop.keychain.held(&person).is_empty(),
        "its own orphan, and only that, deleted"
    );
    assert_eq!(studio.keychain.held(&person).len(), 1);

    // iCloud Keychain delivers the studio's key.
    let key = live_key(&studio.keychain, &person).await;
    laptop.keychain.insert(
        &person,
        &key,
        velvt_service::vault::sync::keys::HeldKey {
            key_id: key.key_id(),
            supersedes: None,
            retired_by: None,
            created_at_ms: Some(START_MS),
        },
    );
    let adopted = laptop.sync().await;
    assert_state(&adopted, SyncState::Syncing);
    assert_eq!(laptop.row().unwrap().key_id, studio.row().unwrap().key_id);
    assert_eq!(
        stored_version(&core, person.as_str()),
        Some(2),
        "the laptop's entry written"
    );
}

/// I1, I2, §16.1 "Privacy (Rust)": no request body carries `K`, the
/// person salt, a Mac's name or any other document plaintext, and nothing on
/// disk holds them outside the sealed copy.
#[tokio::test]
async fn no_key_salt_or_plaintext_leaves_the_mac_or_reaches_the_database() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;

    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio.turn_on("Sentinel Studio Mac").await;
    studio
        .persistence
        .vault_state_repo()
        .upsert_rule_link(&link('e', "SOCIAL_FEED", START_MS + 1, &studio))
        .unwrap();
    studio.sync().await;
    laptop.turn_on("Sentinel Laptop").await;
    studio.sync().await;

    let key = live_key(&icloud, &person).await;
    let version = stored_version(&core, person.as_str()).unwrap();
    let ciphertext = core.with_account(person.as_str(), |account| {
        account.blob.clone().unwrap().ciphertext
    });
    let document = open_copy(&key, &person, version, &ciphertext);
    let salt = serde_json::to_value(&document).unwrap()["person_salt"]
        .as_str()
        .unwrap()
        .to_owned();
    let secrets = [
        key.to_base64().to_string(),
        salt,
        "Sentinel".to_owned(),
        "SOCIAL_FEED".to_owned(),
    ];
    let received = core.received();
    assert!(received.iter().any(|request| request.method == "PUT"));
    for request in &received {
        assert_eq!(request.user, person.as_str());
        let text = request
            .body
            .as_ref()
            .map(|body| body.to_string())
            .unwrap_or_default();
        for secret in &secrets {
            assert!(
                !text.contains(secret.as_str()),
                "{} {} carries a secret",
                request.method,
                request.path
            );
        }
    }
    for mac in [&studio, &laptop] {
        let row = mac.row().unwrap();
        let sealed = row.ciphertext.clone().unwrap();
        for secret in &secrets {
            assert!(!sealed
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()));
        }
        let raw_key = BASE64.decode(key.to_base64().as_bytes()).unwrap();
        assert!(!sealed
            .windows(raw_key.len())
            .any(|window| window == raw_key));
        assert!(
            format!("{row:?}").len() < 2_000,
            "Debug prints lengths, not bytes"
        );
    }
}
