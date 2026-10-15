//! §10.4: with sync on, every pass lists the account's devices (`GET
//! /v1/devices`), so a new sign-in posts `new_sign_in` (J6) in every state
//! the copy can be in, not only while it syncs. A pass whose copy waits for a
//! key, is paused, read-only, missing or reset, or backs off after a 412
//! storm, still lists them; a Mac whose session ended lists nothing, and
//! nor does one where sync is off (a Turn On that failed before a vault was
//! made) or a build without the Keychain entitlement.

use serde_json::json;
use velvt_service::vault::crypto::statement::PutStatement;
use velvt_service::vault::crypto::{sha256, BlobName, DocumentContext, VaultKey};
use velvt_service::vault::doc::{DeviceName, SyncDeviceId};
use velvt_service::vault::sync::keys::{KeyStoreError, MemoryKeyStore, VaultKeyStore};
use velvt_service::vault::sync::notices::NoticeKind;
use velvt_service::vault::sync::{SyncSnapshot, SyncState};

use super::fake_core::{error, Clock, FakeCore, StoredBlob, RESET_DELAY_MS};
use super::harness::{live_key, open_copy, put_signature, rotate, user, Mac};
use super::{assert_state, MINUTE, START_MS};

fn setup() -> (FakeCore, Clock) {
    let clock = Clock::at(START_MS);
    (FakeCore::new(clock.clone()), clock)
}

/// A syncing Mac, and the vault's key.
async fn syncing(core: &FakeCore, clock: &Clock) -> (Mac, VaultKey) {
    let person = user("person");
    let mac = Mac::new(core, clock, &person, &MemoryKeyStore::new(), 1).await;
    assert_state(&mac.turn_on("Studio Mac").await, SyncState::Syncing);
    let key = live_key(&mac.keychain, &person).await;
    (mac, key)
}

fn sign_ins(snapshot: &SyncSnapshot) -> usize {
    snapshot
        .notices
        .iter()
        .filter(|notice| notice.kind == NoticeKind::NewSignIn)
        .count()
}

fn listed_devices(core: &FakeCore) -> bool {
    core.received()
        .iter()
        .any(|request| request.method == "GET" && request.path == "/v1/devices")
}

