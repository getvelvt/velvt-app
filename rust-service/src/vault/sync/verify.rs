//! What a v1 Mac believes of velvt-core (design §6.1), as pure functions.
//!
//! No I/O and no clock of their own: each takes the trust anchor this Mac
//! kept, what the server answered, the Keychain inventory and `now`, and says
//! what to do. The engine does it. Every judgement here fails closed into
//! `server_inconsistent`, which pauses sync and never offers code entry: a
//! server that wants a Mac to type an attacker's code has to make it believe
//! a key change, and nothing below believes one the Mac cannot check.

use crate::vault::crypto::statement::{DeleteStatement, PutStatement, Signed, SignedRotation};
use crate::vault::crypto::{sha256, BlobName, KeyId, UserId, WritePublicKey};

use super::api::{ServedBlob, ServerReset, ServerTombstone, VaultView};
use super::keys::KeyInventory;

/// A reset of a vault without its key takes effect at least this long after
/// it was requested. A client-side constant: the server's word is not enough.
pub const RESET_DELAY_MS: i64 = 72 * 60 * 60 * 1_000;
/// The allowance for the difference between this Mac's clock and the
/// server's, in every comparison that mixes them.
pub const CLOCK_MARGIN_MS: i64 = 10 * 60 * 1_000;

/// The key this Mac trusts for the account's vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trusted {
    pub key_id: KeyId,
    pub write_pk: WritePublicKey,
}

impl Trusted {
    pub fn of(write_pk: WritePublicKey) -> Self {
        Self {
            key_id: write_pk.key_id(),
            write_pk,
        }
    }
}

/// The trust anchor, as `vault_state` keeps it across Log Out, removal and an
/// account switch (§4.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Anchor {
    pub trusted: Option<Trusted>,
    pub highest_seen_version: u64,
    pub highest_seen_sha256: Option<[u8; 32]>,
    pub fetched_at_ms: Option<i64>,
    pub seen_reset_request_id: Option<String>,
}

/// Why this Mac stopped believing the server. Each is a fixed token
/// (`vault_state.last_error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inconsistency {
    /// A key that an item this Mac holds supersedes or marks retired.
    RetiredKeyServed,
    /// The trusted key id with another write key.
    WriteKeyChanged,
    /// A new key with no verified rotation and no reset this Mac can accept.
    UnverifiedKeyChange,
    /// A completed reset this Mac was online for and never saw pending.
    UnseenReset,
    /// A deletion of the trusted vault whose signature does not verify.
    ForgedTombstone,
    /// A copy whose write signature does not verify under the trusted key.
    BadWriteSignature,
    /// A copy that names another key than the vault does.
    BlobKeyMismatch,
    /// A different copy at the version this Mac last accepted.
    Fork,
    /// A typed code delivered a key the server's vault does not have (§7.1
    /// step 7): the code wins.
    DeliveredKeyMismatch,
}

impl Inconsistency {
    pub fn code(self) -> &'static str {
        match self {
            Self::RetiredKeyServed => "retired_key_served",
            Self::WriteKeyChanged => "write_key_changed",
            Self::UnverifiedKeyChange => "unverified_key_change",
            Self::UnseenReset => "unseen_reset",
            Self::ForgedTombstone => "forged_tombstone",
            Self::BadWriteSignature => "bad_write_signature",
            Self::BlobKeyMismatch => "blob_key_mismatch",
            Self::Fork => "fork",
            Self::DeliveredKeyMismatch => "delivered_key_mismatch",
        }
    }
}

/// What a completed reset of the trusted key means to this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetJudgement {
    /// Accepted. `while_offline` when this Mac never saw it pending and made
    /// no fetch after it was requested: the notice says so (T5).
    Accepted { while_offline: bool },
    /// This Mac fetched inside the window and never saw it pending, or the
    /// window is shorter than 72 hours, or it ends in the future.
    Refused,
}

