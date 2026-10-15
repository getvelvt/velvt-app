//! The malicious-server suite (design §16.1): one named test per attack. In
//! every one, the Mac never overwrites the copy it could not trust, never
//! creates a vault on a 404, never adopts a key it cannot verify, and never
//! offers code entry where the server could be steering the person.
//!
//! Two rows of the design's table belong to the setup-code PR (A5), which
//! claims and opens handoffs: "a substituted handoff payload" and "a
//! replayed, claimed handoff". The payload's AEAD is A1's and is tested
//! there (`vault::code`).

use serde_json::json;
use velvt_service::vault::crypto::statement::SignedRotation;
use velvt_service::vault::crypto::VaultKey;
use velvt_service::vault::sync::keys::{HeldKey, MemoryKeyStore, RetiredBy};
use velvt_service::vault::sync::notices::NoticeKind;
use velvt_service::vault::sync::SyncState;

use super::fake_core::{
    blob_response, response, stored_version, Clock, FakeCore, StoredBlob, RESET_DELAY_MS,
};
use super::harness::{live_key, open_copy, put_signature, seal_copy, user, Mac};
use super::{assert_state, DAY, HOUR, MINUTE, START_MS};

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

/// Nothing written and nothing created since `mark`.
fn wrote_nothing(core: &FakeCore) {
    for request in core.received() {
        assert!(
            request.method == "GET",
            "{} {} after the server lied",
            request.method,
            request.path
        );
    }
}

/// The paused states never offer a code; the notice never does either.
fn paused_without_a_code(snapshot: &velvt_service::vault::sync::SyncSnapshot) {
    assert!(!snapshot.offers_code_entry, "{:?}", snapshot.state);
    for notice in &snapshot.notices {
        assert!(
            !notice.body.to_lowercase().contains("code"),
            "{}",
            notice.body
        );
    }
}

/// A blob the server swaps in for the stored one.
fn serve_blob(core: &FakeCore, user: &str, version: u64, ciphertext: Vec<u8>, put_sig_b64: String) {
    core.with_account(user, |account| {
        account.blob = Some(StoredBlob {
            version,
            base_version: version - 1,
            schema: 1,
            ciphertext,
            put_sig: velvt_service::vault::crypto::Signature::from_base64(&put_sig_b64).unwrap(),
        });
    });
}

#[tokio::test]
async fn another_accounts_copy_is_refused_and_never_overwritten() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    let stranger = user("stranger");
    let other = Mac::new(&core, &clock, &stranger, &MemoryKeyStore::new(), 7).await;
    other.turn_on("Their Mac").await;
    let theirs = core.with_account(stranger.as_str(), |account| account.blob.clone().unwrap());
    core.with_account(mac.user.as_str(), |account| {
        account.blob = Some(StoredBlob {
            version: 2,
            ..theirs.clone()
        })
    });
    core.clear_received();
    clock.advance(MINUTE);

    let paused = mac.sync().await;
    assert_state(&paused, SyncState::ServerInconsistent);
    paused_without_a_code(&paused);
    wrote_nothing(&core);

    // Even signed by this account's own key, a copy sealed for another
    // account does not open: user_id is in the AAD.
    let document = open_copy(
        &live_key(&other.keychain, &stranger).await,
        &stranger,
        1,
        &theirs.ciphertext,
    );
    let sealed_for_stranger = seal_copy(&key, &stranger, 3, &document);
    serve_blob(
        &core,
        mac.user.as_str(),
        3,
        sealed_for_stranger.clone(),
        put_signature(&key, &mac.user, 2, 3, &sealed_for_stranger),
    );
    core.clear_received();
    let unreadable = mac.sync().await;
    assert_state(&unreadable, SyncState::ServerCopyUnreadable);
    paused_without_a_code(&unreadable);
    wrote_nothing(&core);
}

#[tokio::test]
async fn an_older_copy_at_its_old_version_is_never_taken_as_current() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    let first = core.with_account(mac.user.as_str(), |account| account.blob.clone().unwrap());
    let laptop_id =
        velvt_service::vault::doc::SyncDeviceId::parse(&mac.row().unwrap().sync_device_id).unwrap();
    mac.engine
        .rename_device(
            laptop_id,
            velvt_service::vault::doc::DeviceName::new("Renamed").unwrap(),
        )
        .await;
    mac.sync().await;
    assert_eq!(stored_version(&core, mac.user.as_str()), Some(2));

    // The server replays version 1, validly signed.
    core.with_account(mac.user.as_str(), |account| {
        account.blob = Some(first.clone())
    });
    clock.advance(MINUTE);
    let snapshot = mac.sync().await;
    assert_state(&snapshot, SyncState::Syncing);
    let row = mac.row().unwrap();
    assert_eq!(
        row.highest_seen_version, 3,
        "merged, then written at highest + 1"
    );
    let named: Vec<_> = snapshot
        .devices
        .iter()
        .filter_map(|row| row.name.clone())
        .collect();
    assert_eq!(named, vec!["Renamed".to_owned()], "the older register lost");
}