/// Someone signs in to the account while `mac` is in `state`. The next pass,
/// `wait_ms` later, lists the account's devices and posts `new_sign_in` once,
/// and the state stays what it was.
async fn a_sign_in_is_noticed(
    core: &FakeCore,
    clock: &Clock,
    mac: &Mac,
    state: SyncState,
    wait_ms: i64,
) {
    assert_eq!(sign_ins(&mac.engine.snapshot().await), 0);
    let _intruder = core.sign_in(mac.user.as_str(), &uuid::Uuid::new_v4().to_string());
    core.clear_received();
    clock.advance(wait_ms);
    let seen = mac.sync().await;
    assert_state(&seen, state);
    assert!(
        listed_devices(core),
        "{state:?}: GET /v1/devices was not sent"
    );
    assert_eq!(sign_ins(&seen), 1, "{state:?}: no new_sign_in");
    assert!(
        mac.engine
            .take_due_notices()
            .await
            .iter()
            .any(|notice| notice.kind == NoticeKind::NewSignIn),
        "{state:?}: the notice is due"
    );
    let again = mac.sync().await;
    assert_state(&again, state);
    assert_eq!(sign_ins(&again), 1, "{state:?}: posted once");
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_the_copy_has_a_newer_schema() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    // A Mac on a newer Velvt writes a schema-2 copy, signed with the key.
    let current = core.with_account(mac.user.as_str(), |account| account.blob.clone().unwrap());
    let document = open_copy(&key, &mac.user, current.version, &current.ciphertext);
    let mut newer: serde_json::Value =
        serde_json::from_slice(&document.to_json().unwrap()).unwrap();
    newer["schema"] = json!(2);
    let version = current.version + 1;
    let name = BlobName::prefs();
    let context = DocumentContext {
        user_id: &mac.user,
        name: &name,
        key_id: key.key_id(),
        schema: 2,
        version,
    };
    let sealed = key
        .seal_document(&context, &serde_json::to_vec(&newer).unwrap())
        .unwrap();
    let put = PutStatement {
        user_id: mac.user.clone(),
        name: BlobName::prefs(),
        key_id: key.key_id(),
        schema: 2,
        base_version: current.version,
        new_version: version,
        ciphertext_sha256: sha256(&sealed),
    };
    let put_sig = key.sign_statement(&put).unwrap();
    core.with_account(mac.user.as_str(), |account| {
        account.blob = Some(StoredBlob {
            version,
            base_version: current.version,
            schema: 2,
            ciphertext: sealed,
            put_sig,
        });
    });
    clock.advance(MINUTE);
    assert_state(&mac.sync().await, SyncState::ReadOnlyNewerSchema);

    a_sign_in_is_noticed(
        &core,
        &clock,
        &mac,
        SyncState::ReadOnlyNewerSchema,
        10 * MINUTE,
    )
    .await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_sync_is_paused_on_a_key_change() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    let server_key = VaultKey::generate().unwrap();
    core.with_account(mac.user.as_str(), |account| {
        let vault = account.vault.as_mut().unwrap();
        vault.key_id = server_key.key_id();
        vault.write_pk = *server_key.write_public_key();
    });
    clock.advance(MINUTE);
    assert_state(&mac.sync().await, SyncState::ServerInconsistent);

    a_sign_in_is_noticed(
        &core,
        &clock,
        &mac,
        SyncState::ServerInconsistent,
        10 * MINUTE,
    )
    .await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_the_copy_cannot_be_opened() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    // A buggy Mac signed bytes that do not open under the key.
    let garbage = vec![0x5a; velvt_service::vault::crypto::SEALED_SIZES[0]];
    let put_sig = put_signature(&key, &mac.user, 1, 2, &garbage);
    core.with_account(mac.user.as_str(), |account| {
        account.blob = Some(StoredBlob {
            version: 2,
            base_version: 1,
            schema: 1,
            ciphertext: garbage,
            put_sig: velvt_service::vault::crypto::Signature::from_base64(&put_sig).unwrap(),
        });
    });
    clock.advance(MINUTE);
    assert_state(&mac.sync().await, SyncState::ServerCopyUnreadable);

    a_sign_in_is_noticed(
        &core,
        &clock,
        &mac,
        SyncState::ServerCopyUnreadable,
        10 * MINUTE,
    )
    .await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_the_keychain_is_unavailable() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    mac.keychain
        .set_unavailable(Some(KeyStoreError::KeychainUnavailable));
    clock.advance(MINUTE);
    let paused = mac.sync().await;
    assert_state(&paused, SyncState::Error);
    assert_eq!(paused.error_code.as_deref(), Some("keychain_unavailable"));

    a_sign_in_is_noticed(&core, &clock, &mac, SyncState::Error, 10 * MINUTE).await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_the_vault_is_switched_off() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    core.set_enabled(false);
    clock.advance(MINUTE);
    let paused = mac.sync().await;
    assert_eq!(paused.error_code.as_deref(), Some("feature_disabled"));

    a_sign_in_is_noticed(&core, &clock, &mac, SyncState::Syncing, 10 * MINUTE).await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_waiting_for_the_key() {
    let (core, clock) = setup();
    let person = user("person");
    let studio = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 1).await;
    let work = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 3).await;
    studio.turn_on("Studio Mac").await;
    assert_state(&work.turn_on("Work Mac").await, SyncState::WaitingForKey);

    a_sign_in_is_noticed(&core, &clock, &work, SyncState::WaitingForKey, 10 * MINUTE).await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_waiting_for_the_rotated_key() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let work = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 3).await;
    studio.turn_on("Studio Mac").await;
    let old = live_key(&icloud, &person).await;
    work.engine
        .adopt_delivered_key(live_key(&icloud, &person).await)
        .await;
    let new = VaultKey::generate().unwrap();
    icloud
        .store(&person, &new, Some(old.key_id()))
        .await
        .unwrap();
    let answer = rotate(&studio.client(), &person, &old, &new).await;
    assert_eq!(answer.status, 200, "{:?}", answer.raw_body);
    clock.advance(MINUTE);
    assert_state(&work.sync().await, SyncState::WaitingForRotatedKey);

    a_sign_in_is_noticed(
        &core,
        &clock,
        &work,
        SyncState::WaitingForRotatedKey,
        10 * MINUTE,
    )
    .await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_while_the_copy_is_missing_on_the_server() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    core.with_account(mac.user.as_str(), |account| {
        account.vault = None;
        account.blob = None;
    });
    clock.advance(MINUTE);
    assert_state(&mac.sync().await, SyncState::MissingOnServer);

    a_sign_in_is_noticed(&core, &clock, &mac, SyncState::MissingOnServer, 10 * MINUTE).await;
}

#[tokio::test]
async fn a_sign_in_is_noticed_after_a_reset_completed() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    core.request_reset(mac.user.as_str());
    mac.sync().await;
    clock.advance(RESET_DELAY_MS + MINUTE);
    assert_state(&mac.sync().await, SyncState::ResetCompleted);

    a_sign_in_is_noticed(&core, &clock, &mac, SyncState::ResetCompleted, 10 * MINUTE).await;
}