/// `check_reset` (§6.1).
pub fn check_reset(reset: &ServerReset, anchor: &Anchor, now_ms: i64) -> ResetJudgement {
    if reset.effective_at_ms.saturating_sub(reset.requested_at_ms) < RESET_DELAY_MS
        || reset.effective_at_ms > now_ms.saturating_add(CLOCK_MARGIN_MS)
    {
        return ResetJudgement::Refused;
    }
    if anchor.seen_reset_request_id.as_deref() == Some(reset.id.as_str()) {
        return ResetJudgement::Accepted {
            while_offline: false,
        };
    }
    match anchor.fetched_at_ms {
        Some(fetched_at) if fetched_at >= reset.requested_at_ms.saturating_add(CLOCK_MARGIN_MS) => {
            ResetJudgement::Refused
        }
        _ => ResetJudgement::Accepted {
            while_offline: true,
        },
    }
}

/// How `seen_reset_request_id` changes after a fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeenReset {
    Keep,
    Set(String),
    Clear,
}

/// What a `GET /v1/vault` answer means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultJudgement {
    /// The trusted key: go on to the copy.
    Current,
    /// This Mac trusts no key yet. It may adopt `key_id` only with `K` from
    /// its own Keychain or a typed code, and only if `K` gives `write_pk`.
    Untrusted {
        key_id: KeyId,
        write_pk: WritePublicKey,
    },
    /// A rotation path from the trusted key, every step signed by the key
    /// before it and the key it introduces (§5.6 step 7).
    Rotated {
        path: Vec<SignedRotation>,
        to: Trusted,
    },
    /// The trusted vault was deleted by a holder of its key (J9).
    Deleted {
        key_id: KeyId,
    },
    /// A reset of the trusted key that §6.1 accepts. `new_vault` is the vault
    /// made since, if any: adopted only through J3's routes.
    ResetCompleted {
        reset_id: String,
        while_offline: bool,
        new_vault: Option<Trusted>,
    },
    /// No vault, and nothing tells this Mac one existed.
    NoVault,
    /// No vault, but this Mac holds a live key or has seen a version: never
    /// create on its own (§6.1, I6).
    MissingOnServer,
    Inconsistent(Inconsistency),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub judgement: VaultJudgement,
    pub seen_reset: SeenReset,
}

/// §6.1 for `GET /v1/vault`, 200 or 404.
pub fn judge_vault(
    view: &VaultView,
    anchor: &Anchor,
    held: &KeyInventory,
    user: &UserId,
    now_ms: i64,
) -> Judged {
    let facts = view.facts();
    let seen_reset = match (&facts.pending_reset, &anchor.seen_reset_request_id) {
        (Some(pending), _) => SeenReset::Set(pending.id.clone()),
        // Cancelled: a later "completed" claim for it must pass the
        // fetched_at test.
        (None, Some(seen))
            if facts
                .completed_reset
                .as_ref()
                .is_none_or(|completed| &completed.id != seen) =>
        {
            SeenReset::Clear
        }
        (None, _) => SeenReset::Keep,
    };
    let judgement = judgement(view, anchor, held, user, now_ms);
    Judged {
        judgement,
        seen_reset,
    }
}