#[tokio::test]
async fn an_old_copy_relabelled_with_a_new_version_is_refused() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    core.with_account(mac.user.as_str(), |account| {
        let blob = account.blob.as_mut().unwrap();
        blob.version = 9;
        blob.base_version = 8;
    });
    core.clear_received();
    clock.advance(MINUTE);
    let paused = mac.sync().await;
    // The put signature binds the version, so the relabel fails before the
    // AEAD, which binds it too (A1: changing_any_context_field_fails_to_open).
    assert_state(&paused, SyncState::ServerInconsistent);
    assert_eq!(paused.error_code.as_deref(), Some("bad_write_signature"));
    paused_without_a_code(&paused);
    wrote_nothing(&core);
}

#[tokio::test]
async fn a_copy_with_a_bad_write_signature_pauses_sync() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    let current = core.with_account(mac.user.as_str(), |account| account.blob.clone().unwrap());
    let document = open_copy(&key, &mac.user, 1, &current.ciphertext);
    let sealed = seal_copy(&key, &mac.user, 2, &document);
    // Signed by a key the server holds, not the vault's.
    serve_blob(
        &core,
        mac.user.as_str(),
        2,
        sealed.clone(),
        put_signature(&VaultKey::generate().unwrap(), &mac.user, 1, 2, &sealed),
    );
    core.clear_received();
    let paused = mac.sync().await;
    assert_state(&paused, SyncState::ServerInconsistent);
    assert!(paused
        .notices
        .iter()
        .any(|notice| notice.kind == NoticeKind::ServerInconsistent));
    paused_without_a_code(&paused);
    wrote_nothing(&core);
}

#[tokio::test]
async fn a_new_key_with_no_rotation_and_no_reset_this_mac_saw_pauses_sync_and_offers_no_code() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    let server_key = VaultKey::generate().unwrap();
    core.with_account(mac.user.as_str(), |account| {
        let vault = account.vault.as_mut().unwrap();
        vault.key_id = server_key.key_id();
        vault.write_pk = *server_key.write_public_key();
    });
    core.clear_received();
    let paused = mac.sync().await;
    assert_state(&paused, SyncState::ServerInconsistent);
    assert_eq!(paused.error_code.as_deref(), Some("unverified_key_change"));
    paused_without_a_code(&paused);
    wrote_nothing(&core);
}

#[tokio::test]
async fn a_new_key_with_a_forged_rotation_is_refused() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    let server_key = VaultKey::generate().unwrap();
    // Signed by the new key in both places: the server cannot sign as the old.
    let mut forged = SignedRotation::sign(mac.user.clone(), &key, &server_key, 2).unwrap();
    forged.old_signature = forged.new_signature;
    core.with_account(mac.user.as_str(), |account| {
        let vault = account.vault.as_mut().unwrap();
        vault.key_id = server_key.key_id();
        vault.write_pk = *server_key.write_public_key();
        vault.rotations.push(forged.to_bytes());
    });
    core.clear_received();
    let paused = mac.sync().await;
    assert_state(&paused, SyncState::ServerInconsistent);
    paused_without_a_code(&paused);
    wrote_nothing(&core);
    assert_eq!(
        mac.row().unwrap().key_id,
        Some(key.key_id().to_hex()),
        "the old key is still trusted"
    );
}

#[tokio::test]
async fn a_forged_tombstone_is_refused() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    let forged = velvt_service::vault::crypto::statement::Signed::sign(
        &VaultKey::generate().unwrap(),
        velvt_service::vault::crypto::statement::DeleteStatement {
            user_id: mac.user.clone(),
            key_id: key.key_id(),
            at_version: 1,
            requested_at: clock.now() / 1_000,
        },
    )
    .unwrap();
    core.with_account(mac.user.as_str(), |account| {
        account.vault = None;
        account.blob = None;
        account.tombstone = Some((key.key_id(), clock.now(), forged.to_bytes()));
    });
    core.clear_received();
    let paused = mac.sync().await;
    assert_state(&paused, SyncState::ServerInconsistent);
    assert_eq!(paused.error_code.as_deref(), Some("forged_tombstone"));
    assert!(mac.row().is_some(), "the row stays");
    assert_eq!(
        mac.keychain.held(&mac.user).len(),
        1,
        "no item deleted on a server's claim"
    );
    paused_without_a_code(&paused);
    wrote_nothing(&core);
}

