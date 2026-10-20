//! Where `K` is kept: the synchronizable Keychain item Swift stores (design
//! §5.5), behind a trait.
//!
//! Swift's `SyncKeyStore` and the protocol-38 messages that reach it
//! (`vault_key_inventory_request`, `vault_key_request`, `store_vault_key`,
//! `retire_vault_key`, `delete_vault_key`) are PR A4. Until then the engine
//! has only this trait, and nothing in the shipped path implements it, so no
//! key is ever made or read. [`MemoryKeyStore`] stands in for an iCloud
//! Keychain in tests: two Macs on one Apple ID share one.
//!
//! Rust decides what the rotation markers mean (§9.2): an item that names a
//! key it `supersedes`, or that is `retired_by` another, is never adopted, and
//! a served key either marks is a rollback.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use crate::vault::crypto::{KeyId, UserId, VaultKey};

pub type KeyFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, KeyStoreError>> + Send + 'a>>;

/// Why the Keychain could not answer (§9.2 `reason`, §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyStoreError {
    #[error("no item for that key")]
    NotFound,
    #[error("the Keychain is not available")]
    KeychainUnavailable,
    /// `errSecMissingEntitlement`: the build has no Keychain Sharing profile.
    #[error("this build cannot use iCloud Keychain")]
    EntitlementMissing,
}

impl KeyStoreError {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFound => "key_not_found",
            Self::KeychainUnavailable => "keychain_unavailable",
            Self::EntitlementMissing => "entitlement_missing",
        }
    }
}

/// What retired an item: a newer key, or (Phase 4) the device chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetiredBy {
    Key(KeyId),
    V2,
}

/// One item the Keychain holds for the account, without its key bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldKey {
    pub key_id: KeyId,
    pub supersedes: Option<KeyId>,
    pub retired_by: Option<RetiredBy>,
    /// When the item was made, from its value's `created_at`. A retired item
    /// is deleted 30 days after the item that superseded it was made (§5.6).
    pub created_at_ms: Option<i64>,
}

/// Every item the Keychain holds for one account (`vault_keys_held`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyInventory {
    pub keys: Vec<HeldKey>,
}

impl KeyInventory {
    pub fn get(&self, key_id: KeyId) -> Option<&HeldKey> {
        self.keys.iter().find(|held| held.key_id == key_id)
    }

    /// Items not retired.
    pub fn live(&self) -> impl Iterator<Item = &HeldKey> {
        self.keys.iter().filter(|held| held.retired_by.is_none())
    }

    pub fn holds_live(&self, key_id: KeyId) -> bool {
        self.get(key_id)
            .is_some_and(|held| held.retired_by.is_none())
    }

    /// Whether an item this Mac holds says `key_id` is no longer the vault's
    /// key: another item supersedes it, or its own item is retired (§5.5). A
    /// server serving such a key is rolling back past a rotation.
    pub fn retires(&self, key_id: KeyId) -> bool {
        self.keys.iter().any(|held| held.supersedes == Some(key_id))
            || self
                .get(key_id)
                .is_some_and(|held| held.retired_by.is_some())
    }
}

/// The Keychain, as Rust asks it. Every method answers for one account's
/// items, `<user_id>.<key_id>`.
pub trait VaultKeyStore: Send + Sync {
    fn inventory<'a>(&'a self, user: &'a UserId) -> KeyFuture<'a, KeyInventory>;
    /// `K` itself. A secret: it reaches Rust only from a peer-verified app
    /// (A0), and only into a zeroizing [`VaultKey`].
    fn load<'a>(&'a self, user: &'a UserId, key_id: KeyId) -> KeyFuture<'a, VaultKey>;
    /// Stores `K`, synchronizable, before any vault exists under it (§5.5
    /// "Keychain first, server second"). Never overwrites an item.
    fn store<'a>(
        &'a self,
        user: &'a UserId,
        key: &'a VaultKey,
        supersedes: Option<KeyId>,
    ) -> KeyFuture<'a, ()>;
    /// Rewrites the item's value with `retired_by`, keeping its key bytes.
    fn retire<'a>(
        &'a self,
        user: &'a UserId,
        key_id: KeyId,
        retired_by: KeyId,
    ) -> KeyFuture<'a, ()>;
    /// Deletes the item, from every Mac on the Apple ID. Only for §5.5's
    /// list: Delete Account, Delete Synced Data, a verified tombstone, an
    /// orphan that lost a create race, and 30 days after a verified rotation.
    fn delete<'a>(&'a self, user: &'a UserId, key_id: KeyId) -> KeyFuture<'a, ()>;
}

#[derive(Clone)]
struct MemoryItem {
    key_b64: Zeroizing<String>,
    held: HeldKey,
}