fn judgement(
    view: &VaultView,
    anchor: &Anchor,
    held: &KeyInventory,
    user: &UserId,
    now_ms: i64,
) -> VaultJudgement {
    let facts = view.facts();
    let trusted = anchor.trusted;
    let reset = match (&facts.completed_reset, trusted) {
        (Some(completed), Some(trusted)) if completed.key_id == trusted.key_id => {
            Some((completed, check_reset(completed, anchor, now_ms)))
        }
        _ => None,
    };
    let tombstone = match (&facts.tombstone, trusted) {
        (Some(tombstone), Some(trusted)) if tombstone.key_id == trusted.key_id => Some(
            verify_tombstone(tombstone, user, &trusted)
                .then_some(trusted.key_id)
                .ok_or(Inconsistency::ForgedTombstone),
        ),
        _ => None,
    };

    match view {
        VaultView::Missing { .. } => {
            if let Some(tombstone) = tombstone {
                return match tombstone {
                    Ok(key_id) => VaultJudgement::Deleted { key_id },
                    Err(inconsistency) => VaultJudgement::Inconsistent(inconsistency),
                };
            }
            match reset {
                Some((completed, ResetJudgement::Accepted { while_offline })) => {
                    VaultJudgement::ResetCompleted {
                        reset_id: completed.id.clone(),
                        while_offline,
                        new_vault: None,
                    }
                }
                Some((_, ResetJudgement::Refused)) => {
                    VaultJudgement::Inconsistent(Inconsistency::UnseenReset)
                }
                None if held.live().next().is_some() || anchor.highest_seen_version > 0 => {
                    VaultJudgement::MissingOnServer
                }
                None => VaultJudgement::NoVault,
            }
        }
        VaultView::Present {
            key_id,
            write_pk,
            rotations,
            ..
        } => {
            if held.retires(*key_id) {
                return VaultJudgement::Inconsistent(Inconsistency::RetiredKeyServed);
            }
            let Some(trusted) = trusted else {
                return VaultJudgement::Untrusted {
                    key_id: *key_id,
                    write_pk: *write_pk,
                };
            };
            if *key_id == trusted.key_id {
                return if *write_pk == trusted.write_pk {
                    VaultJudgement::Current
                } else {
                    VaultJudgement::Inconsistent(Inconsistency::WriteKeyChanged)
                };
            }
            if let Some(tombstone) = tombstone {
                return match tombstone {
                    Ok(key_id) => VaultJudgement::Deleted { key_id },
                    Err(inconsistency) => VaultJudgement::Inconsistent(inconsistency),
                };
            }
            let served = Trusted {
                key_id: *key_id,
                write_pk: *write_pk,
            };
            if let Some(path) = rotation_path(rotations, user, &trusted, &served) {
                return VaultJudgement::Rotated { path, to: served };
            }
            match reset {
                Some((completed, ResetJudgement::Accepted { while_offline })) => {
                    VaultJudgement::ResetCompleted {
                        reset_id: completed.id.clone(),
                        while_offline,
                        new_vault: Some(served),
                    }
                }
                Some((_, ResetJudgement::Refused)) => {
                    VaultJudgement::Inconsistent(Inconsistency::UnseenReset)
                }
                None => VaultJudgement::Inconsistent(Inconsistency::UnverifiedKeyChange),
            }
        }
    }
}

/// A signed deletion of the trusted vault: parsed strictly, for this
/// account, under the trusted key.
pub fn verify_tombstone(tombstone: &ServerTombstone, user: &UserId, trusted: &Trusted) -> bool {
    Signed::<DeleteStatement>::parse(&tombstone.statement)
        .ok()
        .is_some_and(|signed| {
            signed.statement.key_id == tombstone.key_id
                && signed.verify(user, &trusted.write_pk).is_ok()
        })
}

/// The chain of stored rotations from `trusted` to `served`, each step
/// verified under the key before it, or `None`. A chain never revisits a
/// key, so it is at most as long as the list; every branch is tried, so a
/// server cannot hide the real chain behind a step that leads nowhere.
pub fn rotation_path(
    rotations: &[Vec<u8>],
    user: &UserId,
    trusted: &Trusted,
    served: &Trusted,
) -> Option<Vec<SignedRotation>> {
    let parsed: Vec<SignedRotation> = rotations
        .iter()
        .filter_map(|bytes| SignedRotation::parse(bytes).ok())
        .collect();
    let mut path = Vec::new();
    let mut visited = vec![trusted.key_id];
    search(&parsed, user, *trusted, served, &mut visited, &mut path).then_some(path)
}

fn search(
    rotations: &[SignedRotation],
    user: &UserId,
    current: Trusted,
    served: &Trusted,
    visited: &mut Vec<KeyId>,
    path: &mut Vec<SignedRotation>,
) -> bool {
    if current.key_id == served.key_id {
        return current.write_pk == served.write_pk;
    }
    for step in rotations
        .iter()
        .filter(|rotation| rotation.statement.old_key_id == current.key_id)
    {
        let Ok(next) = step.verify(user, &current.write_pk) else {
            continue;
        };
        if visited.contains(&next.key_id()) {
            continue;
        }
        visited.push(next.key_id());
        path.push(step.clone());
        if search(rotations, user, Trusted::of(next), served, visited, path) {
            return true;
        }
        path.pop();
    }
    false
}