#[tokio::test]
async fn a_404_while_this_mac_holds_the_key_never_creates() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    core.with_account(mac.user.as_str(), |account| {
        account.vault = None;
        account.blob = None;
    });
    core.clear_received();
    let missing = mac.sync().await;
    assert_state(&missing, SyncState::MissingOnServer);
    paused_without_a_code(&missing);
    wrote_nothing(&core);
    // Not even when the person turns sync on again.
    let again = mac.turn_on("Studio Mac").await;
    assert_state(&again, SyncState::MissingOnServer);
    wrote_nothing(&core);
}

#[tokio::test]
async fn garbage_bytes_are_never_overwritten_and_prompt_nothing() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    // Garbage the server made: it cannot sign it.
    let garbage = vec![0x5a; velvt_service::vault::crypto::SEALED_SIZES[0]];
    serve_blob(
        &core,
        mac.user.as_str(),
        2,
        garbage.clone(),
        put_signature(&VaultKey::generate().unwrap(), &mac.user, 1, 2, &garbage),
    );
    core.clear_received();
    let paused = mac.sync().await;
    assert_state(&paused, SyncState::ServerInconsistent);
    paused_without_a_code(&paused);
    wrote_nothing(&core);

    // Garbage a buggy Mac signed with the vault's key: it cannot be opened.
    serve_blob(
        &core,
        mac.user.as_str(),
        3,
        garbage.clone(),
        put_signature(&key, &mac.user, 2, 3, &garbage),
    );
    core.clear_received();
    let unreadable = mac.sync().await;
    assert_state(&unreadable, SyncState::ServerCopyUnreadable);
    assert_eq!(
        unreadable.status_line,
        "Sync paused: the copy on Velvt's server can't be opened on this Mac. Nothing on this Mac changed."
    );
    paused_without_a_code(&unreadable);
    wrote_nothing(&core);
    assert_eq!(mac.keychain.held(&mac.user).len(), 1);
}

#[tokio::test]
async fn a_different_copy_at_the_version_this_mac_accepted_is_a_fork() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    // A second valid copy at version 1: a key holder's write the server kept
    // aside, shown to this Mac in place of the one it accepted.
    let current = core.with_account(mac.user.as_str(), |account| account.blob.clone().unwrap());
    let mut document = open_copy(&key, &mac.user, 1, &current.ciphertext);
    document.devices.clear();
    let other = seal_copy(&key, &mac.user, 1, &document);
    serve_blob(
        &core,
        mac.user.as_str(),
        1,
        other.clone(),
        put_signature(&key, &mac.user, 0, 1, &other),
    );
    // An honest server answers the Mac's If-None-Match with 304; this one
    // sends the other copy anyway.
    core.lie(move |received, account| {
        (received.method == "GET" && received.path == "/v1/vault/blobs/prefs").then(|| {
            blob_response(
                account.vault.as_ref().unwrap().key_id,
                account.blob.as_ref().unwrap(),
                START_MS,
            )
        })
    });
    core.clear_received();
    let forked = mac.sync().await;
    assert_state(&forked, SyncState::ServerInconsistent);
    assert_eq!(forked.error_code.as_deref(), Some("fork"));
    paused_without_a_code(&forked);
    wrote_nothing(&core);
}

/// F1: after a rotation, a fresh Mac on the same Apple ID holds the old item
/// and the new one marked `supersedes`. The server serves the pre-rotation
/// copy: the old key is never adopted.
#[tokio::test]
async fn the_pre_rotation_copy_served_to_a_fresh_mac_is_a_rollback() {
    for marker in ["supersedes", "retired_by"] {
        let (core, clock) = setup();
        let (mac, old) = syncing(&core, &clock).await;
        let new = VaultKey::generate().unwrap();
        let fresh_keychain = MemoryKeyStore::new();
        fresh_keychain.insert(
            &mac.user,
            &old,
            HeldKey {
                key_id: old.key_id(),
                supersedes: None,
                retired_by: (marker == "retired_by").then_some(RetiredBy::Key(new.key_id())),
                created_at_ms: Some(START_MS),
            },
        );
        fresh_keychain.insert(
            &mac.user,
            &new,
            HeldKey {
                key_id: new.key_id(),
                supersedes: (marker == "supersedes").then_some(old.key_id()),
                retired_by: None,
                created_at_ms: Some(START_MS),
            },
        );
        let fresh = Mac::new(&core, &clock, &mac.user, &fresh_keychain, 5).await;
        core.clear_received();
        let refused = fresh.turn_on("Mac mini").await;
        assert_state(&refused, SyncState::ServerInconsistent);
        assert_eq!(
            refused.error_code.as_deref(),
            Some("retired_key_served"),
            "{marker}"
        );
        assert_eq!(fresh.row().unwrap().key_id, None, "{marker}: never adopted");
        paused_without_a_code(&refused);
        wrote_nothing(&core);
    }
}