#[derive(Default)]
struct MemoryKeychain {
    items: BTreeMap<(String, KeyId), MemoryItem>,
    unavailable: Option<KeyStoreError>,
    clock_ms: i64,
}

/// An in-memory Keychain. Clones share one store, the way two Macs signed in
/// to one Apple ID share their iCloud Keychain; a second `new()` is another
/// Apple ID.
#[derive(Clone, Default)]
pub struct MemoryKeyStore {
    inner: Arc<Mutex<MemoryKeychain>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers every request with `error` until `None` is set again.
    pub fn set_unavailable(&self, error: Option<KeyStoreError>) {
        if let Ok(mut keychain) = self.inner.lock() {
            keychain.unavailable = error;
        }
    }

    /// The `created_at` given to items stored from now on.
    pub fn set_clock_ms(&self, now_ms: i64) {
        if let Ok(mut keychain) = self.inner.lock() {
            keychain.clock_ms = now_ms;
        }
    }

    /// Puts an item in place directly, as iCloud Keychain delivering one
    /// would, or the person restoring one.
    pub fn insert(&self, user: &UserId, key: &VaultKey, held: HeldKey) {
        if let Ok(mut keychain) = self.inner.lock() {
            keychain.items.insert(
                (user.as_str().to_owned(), key.key_id()),
                MemoryItem {
                    key_b64: key.to_base64(),
                    held,
                },
            );
        }
    }

    /// Removes an item without the engine asking, as the person deleting it
    /// in Keychain Access would.
    pub fn remove(&self, user: &UserId, key_id: KeyId) {
        if let Ok(mut keychain) = self.inner.lock() {
            keychain.items.remove(&(user.as_str().to_owned(), key_id));
        }
    }

    pub fn held(&self, user: &UserId) -> Vec<HeldKey> {
        self.inner
            .lock()
            .map(|keychain| {
                keychain
                    .items
                    .iter()
                    .filter(|((owner, _), _)| owner == user.as_str())
                    .map(|(_, item)| item.held.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn with<T>(
        &self,
        operation: impl FnOnce(&mut MemoryKeychain) -> Result<T, KeyStoreError>,
    ) -> Result<T, KeyStoreError> {
        let mut keychain = self
            .inner
            .lock()
            .map_err(|_| KeyStoreError::KeychainUnavailable)?;
        if let Some(error) = keychain.unavailable {
            return Err(error);
        }
        operation(&mut keychain)
    }
}

impl VaultKeyStore for MemoryKeyStore {
    fn inventory<'a>(&'a self, user: &'a UserId) -> KeyFuture<'a, KeyInventory> {
        Box::pin(async move {
            self.with(|keychain| {
                Ok(KeyInventory {
                    keys: keychain
                        .items
                        .iter()
                        .filter(|((owner, _), _)| owner == user.as_str())
                        .map(|(_, item)| item.held.clone())
                        .collect(),
                })
            })
        })
    }

    fn load<'a>(&'a self, user: &'a UserId, key_id: KeyId) -> KeyFuture<'a, VaultKey> {
        Box::pin(async move {
            self.with(|keychain| {
                let item = keychain
                    .items
                    .get(&(user.as_str().to_owned(), key_id))
                    .ok_or(KeyStoreError::NotFound)?;
                VaultKey::from_base64(&item.key_b64).map_err(|_| KeyStoreError::NotFound)
            })
        })
    }

    fn store<'a>(
        &'a self,
        user: &'a UserId,
        key: &'a VaultKey,
        supersedes: Option<KeyId>,
    ) -> KeyFuture<'a, ()> {
        Box::pin(async move {
            self.with(|keychain| {
                let created_at_ms = Some(keychain.clock_ms);
                keychain
                    .items
                    .entry((user.as_str().to_owned(), key.key_id()))
                    .or_insert_with(|| MemoryItem {
                        key_b64: key.to_base64(),
                        held: HeldKey {
                            key_id: key.key_id(),
                            supersedes,
                            retired_by: None,
                            created_at_ms,
                        },
                    });
                Ok(())
            })
        })
    }

    fn retire<'a>(
        &'a self,
        user: &'a UserId,
        key_id: KeyId,
        retired_by: KeyId,
    ) -> KeyFuture<'a, ()> {
        Box::pin(async move {
            self.with(|keychain| {
                let item = keychain
                    .items
                    .get_mut(&(user.as_str().to_owned(), key_id))
                    .ok_or(KeyStoreError::NotFound)?;
                item.held.retired_by = Some(RetiredBy::Key(retired_by));
                Ok(())
            })
        })
    }

    fn delete<'a>(&'a self, user: &'a UserId, key_id: KeyId) -> KeyFuture<'a, ()> {
        Box::pin(async move {
            self.with(|keychain| {
                keychain.items.remove(&(user.as_str().to_owned(), key_id));
                Ok(())
            })
        })
    }
}