/// What a served copy means before it is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobJudgement {
    /// At or above the highest version this Mac has seen: take it as current.
    /// `same` when it is the very copy this Mac last accepted.
    Current {
        same: bool,
    },
    /// Below it: merge, never take as current (§6.1, F4).
    Older,
    Inconsistent(Inconsistency),
}

/// §6.1's checks on `GET /v1/vault/blobs/prefs`: the key, the writer's
/// signature, and the fork hash.
pub fn judge_blob(
    blob: &ServedBlob,
    user: &UserId,
    trusted: &Trusted,
    anchor: &Anchor,
) -> BlobJudgement {
    if blob.key_id != trusted.key_id {
        return BlobJudgement::Inconsistent(Inconsistency::BlobKeyMismatch);
    }
    let digest = sha256(&blob.ciphertext);
    let put = PutStatement {
        user_id: user.clone(),
        name: BlobName::prefs(),
        key_id: blob.key_id,
        schema: blob.schema,
        base_version: blob.base_version,
        new_version: blob.version,
        ciphertext_sha256: digest,
    };
    if put
        .verify(&blob.put_signature, user, &trusted.write_pk)
        .is_err()
    {
        return BlobJudgement::Inconsistent(Inconsistency::BadWriteSignature);
    }
    if blob.version == anchor.highest_seen_version {
        return match anchor.highest_seen_sha256 {
            Some(seen) if seen != digest => BlobJudgement::Inconsistent(Inconsistency::Fork),
            Some(_) => BlobJudgement::Current { same: true },
            None => BlobJudgement::Current { same: false },
        };
    }
    if blob.version < anchor.highest_seen_version {
        BlobJudgement::Older
    } else {
        BlobJudgement::Current { same: false }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_reset, judge_blob, judge_vault, rotation_path, Anchor, BlobJudgement, Inconsistency,
        ResetJudgement, SeenReset, Trusted, VaultJudgement, CLOCK_MARGIN_MS, RESET_DELAY_MS,
    };
    use crate::vault::crypto::statement::{DeleteStatement, PutStatement, Signed, SignedRotation};
    use crate::vault::crypto::{sha256, BlobName, VaultKey};
    use crate::vault::sync::api::{
        ServedBlob, ServerReset, ServerTombstone, VaultFacts, VaultView,
    };
    use crate::vault::sync::keys::{HeldKey, KeyInventory, RetiredBy};
    use crate::vault::test_support::{other_key, other_user, test_key, user, NOW_MS};

    const NOW: i64 = NOW_MS as i64;
    const HOUR: i64 = 3_600_000;
    const RESET_ID: &str = "6f9619ff-8b86-4011-b42d-00c04fc964ff";

    fn trusted(key: &VaultKey) -> Trusted {
        Trusted::of(*key.write_public_key())
    }

    fn anchor(key: &VaultKey) -> Anchor {
        Anchor {
            trusted: Some(trusted(key)),
            highest_seen_version: 5,
            highest_seen_sha256: Some([5; 32]),
            fetched_at_ms: Some(NOW - HOUR),
            seen_reset_request_id: None,
        }
    }

    fn reset(key: &VaultKey, requested_at_ms: i64) -> ServerReset {
        ServerReset {
            id: RESET_ID.into(),
            key_id: key.key_id(),
            requested_at_ms,
            effective_at_ms: requested_at_ms + RESET_DELAY_MS,
            resolved_at_ms: Some(requested_at_ms + RESET_DELAY_MS),
        }
    }

    fn present(key: &VaultKey, rotations: Vec<Vec<u8>>, facts: VaultFacts) -> VaultView {
        VaultView::Present {
            key_id: key.key_id(),
            write_pk: *key.write_public_key(),
            created_at_ms: NOW - 100 * HOUR,
            rotations,
            facts,
        }
    }

    fn tombstone(key: &VaultKey, signer: &VaultKey) -> ServerTombstone {
        let signed = Signed::sign(
            signer,
            DeleteStatement {
                user_id: user(),
                key_id: key.key_id(),
                at_version: 5,
                requested_at: NOW / 1_000,
            },
        )
        .unwrap();
        ServerTombstone {
            key_id: key.key_id(),
            deleted_at_ms: NOW,
            statement: signed.to_bytes(),
        }
    }

    fn judge(view: &VaultView, anchor: &Anchor, held: &KeyInventory) -> VaultJudgement {
        judge_vault(view, anchor, held, &user(), NOW).judgement
    }

    #[test]
    fn the_trusted_key_with_its_own_write_key_is_current() {
        let key = test_key();
        let view = present(&key, vec![], VaultFacts::default());
        assert_eq!(
            judge(&view, &anchor(&key), &KeyInventory::default()),
            VaultJudgement::Current
        );
        let mut swapped = anchor(&key);
        swapped.trusted = Some(Trusted {
            key_id: key.key_id(),
            write_pk: *other_key().write_public_key(),
        });
        assert_eq!(
            judge(&view, &swapped, &KeyInventory::default()),
            VaultJudgement::Inconsistent(Inconsistency::WriteKeyChanged)
        );
    }

    #[test]
    fn a_new_key_needs_a_rotation_signed_by_the_old_key_and_the_new() {
        let (old, new) = (test_key(), other_key());
        let rotation = SignedRotation::sign(user(), &old, &new, 6).unwrap();
        let view = present(&new, vec![rotation.to_bytes()], VaultFacts::default());
        assert!(matches!(
            judge(&view, &anchor(&old), &KeyInventory::default()),
            VaultJudgement::Rotated { ref path, to } if path.len() == 1 && to == trusted(&new)
        ));

        let unsigned = present(&new, vec![], VaultFacts::default());
        assert_eq!(
            judge(&unsigned, &anchor(&old), &KeyInventory::default()),
            VaultJudgement::Inconsistent(Inconsistency::UnverifiedKeyChange)
        );

        // Signed by the new key twice: a server that holds only the new key.
        let mut forged = rotation.clone();
        forged.old_signature = forged.new_signature;
        let view = present(&new, vec![forged.to_bytes()], VaultFacts::default());
        assert_eq!(
            judge(&view, &anchor(&old), &KeyInventory::default()),
            VaultJudgement::Inconsistent(Inconsistency::UnverifiedKeyChange)
        );
    }

    #[test]
    fn a_rotation_path_may_take_several_steps_and_never_loops() {
        let (first, second) = (test_key(), other_key());
        let third = VaultKey::from_slice(&[0x77; 32]).unwrap();
        let step_one = SignedRotation::sign(user(), &first, &second, 6).unwrap();
        let step_two = SignedRotation::sign(user(), &second, &third, 7).unwrap();
        let back = SignedRotation::sign(user(), &second, &first, 8).unwrap();
        let rotations = vec![
            back.to_bytes(),
            step_two.to_bytes(),
            b"garbage".to_vec(),
            step_one.to_bytes(),
        ];
        let path = rotation_path(&rotations, &user(), &trusted(&first), &trusted(&third)).unwrap();
        assert_eq!(path, vec![step_one, step_two.clone()]);
        assert!(rotation_path(
            &[step_two.to_bytes()],
            &user(),
            &trusted(&first),
            &trusted(&third)
        )
        .is_none());
        assert!(rotation_path(
            &rotations,
            &other_user(),
            &trusted(&first),
            &trusted(&third)
        )
        .is_none());
    }

    #[test]
    fn a_key_an_item_supersedes_or_retires_is_a_rollback() {
        let (old, new) = (test_key(), other_key());
        let view = present(&old, vec![], VaultFacts::default());
        let superseding = KeyInventory {
            keys: vec![HeldKey {
                key_id: new.key_id(),
                supersedes: Some(old.key_id()),
                retired_by: None,
                created_at_ms: None,
            }],
        };
        assert_eq!(
            judge(&view, &Anchor::default(), &superseding),
            VaultJudgement::Inconsistent(Inconsistency::RetiredKeyServed)
        );
        let retired = KeyInventory {
            keys: vec![HeldKey {
                key_id: old.key_id(),
                supersedes: None,
                retired_by: Some(RetiredBy::Key(new.key_id())),
                created_at_ms: None,
            }],
        };
        assert_eq!(
            judge(&view, &anchor(&old), &retired),
            VaultJudgement::Inconsistent(Inconsistency::RetiredKeyServed)
        );
    }

    #[test]
    fn a_reset_is_accepted_only_if_this_mac_could_not_have_seen_it_pending() {
        let key = test_key();
        let mut polling = anchor(&key);
        // Requested 73 h ago; this Mac fetched an hour ago and never saw it.
        let late = reset(&key, NOW - 73 * HOUR);
        assert_eq!(check_reset(&late, &polling, NOW), ResetJudgement::Refused);
        // Seen pending: accepted.
        polling.seen_reset_request_id = Some(RESET_ID.into());
        assert_eq!(
            check_reset(&late, &polling, NOW),
            ResetJudgement::Accepted {
                while_offline: false
            }
        );
        // Offline since before it was requested: accepted, and said so.
        let mut offline = anchor(&key);
        offline.fetched_at_ms = Some(NOW - 80 * HOUR);
        assert_eq!(
            check_reset(&late, &offline, NOW),
            ResetJudgement::Accepted {
                while_offline: true
            }
        );
        // A claim that it was requested after this Mac's last fetch ends in the
        // future, so it is refused however it is dated.
        let fresh = reset(&key, NOW - HOUR + 1);
        assert_eq!(check_reset(&fresh, &offline, NOW), ResetJudgement::Refused);
        // A window shorter than 72 hours is refused.
        let mut short = late.clone();
        short.effective_at_ms = short.requested_at_ms + RESET_DELAY_MS - 1;
        assert_eq!(check_reset(&short, &offline, NOW), ResetJudgement::Refused);
        // The margin: a fetch less than 10 minutes after the request is clock skew.
        let mut skewed = anchor(&key);
        skewed.fetched_at_ms = Some(late.requested_at_ms + CLOCK_MARGIN_MS - 1);
        assert!(matches!(
            check_reset(&late, &skewed, NOW),
            ResetJudgement::Accepted { .. }
        ));
    }

    #[test]
    fn a_404_is_never_a_reason_to_create_when_this_mac_has_seen_a_vault() {
        let key = test_key();
        let missing = VaultView::Missing {
            facts: VaultFacts::default(),
        };
        assert_eq!(
            judge(&missing, &anchor(&key), &KeyInventory::default()),
            VaultJudgement::MissingOnServer
        );
        let holding = KeyInventory {
            keys: vec![HeldKey {
                key_id: key.key_id(),
                supersedes: None,
                retired_by: None,
                created_at_ms: None,
            }],
        };
        assert_eq!(
            judge(&missing, &Anchor::default(), &holding),
            VaultJudgement::MissingOnServer
        );
        assert_eq!(
            judge(&missing, &Anchor::default(), &KeyInventory::default()),
            VaultJudgement::NoVault
        );
    }

    #[test]
    fn a_tombstone_is_believed_only_under_the_trusted_key() {
        let (key, other) = (test_key(), other_key());
        let facts = |tombstone| VaultFacts {
            tombstone: Some(tombstone),
            ..VaultFacts::default()
        };
        let missing = VaultView::Missing {
            facts: facts(tombstone(&key, &key)),
        };
        assert_eq!(
            judge(&missing, &anchor(&key), &KeyInventory::default()),
            VaultJudgement::Deleted {
                key_id: key.key_id()
            }
        );
        let forged = VaultView::Missing {
            facts: facts(tombstone(&key, &other)),
        };
        assert_eq!(
            judge(&forged, &anchor(&key), &KeyInventory::default()),
            VaultJudgement::Inconsistent(Inconsistency::ForgedTombstone)
        );
        // C2 carries the tombstone on a 200 too: the vault was deleted and
        // made again under a new key while this Mac was away.
        let remade = present(&other, vec![], facts(tombstone(&key, &key)));
        assert_eq!(
            judge(&remade, &anchor(&key), &KeyInventory::default()),
            VaultJudgement::Deleted {
                key_id: key.key_id()
            }
        );
    }

    #[test]
    fn a_completed_reset_in_a_404_body_is_checked_like_any_other() {
        let key = test_key();
        let facts = VaultFacts {
            completed_reset: Some(reset(&key, NOW - 73 * HOUR)),
            ..VaultFacts::default()
        };
        let missing = VaultView::Missing {
            facts: facts.clone(),
        };
        assert_eq!(
            judge(&missing, &anchor(&key), &KeyInventory::default()),
            VaultJudgement::Inconsistent(Inconsistency::UnseenReset)
        );
        let mut offline = anchor(&key);
        offline.fetched_at_ms = Some(NOW - 74 * HOUR);
        assert_eq!(
            judge(&missing, &offline, &KeyInventory::default()),
            VaultJudgement::ResetCompleted {
                reset_id: RESET_ID.into(),
                while_offline: true,
                new_vault: None
            }
        );
        let other = other_key();
        let remade = present(&other, vec![], facts);
        assert_eq!(
            judge(&remade, &offline, &KeyInventory::default()),
            VaultJudgement::ResetCompleted {
                reset_id: RESET_ID.into(),
                while_offline: true,
                new_vault: Some(trusted(&other))
            }
        );
    }

    #[test]
    fn a_pending_reset_is_recorded_as_seen_and_forgotten_when_cancelled() {
        let key = test_key();
        let mut pending = reset(&key, NOW - HOUR);
        pending.resolved_at_ms = None;
        let view = present(
            &key,
            vec![],
            VaultFacts {
                pending_reset: Some(pending),
                ..VaultFacts::default()
            },
        );
        let first = judge_vault(&view, &anchor(&key), &KeyInventory::default(), &user(), NOW);
        assert_eq!(first.seen_reset, SeenReset::Set(RESET_ID.into()));
        let mut seen = anchor(&key);
        seen.seen_reset_request_id = Some(RESET_ID.into());
        let again = judge_vault(&view, &seen, &KeyInventory::default(), &user(), NOW);
        assert_eq!(again.seen_reset, SeenReset::Set(RESET_ID.into()));
        let cancelled = present(&key, vec![], VaultFacts::default());
        assert_eq!(
            judge_vault(&cancelled, &seen, &KeyInventory::default(), &user(), NOW).seen_reset,
            SeenReset::Clear
        );
    }

    fn blob(key: &VaultKey, version: u64, ciphertext: Vec<u8>) -> ServedBlob {
        let put = PutStatement {
            user_id: user(),
            name: BlobName::prefs(),
            key_id: key.key_id(),
            schema: 1,
            base_version: version - 1,
            new_version: version,
            ciphertext_sha256: sha256(&ciphertext),
        };
        ServedBlob {
            version,
            base_version: version - 1,
            key_id: key.key_id(),
            schema: 1,
            put_signature: key.sign_statement(&put).unwrap(),
            ciphertext,
        }
    }

    #[test]
    fn a_copy_is_checked_for_its_signature_its_key_and_a_fork() {
        let key = test_key();
        let mut seen = anchor(&key);
        let copy = blob(&key, 5, vec![1; 64]);
        seen.highest_seen_sha256 = Some(sha256(&copy.ciphertext));
        let judge =
            |blob: &ServedBlob, anchor: &Anchor| judge_blob(blob, &user(), &trusted(&key), anchor);
        assert_eq!(judge(&copy, &seen), BlobJudgement::Current { same: true });
        assert_eq!(
            judge(&blob(&key, 6, vec![2; 64]), &seen),
            BlobJudgement::Current { same: false }
        );
        assert_eq!(
            judge(&blob(&key, 4, vec![3; 64]), &seen),
            BlobJudgement::Older
        );
        assert_eq!(
            judge(&blob(&key, 5, vec![9; 64]), &seen),
            BlobJudgement::Inconsistent(Inconsistency::Fork)
        );
        let mut relabelled = blob(&key, 6, vec![2; 64]);
        relabelled.version = 7;
        assert_eq!(
            judge(&relabelled, &seen),
            BlobJudgement::Inconsistent(Inconsistency::BadWriteSignature)
        );
        assert_eq!(
            judge(&blob(&other_key(), 6, vec![2; 64]), &seen),
            BlobJudgement::Inconsistent(Inconsistency::BlobKeyMismatch)
        );
    }
}