/// A completed reset shown in a 404 body to a Mac that polled through the
/// window and never saw it pending.
#[tokio::test]
async fn a_reset_this_mac_never_saw_while_polling_is_refused() {
    let (core, clock) = setup();
    let (mac, key) = syncing(&core, &clock).await;
    // The server hides the pending reset from every poll.
    core.request_reset(mac.user.as_str());
    let reset_id = core.with_account(mac.user.as_str(), |account| account.resets[0].id.clone());
    core.lie(|received, account| {
        (received.method == "GET" && received.path == "/v1/vault" && account.vault.is_some()).then(
            || {
                let vault = account.vault.as_ref().unwrap();
                response(
                    200,
                    json!({
                        "key_id": vault.key_id.to_hex(),
                        "write_pk_b64": vault.write_pk.to_base64(),
                        "created_at": "2025-10-15T00:00:00Z",
                        "rotations_b64": [],
                    }),
                )
            },
        )
    });
    for _ in 0..8 {
        clock.advance(10 * HOUR);
        mac.sync().await;
    }
    core.tell_the_truth();
    clock.advance(HOUR);
    core.clear_received();
    let refused = mac.sync().await;
    assert_state(&refused, SyncState::ServerInconsistent);
    assert_eq!(refused.error_code.as_deref(), Some("unseen_reset"));
    assert_ne!(mac.row().unwrap().seen_reset_request_id, Some(reset_id));
    paused_without_a_code(&refused);
    wrote_nothing(&core);
    assert_eq!(mac.keychain.held(&mac.user).len(), 1, "no item touched");
    assert_eq!(
        live_key(&mac.keychain, &mac.user).await.key_id(),
        key.key_id()
    );
}

/// F2: the same, for a Mac that logged out and back in during the window.
/// The anchor survived Log Out, so it knows it fetched inside the window.
#[tokio::test]
async fn a_reset_shown_to_a_mac_that_logged_out_during_the_window_is_refused() {
    let (core, clock) = setup();
    let (mut mac, _) = syncing(&core, &clock).await;
    core.request_reset(mac.user.as_str());
    core.lie(|received, account| {
        (received.method == "GET" && received.path == "/v1/vault" && account.vault.is_some()).then(
            || {
                let vault = account.vault.as_ref().unwrap();
                response(
                    200,
                    json!({
                        "key_id": vault.key_id.to_hex(),
                        "write_pk_b64": vault.write_pk.to_base64(),
                        "created_at": "2025-10-15T00:00:00Z",
                        "rotations_b64": [],
                    }),
                )
            },
        )
    });
    clock.advance(HOUR);
    mac.sync().await;
    mac.engine.log_out().await;
    clock.advance(RESET_DELAY_MS);
    core.tell_the_truth();
    mac.sign_back_in().await;
    core.clear_received();
    let refused = mac.turn_on("Studio Mac").await;
    assert_state(&refused, SyncState::ServerInconsistent);
    assert_eq!(refused.error_code.as_deref(), Some("unseen_reset"));
    paused_without_a_code(&refused);
    wrote_nothing(&core);
}

/// A Mac that could not reach velvt-core for the whole window is shown the
/// reset, says so, and may set sync up again (T5's stated residual).
#[tokio::test]
async fn a_reset_during_a_long_absence_is_accepted_and_says_so() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    clock.advance(HOUR);
    core.request_reset(mac.user.as_str());
    clock.advance(RESET_DELAY_MS + DAY);
    let accepted = mac.sync().await;
    assert_state(&accepted, SyncState::ResetCompleted);
    let notice = accepted
        .notices
        .iter()
        .find(|notice| notice.kind == NoticeKind::ResetCompleted)
        .unwrap();
    assert!(notice.body.contains("while this Mac couldn't reach Velvt"));
}

