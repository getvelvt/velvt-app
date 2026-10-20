//! The statements a write key signs (design §5.3).
//!
//! Each is a fixed binary layout behind its own NUL-terminated tag, so a
//! signature made for one purpose never verifies as another. velvt-core checks
//! the put, create, rotate, delete, handoff and cancel-reset signatures; Macs
//! check the put signature on every fetch and the rotate and delete statements
//! the server hands back.
//!
//! The stored form of a signed statement, which velvt-core keeps and serves
//! (`vaults.rotations`, `vault_tombstones.statement`), is the statement's bytes
//! followed by its 64-byte signatures: one, or for a rotation the old key's and
//! then the new key's. A parser rejects anything after them.
//!
//! The layouts are §5.3's table, byte for byte, which is what velvt-core
//! (`app/services/vault_formats.py`, PR C2) rebuilds and checks. Times inside
//! signed bytes are whole Unix seconds, as velvt-core signs and compares them.

use uuid::Uuid;

use super::{
    BlobName, CryptoError, FormatError, KeyId, Signature, UserId, VaultKey, WritePublicKey,
    PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH,
};
use crate::vault::code::HandoffId;
use crate::vault::wire::{Reader, Writer};

const PUT_TAG: &[u8] = b"velvt-vault-put-v1\0";
const CREATE_TAG: &[u8] = b"velvt-vault-create-v1\0";
const ROTATE_TAG: &[u8] = b"velvt-vault-rotate-v1\0";
const DELETE_TAG: &[u8] = b"velvt-vault-delete-v1\0";
const HANDOFF_TAG: &[u8] = b"velvt-vault-handoff-v1\0";
const RESET_CANCEL_TAG: &[u8] = b"velvt-vault-reset-cancel-v1\0";

mod private {
    use crate::vault::wire::Reader;

    use super::FormatError;

    pub trait Sealed: Sized {
        fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError>;
    }
}

/// A statement a write key can sign. Sealed: only the six layouts below
/// exist, and `VaultKey`'s raw signing is private to `crypto`, so nothing else
/// can be signed with `W`, inside this crate or outside it.
pub trait Statement: private::Sealed {
    fn to_bytes(&self) -> Vec<u8>;

    /// The statement's bytes, and nothing after them.
    fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut reader = Reader::new(bytes);
        let statement = Self::read(&mut reader)?;
        reader.finish()?;
        Ok(statement)
    }
}

impl VaultKey {
    /// Signs a statement with this key's write key `W`.
    pub fn sign_statement<S: Statement>(&self, statement: &S) -> Result<Signature, CryptoError> {
        self.sign(&statement.to_bytes())
    }
}

/// A statement and the one signature over it, in the stored form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed<S> {
    pub statement: S,
    pub signature: Signature,
}

impl<S: Statement> Signed<S> {
    pub fn sign(key: &VaultKey, statement: S) -> Result<Self, CryptoError> {
        let signature = key.sign_statement(&statement)?;
        Ok(Self {
            statement,
            signature,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = self.statement.to_bytes();
        bytes.extend_from_slice(self.signature.as_bytes());
        bytes
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut reader = Reader::new(bytes);
        let statement = S::read(&mut reader)?;
        let signature = read_signature(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            statement,
            signature,
        })
    }
}

/// `"velvt-vault-put-v1\0" ‖ lp(user_id) ‖ lp(name) ‖ key_id ‖ u16_be(schema)
/// ‖ u64_be(base_version) ‖ u64_be(new_version) ‖ SHA-256(ciphertext)`.
///
/// The write signature on every create and `PUT`, checked by velvt-core, and by
/// every Mac on every fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutStatement {
    pub user_id: UserId,
    pub name: BlobName,
    pub key_id: KeyId,
    pub schema: u16,
    pub base_version: u64,
    pub new_version: u64,
    pub ciphertext_sha256: [u8; 32],
}