/// After three 412s the vault's requests wait 10 minutes (§10.4 Write step
/// 4); the device list does not.
#[tokio::test]
async fn a_sign_in_is_noticed_while_writes_back_off() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    let this = SyncDeviceId::parse(&mac.row().unwrap().sync_device_id).unwrap();
    mac.engine
        .rename_device(this, DeviceName::new("Renamed").unwrap())
        .await;
    core.lie(|received, account| {
        (received.method == "PUT").then(|| {
            error(
                412,
                "vault_version_conflict",
                Some(json!({"current_version": account.blob.as_ref().unwrap().version})),
            )
        })
    });
    clock.advance(MINUTE);
    let storm = mac.sync().await;
    assert_state(&storm, SyncState::Syncing);
    assert_eq!(storm.error_code.as_deref(), Some("write_conflicts"));
    core.tell_the_truth();

    a_sign_in_is_noticed(&core, &clock, &mac, SyncState::Syncing, 9 * MINUTE).await;
    assert!(
        core.received()
            .iter()
            .all(|request| !request.path.starts_with("/v1/vault")),
        "the backoff still holds the vault's requests"
    );
}

/// A Mac removed from the account, or with sync turned off, lists nothing.
#[tokio::test]
async fn a_mac_whose_session_ended_lists_no_devices() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio.turn_on("Studio Mac").await;
    laptop.turn_on("MacBook Pro").await;

    core.remove_device(person.as_str(), &laptop.device.to_string());
    clock.advance(MINUTE);
    laptop.sync().await;
    core.clear_received();
    clock.advance(10 * MINUTE);
    laptop.sync().await;
    assert!(
        core.received()
            .iter()
            .all(|request| request.device != laptop.device.to_string()),
        "a removed Mac sends nothing"
    );

    studio.engine.turn_off().await;
    core.clear_received();
    clock.advance(10 * MINUTE);
    studio.sync().await;
    assert!(
        !listed_devices(&core),
        "with sync off, the device list is not fetched"
    );
}

/// Turn On that failed before any vault was made (the Keychain unavailable)
/// ends, on the next pass, in "Sync is off on this Mac.": off as after Turn
/// Off, so no pass sends anything, `GET /v1/devices` included, and no
/// new_sign_in is posted, until the person turns sync on again. Then the
/// first list seeds without alerting, as turning on always does.
#[tokio::test]
async fn a_mac_left_off_by_a_failed_turn_on_lists_no_devices() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    icloud.set_unavailable(Some(KeyStoreError::KeychainUnavailable));
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    assert_state(&studio.turn_on("Studio Mac").await, SyncState::Error);
    icloud.set_unavailable(None);
    clock.advance(10 * MINUTE);
    let off = studio.sync().await;
    assert_state(&off, SyncState::Off);
    assert_eq!(off.status_line, "Sync is off on this Mac.");

    let _intruder = core.sign_in(person.as_str(), &uuid::Uuid::new_v4().to_string());
    core.clear_received();
    clock.advance(10 * MINUTE);
    let later = studio.sync().await;
    assert_state(&later, SyncState::Off);
    assert!(core.received().is_empty(), "{:?}", core.received());
    assert_eq!(sign_ins(&later), 0);

    let on = studio.turn_on("Studio Mac").await;
    assert_state(&on, SyncState::Syncing);
    assert!(listed_devices(&core));
    assert_eq!(sign_ins(&on), 0, "turning on seeds the list");
}

/// §14: a build without the Keychain entitlement keeps sync off with "This
/// Velvt build can't use iCloud Keychain.", and its passes send nothing:
/// no device list and no new_sign_in.
#[tokio::test]
async fn a_build_without_the_keychain_entitlement_lists_no_devices() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    icloud.set_unavailable(Some(KeyStoreError::EntitlementMissing));
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let on = studio.turn_on("Studio Mac").await;
    assert_state(&on, SyncState::Error);
    assert!(core.received().is_empty(), "{:?}", core.received());

    let _intruder = core.sign_in(person.as_str(), &uuid::Uuid::new_v4().to_string());
    clock.advance(10 * MINUTE);
    let later = studio.sync().await;
    assert_state(&later, SyncState::Error);
    assert_eq!(
        later.status_line,
        "This Velvt build can't use iCloud Keychain."
    );
    assert!(core.received().is_empty(), "{:?}", core.received());
    assert_eq!(sign_ins(&later), 0);
}