/// §7.1 step 7: a code-delivered key whose id is not the served vault's. The
/// typed key wins; the served key is never adopted.
#[tokio::test]
async fn a_code_delivered_key_that_differs_from_the_served_one_wins() {
    let (core, clock) = setup();
    let (_studio, _) = syncing(&core, &clock).await;
    let person = user("person");
    let work = Mac::new(&core, &clock, &person, &MemoryKeyStore::new(), 3).await;
    let typed = VaultKey::generate().unwrap();
    let typed_id = typed.key_id();
    core.clear_received();
    let refused = work.engine.adopt_delivered_key(typed).await;
    assert_state(&refused, SyncState::ServerInconsistent);
    assert_eq!(
        refused.error_code.as_deref(),
        Some("delivered_key_mismatch")
    );
    assert_eq!(
        work.row().unwrap().key_id,
        Some(typed_id.to_hex()),
        "the typed key is the one trusted"
    );
    assert!(
        work.keychain.held(&person).is_empty(),
        "nothing stored on the server's word"
    );
    wrote_nothing(&core);
}

/// F4: velvt-core restored from a backup, so the versions go backwards. The
/// older copy is merged, never taken as current, and the next write lands at
/// highest_seen_version + 1 instead of looping on 412.
#[tokio::test]
async fn versions_that_go_backwards_heal_on_the_next_write() {
    let (core, clock) = setup();
    let (mac, _) = syncing(&core, &clock).await;
    let backup = core.with_account(mac.user.as_str(), |account| account.blob.clone().unwrap());
    let this_mac =
        velvt_service::vault::doc::SyncDeviceId::parse(&mac.row().unwrap().sync_device_id).unwrap();
    for name in ["One", "Two", "Three"] {
        mac.engine
            .rename_device(
                this_mac,
                velvt_service::vault::doc::DeviceName::new(name).unwrap(),
            )
            .await;
        clock.advance(MINUTE);
        mac.sync().await;
    }
    assert_eq!(stored_version(&core, mac.user.as_str()), Some(4));
    core.with_account(mac.user.as_str(), |account| {
        account.blob = Some(backup.clone())
    });

    clock.advance(MINUTE);
    let healed = mac.sync().await;
    assert_state(&healed, SyncState::Syncing);
    assert_eq!(stored_version(&core, mac.user.as_str()), Some(5));
    let named: Vec<_> = healed
        .devices
        .iter()
        .filter_map(|row| row.name.clone())
        .collect();
    assert_eq!(named, vec!["Three".to_owned()]);
    let puts = core
        .received()
        .iter()
        .filter(|request| request.method == "PUT")
        .count();
    assert_eq!(puts, 4, "one write per change and one to heal: no 412 loop");
}

/// Withheld updates (a fork the server keeps up): accepted for preferences
/// in v1 (§1.2, Q20). Once the server stops, the Macs converge.
#[tokio::test]
async fn withheld_updates_converge_once_the_server_stops() {
    let (core, clock) = setup();
    let person = user("person");
    let icloud = MemoryKeyStore::new();
    let studio = Mac::new(&core, &clock, &person, &icloud, 1).await;
    let laptop = Mac::new(&core, &clock, &person, &icloud, 2).await;
    studio.turn_on("Studio Mac").await;
    laptop.turn_on("MacBook Pro").await;
    studio.sync().await;
    let frozen = core.with_account(person.as_str(), |account| account.blob.clone().unwrap());

    let laptop_id =
        velvt_service::vault::doc::SyncDeviceId::parse(&laptop.row().unwrap().sync_device_id)
            .unwrap();
    studio
        .engine
        .rename_device(
            laptop_id,
            velvt_service::vault::doc::DeviceName::new("Renamed").unwrap(),
        )
        .await;
    studio.sync().await;
    // The server keeps showing the laptop the copy from before the rename.
    let laptop_device = laptop.device.to_string();
    let held_back = frozen.clone();
    let now = clock.now();
    core.lie(move |received, account| {
        (received.device == laptop_device
            && received.path == "/v1/vault/blobs/prefs"
            && received.method == "GET")
            .then(|| blob_response(account.vault.as_ref().unwrap().key_id, &held_back, now))
    });
    for _ in 0..3 {
        clock.advance(10 * MINUTE);
        let withheld = laptop.sync().await;
        assert_state(&withheld, SyncState::Syncing);
        let names: Vec<_> = withheld
            .devices
            .iter()
            .filter_map(|row| row.name.clone())
            .collect();
        assert!(
            !names.contains(&"Renamed".to_owned()),
            "not detected in v1: accepted"
        );
    }
    core.tell_the_truth();
    clock.advance(MINUTE);
    let caught_up = laptop.sync().await;
    let names: Vec<_> = caught_up
        .devices
        .iter()
        .filter_map(|row| row.name.clone())
        .collect();
    assert!(names.contains(&"Renamed".to_owned()), "{names:?}");
}