impl PutStatement {
    /// Checks a served write against the write key this Mac trusts.
    pub fn verify(
        &self,
        signature: &Signature,
        expected_user: &UserId,
        trusted: &WritePublicKey,
    ) -> Result<(), CryptoError> {
        require_user(&self.user_id, expected_user)?;
        require_key(self.key_id, trusted)?;
        trusted.verify(&self.to_bytes(), signature)
    }
}

impl private::Sealed for PutStatement {
    fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        reader.tag(PUT_TAG)?;
        Ok(Self {
            user_id: read_user_id(reader)?,
            name: read_blob_name(reader)?,
            key_id: KeyId::from_bytes(reader.array()?),
            schema: reader.u16()?,
            base_version: reader.u64()?,
            new_version: reader.u64()?,
            ciphertext_sha256: reader.array()?,
        })
    }
}

impl Statement for PutStatement {
    fn to_bytes(&self) -> Vec<u8> {
        Writer::with_tag(PUT_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .lp(self.name.as_str().as_bytes())
            .bytes(self.key_id.as_bytes())
            .u16(self.schema)
            .u64(self.base_version)
            .u64(self.new_version)
            .bytes(&self.ciphertext_sha256)
            .finish()
    }
}

/// `"velvt-vault-create-v1\0" ‖ lp(user_id) ‖ key_id ‖ write_pk ‖
/// SHA-256(ciphertext)`, signed by the new `W` as proof of possession.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateStatement {
    pub user_id: UserId,
    pub key_id: KeyId,
    pub write_pk: WritePublicKey,
    pub ciphertext_sha256: [u8; 32],
}

impl CreateStatement {
    /// Checks that `key_id` is the one `write_pk` derives and that the holder
    /// of `write_pk` signed.
    pub fn verify(&self, signature: &Signature, expected_user: &UserId) -> Result<(), CryptoError> {
        require_user(&self.user_id, expected_user)?;
        require_key(self.key_id, &self.write_pk)?;
        self.write_pk.verify(&self.to_bytes(), signature)
    }
}

impl private::Sealed for CreateStatement {
    fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        reader.tag(CREATE_TAG)?;
        Ok(Self {
            user_id: read_user_id(reader)?,
            key_id: KeyId::from_bytes(reader.array()?),
            write_pk: read_public_key(reader)?,
            ciphertext_sha256: reader.array()?,
        })
    }
}

impl Statement for CreateStatement {
    fn to_bytes(&self) -> Vec<u8> {
        Writer::with_tag(CREATE_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .bytes(self.key_id.as_bytes())
            .bytes(self.write_pk.as_bytes())
            .bytes(&self.ciphertext_sha256)
            .finish()
    }
}

/// `"velvt-vault-rotate-v1\0" ‖ lp(user_id) ‖ old_key_id ‖ new_key_id ‖
/// new_write_pk ‖ u64_be(new_version)`, signed by the old `W` (authorization)
/// and by the new `W` (possession).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RotateStatement {
    pub user_id: UserId,
    pub old_key_id: KeyId,
    pub new_key_id: KeyId,
    pub new_write_pk: WritePublicKey,
    pub new_version: u64,
}

impl private::Sealed for RotateStatement {
    fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        reader.tag(ROTATE_TAG)?;
        Ok(Self {
            user_id: read_user_id(reader)?,
            old_key_id: KeyId::from_bytes(reader.array()?),
            new_key_id: KeyId::from_bytes(reader.array()?),
            new_write_pk: read_public_key(reader)?,
            new_version: reader.u64()?,
        })
    }
}

impl Statement for RotateStatement {
    fn to_bytes(&self) -> Vec<u8> {
        Writer::with_tag(ROTATE_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .bytes(self.old_key_id.as_bytes())
            .bytes(self.new_key_id.as_bytes())
            .bytes(self.new_write_pk.as_bytes())
            .u64(self.new_version)
            .finish()
    }
}

/// A rotation statement with both of its signatures, in the stored form
/// `statement ‖ old_signature ‖ new_signature`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedRotation {
    pub statement: RotateStatement,
    pub old_signature: Signature,
    pub new_signature: Signature,
}

impl SignedRotation {
    /// The rotation from `old` to `new` that §5.6 step 5 posts.
    pub fn sign(
        user_id: UserId,
        old: &VaultKey,
        new: &VaultKey,
        new_version: u64,
    ) -> Result<Self, CryptoError> {
        let statement = RotateStatement {
            user_id,
            old_key_id: old.key_id(),
            new_key_id: new.key_id(),
            new_write_pk: *new.write_public_key(),
            new_version,
        };
        Ok(Self {
            old_signature: old.sign_statement(&statement)?,
            new_signature: new.sign_statement(&statement)?,
            statement,
        })
    }

    /// Checks one step away from a key this Mac trusts and returns the key it
    /// introduces. The statement must name the trusted key as the old one, be
    /// signed by it, name a different new key that its own public key derives,
    /// and be signed by that new key too.
    pub fn verify(
        &self,
        expected_user: &UserId,
        trusted_old: &WritePublicKey,
    ) -> Result<WritePublicKey, CryptoError> {
        let statement = &self.statement;
        require_user(&statement.user_id, expected_user)?;
        require_key(statement.old_key_id, trusted_old)?;
        require_key(statement.new_key_id, &statement.new_write_pk)?;
        if statement.new_key_id == statement.old_key_id {
            return Err(CryptoError::KeyMismatch);
        }
        let bytes = statement.to_bytes();
        trusted_old.verify(&bytes, &self.old_signature)?;
        statement.new_write_pk.verify(&bytes, &self.new_signature)?;
        Ok(statement.new_write_pk)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = self.statement.to_bytes();
        bytes.extend_from_slice(self.old_signature.as_bytes());
        bytes.extend_from_slice(self.new_signature.as_bytes());
        bytes
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut reader = Reader::new(bytes);
        let statement = <RotateStatement as private::Sealed>::read(&mut reader)?;
        let old_signature = read_signature(&mut reader)?;
        let new_signature = read_signature(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            statement,
            old_signature,
            new_signature,
        })
    }
}

/// `"velvt-vault-delete-v1\0" ‖ lp(user_id) ‖ key_id ‖ u64_be(at_version) ‖
/// i64_be(requested_at)`, signed by `W`. `requested_at` is whole Unix seconds;
/// velvt-core refuses one more than 10 minutes from its own clock, so a
/// tombstone cannot be replayed later. It keeps the signed statement as the
/// tombstone other Macs verify (J9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteStatement {
    pub user_id: UserId,
    pub key_id: KeyId,
    pub at_version: u64,
    pub requested_at: i64,
}

impl private::Sealed for DeleteStatement {
    fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        reader.tag(DELETE_TAG)?;
        Ok(Self {
            user_id: read_user_id(reader)?,
            key_id: KeyId::from_bytes(reader.array()?),
            at_version: reader.u64()?,
            requested_at: reader.i64()?,
        })
    }
}

impl Statement for DeleteStatement {
    fn to_bytes(&self) -> Vec<u8> {
        Writer::with_tag(DELETE_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .bytes(self.key_id.as_bytes())
            .u64(self.at_version)
            .i64(self.requested_at)
            .finish()
    }
}

/// `"velvt-vault-handoff-v1\0" ‖ lp(user_id) ‖ handoff_id ‖ SHA-256(payload) ‖
/// i64_be(expires_at)`, signed by `W`, so someone with only the password cannot
/// replace the live handoff. `expires_at` is whole Unix seconds, the integer
/// the request body carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandoffStatement {
    pub user_id: UserId,
    pub handoff_id: HandoffId,
    pub payload_sha256: [u8; 32],
    pub expires_at: i64,
}

impl private::Sealed for HandoffStatement {
    fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        reader.tag(HANDOFF_TAG)?;
        Ok(Self {
            user_id: read_user_id(reader)?,
            handoff_id: HandoffId::from_bytes(reader.array()?),
            payload_sha256: reader.array()?,
            expires_at: reader.i64()?,
        })
    }
}

impl Statement for HandoffStatement {
    fn to_bytes(&self) -> Vec<u8> {
        Writer::with_tag(HANDOFF_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .bytes(self.handoff_id.as_bytes())
            .bytes(&self.payload_sha256)
            .i64(self.expires_at)
            .finish()
    }
}

/// `"velvt-vault-reset-cancel-v1\0" ‖ lp(user_id) ‖ key_id ‖
/// reset_request_id`, signed by the current `W`. The request id is velvt-core's
/// UUID as its 16 raw bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetCancelStatement {
    pub user_id: UserId,
    pub key_id: KeyId,
    pub reset_request_id: Uuid,
}

impl private::Sealed for ResetCancelStatement {
    fn read(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        reader.tag(RESET_CANCEL_TAG)?;
        Ok(Self {
            user_id: read_user_id(reader)?,
            key_id: KeyId::from_bytes(reader.array()?),
            reset_request_id: Uuid::from_bytes(reader.array()?),
        })
    }
}

impl Statement for ResetCancelStatement {
    fn to_bytes(&self) -> Vec<u8> {
        Writer::with_tag(RESET_CANCEL_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .bytes(self.key_id.as_bytes())
            .bytes(self.reset_request_id.as_bytes())
            .finish()
    }
}

/// The statements signed by the vault's current `W` alone, checked the same
/// way: the account is this one, the key is the trusted one, and the trusted
/// key signed.
pub trait SignedByWriteKey: Statement {
    fn user_id(&self) -> &UserId;
    fn key_id(&self) -> KeyId;
}

impl SignedByWriteKey for DeleteStatement {
    fn user_id(&self) -> &UserId {
        &self.user_id
    }
    fn key_id(&self) -> KeyId {
        self.key_id
    }
}

impl SignedByWriteKey for ResetCancelStatement {
    fn user_id(&self) -> &UserId {
        &self.user_id
    }
    fn key_id(&self) -> KeyId {
        self.key_id
    }
}

impl<S: SignedByWriteKey> Signed<S> {
    pub fn verify(
        &self,
        expected_user: &UserId,
        trusted: &WritePublicKey,
    ) -> Result<(), CryptoError> {
        require_user(self.statement.user_id(), expected_user)?;
        require_key(self.statement.key_id(), trusted)?;
        trusted.verify(&self.statement.to_bytes(), &self.signature)
    }
}

impl Signed<HandoffStatement> {
    /// A handoff names no key id, so the check is the account and the trusted
    /// key's signature.
    pub fn verify_handoff(
        &self,
        expected_user: &UserId,
        trusted: &WritePublicKey,
    ) -> Result<(), CryptoError> {
        require_user(&self.statement.user_id, expected_user)?;
        trusted.verify(&self.statement.to_bytes(), &self.signature)
    }
}

fn require_user(actual: &UserId, expected: &UserId) -> Result<(), CryptoError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CryptoError::UserMismatch)
    }
}

fn require_key(named: KeyId, key: &WritePublicKey) -> Result<(), CryptoError> {
    if named == key.key_id() {
        Ok(())
    } else {
        Err(CryptoError::KeyMismatch)
    }
}

fn read_user_id(reader: &mut Reader<'_>) -> Result<UserId, FormatError> {
    std::str::from_utf8(reader.lp()?)
        .ok()
        .and_then(UserId::new)
        .ok_or(FormatError::InvalidField("user_id"))
}

fn read_blob_name(reader: &mut Reader<'_>) -> Result<BlobName, FormatError> {
    std::str::from_utf8(reader.lp()?)
        .ok()
        .and_then(BlobName::new)
        .ok_or(FormatError::InvalidField("name"))
}

fn read_public_key(reader: &mut Reader<'_>) -> Result<WritePublicKey, FormatError> {
    let bytes: [u8; PUBLIC_KEY_LENGTH] = reader.array()?;
    WritePublicKey::from_slice(&bytes).map_err(|_| FormatError::InvalidField("write_pk"))
}

fn read_signature(reader: &mut Reader<'_>) -> Result<Signature, FormatError> {
    let bytes: [u8; SIGNATURE_LENGTH] = reader.array()?;
    Signature::from_slice(&bytes).map_err(|_| FormatError::InvalidField("signature"))
}
