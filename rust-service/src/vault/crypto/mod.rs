//! The vault key `K`, what is derived from it, and the sealed document format
//! (design §5.1–§5.4).
//!
//! Every primitive is `ring`'s: ChaCha20-Poly1305 with a random 96-bit nonce,
//! HKDF-SHA256, Ed25519 and HMAC-SHA256. Secret bytes this module holds sit in
//! `Zeroizing` buffers and are wiped when dropped; key schedules inside `ring`
//! are rebuilt per operation and dropped at once, which is the best effort the
//! design asks for (§16.1). `K` never serializes. The one secret with a
//! `Serialize` impl is `person_salt` (in `doc`), because the document carries
//! it, and only ever inside the sealed document.
//!
//! The raw primitives are not reachable from the rest of the crate: signing
//! with `W` is private to this module, so `W` signs only a [`statement`];
//! `K`'s bytes, HKDF and the AEAD are visible only inside `vault`; and sealing
//! with a chosen nonce exists only in test builds.

pub mod statement;

use std::fmt;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::digest::SHA256;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use ring::{hkdf, hmac};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::doc::SyncDeviceId;
pub use super::wire::FormatError;
use super::wire::Writer;

/// Bytes in `K`, in every derived symmetric key and in `person_salt`.
pub const KEY_LENGTH: usize = 32;
pub const NONCE_LENGTH: usize = 12;
pub const TAG_LENGTH: usize = 16;
pub const SIGNATURE_LENGTH: usize = 64;
pub const PUBLIC_KEY_LENGTH: usize = 32;

/// The six sizes a sealed document can have: 16, 32, 64, 128, 256 and 512 KiB,
/// nonce and tag included. Fixing the *sealed* length, rather than the padded
/// plaintext's, keeps the largest copy inside the server's 512 KiB limit and
/// shows the server nothing finer than which of the six steps a copy is in.
pub const SEALED_SIZES: [usize; 6] = [
    16 * 1024,
    32 * 1024,
    64 * 1024,
    128 * 1024,
    256 * 1024,
    512 * 1024,
];
const SEAL_OVERHEAD: usize = NONCE_LENGTH + TAG_LENGTH;
const LENGTH_PREFIX: usize = 4;
/// The longest document JSON that fits in the largest sealed size.
pub const MAX_DOCUMENT_JSON_LENGTH: usize =
    SEALED_SIZES[SEALED_SIZES.len() - 1] - SEAL_OVERHEAD - LENGTH_PREFIX;

const DOC_KEY_INFO: &[u8] = b"velvt-vault-doc-v1";
const WRITE_SEED_INFO: &[u8] = b"velvt-vault-write-v1";
const ISSUER_KEY_INFO: &[u8] = b"velvt-vault-issuer-v1";
const KEY_ID_TAG: &[u8] = b"velvt-vault-key-id-v1\0";
const FINGERPRINT_TAG: &[u8] = b"velvt-vault-fpr-v1\0";
const DOC_AAD_TAG: &[u8] = b"velvt-vault-doc-v1\0";
/// Written as the design writes it: `kind` and the digest after it are fixed
/// length, so no terminator is needed to keep two inputs apart.
const PERSON_KEY_DOMAIN: &[u8] = b"velvt:person-key:v1";

/// Why a vault cryptographic operation failed.
///
/// Opening deliberately says only that the bytes did not open: which of the
/// key, the context or the ciphertext was wrong is not knowable, and guessing
/// would invite the caller to act on a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("the system random number generator failed")]
    Random,
    #[error("a key or value is not the expected length or encoding")]
    Encoding,
    #[error("a key derivation failed")]
    Derivation,
    #[error("sealing failed")]
    Seal,
    #[error("the sealed bytes did not open under this key and context")]
    Open,
    #[error("the sealed bytes are not one of the six sealed sizes")]
    SealedLength,
    #[error("the padded plaintext is malformed")]
    Padding,
    #[error("the document is too large to seal")]
    TooLarge,
    #[error("a signature did not verify")]
    BadSignature,
    #[error("the statement or context names a different key")]
    KeyMismatch,
    #[error("the statement names a different account")]
    UserMismatch,
    #[error("the statement is not well formed: {0}")]
    Format(#[from] FormatError),
}

/// The vault key `K`: 32 random bytes made by Rust, stored by Swift in the
/// synchronizable Keychain item (§5.5), and never sent to velvt-core.
///
/// No `Clone`, no `Serialize`, a redacted `Debug`, and zeroized on drop. The
/// write public key is derived once, at construction, because it is public and
/// every caller needs it; the write signing key is rebuilt for each signature.
pub struct VaultKey {
    secret: Zeroizing<[u8; KEY_LENGTH]>,
    write_public_key: WritePublicKey,
}

impl VaultKey {
    /// A fresh `K` from the operating system's CSPRNG.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut secret = Zeroizing::new([0_u8; KEY_LENGTH]);
        SystemRandom::new()
            .fill(&mut secret[..])
            .map_err(|_| CryptoError::Random)?;
        Self::from_secret(secret)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
        let secret: [u8; KEY_LENGTH] = bytes.try_into().map_err(|_| CryptoError::Encoding)?;
        Self::from_secret(Zeroizing::new(secret))
    }

    /// Reads the `key_b64` the Keychain item holds (standard alphabet, padded).
    pub fn from_base64(encoded: &str) -> Result<Self, CryptoError> {
        let bytes = Zeroizing::new(
            BASE64
                .decode(encoded.as_bytes())
                .map_err(|_| CryptoError::Encoding)?,
        );
        Self::from_slice(&bytes)
    }

    /// The form Swift stores. A secret: hand it only to `store_vault_key`.
    pub fn to_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(BASE64.encode(&self.secret[..]))
    }

    fn from_secret(secret: Zeroizing<[u8; KEY_LENGTH]>) -> Result<Self, CryptoError> {
        let pair = write_key_pair(&secret)?;
        let write_public_key = WritePublicKey::from_slice(pair.public_key().as_ref())?;
        Ok(Self {
            secret,
            write_public_key,
        })
    }

    pub fn write_public_key(&self) -> &WritePublicKey {
        &self.write_public_key
    }

    pub fn key_id(&self) -> KeyId {
        self.write_public_key.key_id()
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.write_public_key.fingerprint()
    }

    /// Seals document JSON at the context's version (§5.3):
    /// `nonce ‖ ChaCha20-Poly1305(K_doc, nonce, aad, u32_be(len) ‖ json ‖ 0…)`,
    /// padded so the result is exactly one of [`SEALED_SIZES`].
    pub fn seal_document(
        &self,
        context: &DocumentContext<'_>,
        json: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        self.seal_document_under(context, json, random_nonce()?)
    }

    /// The deterministic half of [`Self::seal_document`], for the test
    /// vectors. Test builds only: a reused nonce under ChaCha20-Poly1305 gives
    /// up both confidentiality and integrity.
    #[cfg(test)]
    pub(in crate::vault) fn seal_document_with_nonce(
        &self,
        context: &DocumentContext<'_>,
        json: &[u8],
        nonce: [u8; NONCE_LENGTH],
    ) -> Result<Vec<u8>, CryptoError> {
        self.seal_document_under(context, json, nonce)
    }

    fn seal_document_under(
        &self,
        context: &DocumentContext<'_>,
        json: &[u8],
        nonce: [u8; NONCE_LENGTH],
    ) -> Result<Vec<u8>, CryptoError> {
        self.require_own_key(context.key_id)?;
        let plain = pad_document(json)?;
        let doc_key = hkdf_sha256::<KEY_LENGTH>(&self.secret[..], DOC_KEY_INFO)?;
        aead_seal(&doc_key, nonce, &context.aad(), &plain)
    }

    /// Opens a sealed document and returns its JSON, without the padding.
    ///
    /// Fails closed on any change to the key, to any field of the context, or
    /// to any byte of the input, and on an input that is not one of the six
    /// sealed sizes.
    pub fn open_document(
        &self,
        context: &DocumentContext<'_>,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        self.require_own_key(context.key_id)?;
        if !SEALED_SIZES.contains(&sealed.len()) {
            return Err(CryptoError::SealedLength);
        }
        let doc_key = hkdf_sha256::<KEY_LENGTH>(&self.secret[..], DOC_KEY_INFO)?;
        let plain = aead_open(&doc_key, &context.aad(), sealed)?;
        Ok(Zeroizing::new(unpad_document(&plain)?.to_vec()))
    }

    /// The tag a rotation or deletion statement carries to name the Mac that
    /// signed it (J6, J9): `HMAC-SHA256(K_issuer, sync_device_id)[0..16]`, with
    /// `K_issuer = HKDF-SHA256(K, info = "velvt-vault-issuer-v1")`. Another
    /// Mac holding this `K` finds the name by computing the tag for each
    /// device in its last opened document; velvt-core, which stores the
    /// statement, sees no device id.
    pub fn issuer_tag(&self, issuer: SyncDeviceId) -> Result<IssuerTag, CryptoError> {
        let issuer_key = hkdf_sha256::<KEY_LENGTH>(&self.secret[..], ISSUER_KEY_INFO)?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, &issuer_key[..]);
        let mac = hmac::sign(&key, issuer.as_bytes());
        let mut tag = [0_u8; IssuerTag::LENGTH];
        tag.copy_from_slice(&mac.as_ref()[..IssuerTag::LENGTH]);
        Ok(IssuerTag(tag))
    }

    /// The device among `candidates` whose tag under this key is `tag`, if
    /// any. `None` when the signer is not in the document this Mac last
    /// opened, for instance a Mac that joined since; the copy then names no
    /// Mac.
    pub fn identify_issuer<'a>(
        &self,
        tag: &IssuerTag,
        candidates: impl IntoIterator<Item = &'a SyncDeviceId>,
    ) -> Result<Option<SyncDeviceId>, CryptoError> {
        for candidate in candidates {
            if self.issuer_tag(*candidate)? == *tag {
                return Ok(Some(*candidate));
            }
        }
        Ok(None)
    }

    /// Private to this module: `statement` signs through
    /// [`VaultKey::sign_statement`], so nothing else in the crate can make `W`
    /// sign arbitrary bytes.
    fn sign(&self, message: &[u8]) -> Result<Signature, CryptoError> {
        let pair = write_key_pair(&self.secret)?;
        Signature::from_slice(pair.sign(message).as_ref())
    }

    /// `K`'s bytes, for the handoff payload (`vault::code`) and the vectors.
    /// [`VaultKey::to_base64`] is the only way they leave `vault`.
    pub(in crate::vault) fn secret_bytes(&self) -> &[u8; KEY_LENGTH] {
        &self.secret
    }

    fn require_own_key(&self, key_id: KeyId) -> Result<(), CryptoError> {
        if key_id == self.key_id() {
            Ok(())
        } else {
            Err(CryptoError::KeyMismatch)
        }
    }
}

impl PartialEq for VaultKey {
    /// Constant time: two keys are compared when a typed code delivers one the
    /// Keychain may already hold (§7.1 step 7).
    fn eq(&self, other: &Self) -> bool {
        self.secret[..].ct_eq(&other.secret[..]).into()
    }
}

impl Eq for VaultKey {}

impl fmt::Debug for VaultKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultKey")
            .field("key_id", &self.key_id())
            .field("secret", &"[redacted]")
            .finish()
    }
}

/// `key_id = SHA-256("velvt-vault-key-id-v1\0" ‖ write_pk)[0..8]`: 8 raw
/// bytes inside signed and authenticated structures, 16 lowercase hex
/// characters in JSON and in the database.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId([u8; 8]);

impl KeyId {
    pub const LENGTH: usize = 8;

    pub fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }

    /// Exactly 16 lowercase hexadecimal characters.
    pub fn from_hex(encoded: &str) -> Option<Self> {
        decode_hex::<{ Self::LENGTH }>(encoded).map(Self)
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "KeyId({})", self.to_hex())
    }
}

/// The Ed25519 public key of the write key `W` (§5.2), which velvt-core stores
/// as `write_pk` and checks every write against.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WritePublicKey([u8; PUBLIC_KEY_LENGTH]);

impl WritePublicKey {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| CryptoError::Encoding)
    }

    pub fn from_base64(encoded: &str) -> Result<Self, CryptoError> {
        let bytes = BASE64
            .decode(encoded.as_bytes())
            .map_err(|_| CryptoError::Encoding)?;
        Self::from_slice(&bytes)
    }

    pub fn to_base64(&self) -> String {
        BASE64.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; PUBLIC_KEY_LENGTH] {
        &self.0
    }

    pub fn key_id(&self) -> KeyId {
        let hash = sha256_parts(&[KEY_ID_TAG, &self.0]);
        let mut id = [0_u8; KeyId::LENGTH];
        id.copy_from_slice(&hash[..KeyId::LENGTH]);
        KeyId(id)
    }

    pub fn fingerprint(&self) -> Fingerprint {
        let hash = sha256_parts(&[FINGERPRINT_TAG, &self.0]);
        let mut first = [0_u8; 8];
        first.copy_from_slice(&hash[..8]);
        Fingerprint(u64::from_be_bytes(first) % Fingerprint::MODULUS)
    }

    fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
        UnparsedPublicKey::new(&ED25519, &self.0)
            .verify(message, &signature.0)
            .map_err(|_| CryptoError::BadSignature)
    }
}

impl fmt::Debug for WritePublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WritePublicKey({})", encode_hex(&self.0))
    }
}

/// The sync-key fingerprint the device list shows (J4): twelve decimal digits
/// of `SHA-256("velvt-vault-fpr-v1\0" ‖ write_pk)`, read as the first eight
/// bytes big-endian modulo 10^12. Every Mac holding the same key shows the same
/// digits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fingerprint(u64);

impl Fingerprint {
    const MODULUS: u64 = 1_000_000_000_000;

    /// The twelve digits with no separator.
    pub fn digits(&self) -> String {
        format!("{:012}", self.0)
    }
}

impl fmt::Display for Fingerprint {
    /// Grouped as the device list shows it: `4821 9133 0577`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.digits();
        write!(
            formatter,
            "{} {} {}",
            &digits[0..4],
            &digits[4..8],
            &digits[8..12]
        )
    }
}

/// An Ed25519 signature by a write key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature([u8; SIGNATURE_LENGTH]);

impl Signature {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| CryptoError::Encoding)
    }

    pub fn from_base64(encoded: &str) -> Result<Self, CryptoError> {
        let bytes = BASE64
            .decode(encoded.as_bytes())
            .map_err(|_| CryptoError::Encoding)?;
        Self::from_slice(&bytes)
    }

    pub fn to_base64(&self) -> String {
        BASE64.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; SIGNATURE_LENGTH] {
        &self.0
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Signature({})", encode_hex(&self.0))
    }
}

/// The 16 bytes a rotation or deletion statement carries in place of the
/// signing Mac's `sync_device_id`; see [`VaultKey::issuer_tag`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct IssuerTag([u8; IssuerTag::LENGTH]);

impl IssuerTag {
    pub const LENGTH: usize = 16;

    pub fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

impl fmt::Debug for IssuerTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "IssuerTag({})", encode_hex(&self.0))
    }
}

/// velvt-core's id for the account, as it appears in every AAD and statement.
///
/// 1–64 bytes of printable ASCII. velvt-core's ids are UUID strings; the bound
/// keeps `lp(user_id)` small and leaves no room for a byte that could read
/// differently on the server.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct UserId(String);

impl UserId {
    pub const MAX_LENGTH: usize = 64;

    pub fn new(value: &str) -> Option<Self> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_LENGTH
            && value.bytes().all(|byte| byte.is_ascii_graphic());
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UserId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "UserId({})", self.0)
    }
}

/// The name of a blob in the account's vault: `prefs` in v1, a namespace for
/// the return ledger later (§4.2). 1–32 bytes of `[a-z0-9_]`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BlobName(String);

impl BlobName {
    pub const MAX_LENGTH: usize = 32;
    pub const PREFS: &'static str = "prefs";

    pub fn prefs() -> Self {
        Self(Self::PREFS.to_owned())
    }

    pub fn new(value: &str) -> Option<Self> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_LENGTH
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Everything a sealed document is bound to besides its key (§5.3):
/// `aad = "velvt-vault-doc-v1\0" ‖ lp(user_id) ‖ lp(name) ‖ key_id ‖
/// u16_be(schema) ‖ u64_be(version)`.
#[derive(Clone, Copy, Debug)]
pub struct DocumentContext<'a> {
    pub user_id: &'a UserId,
    pub name: &'a BlobName,
    pub key_id: KeyId,
    pub schema: u16,
    pub version: u64,
}

impl DocumentContext<'_> {
    pub fn aad(&self) -> Vec<u8> {
        Writer::with_tag(DOC_AAD_TAG)
            .lp(self.user_id.as_str().as_bytes())
            .lp(self.name.as_str().as_bytes())
            .bytes(self.key_id.as_bytes())
            .u16(self.schema)
            .u64(self.version)
            .finish()
    }
}

/// The sealed size a document of `json_length` bytes gets, or `None` past the
/// largest.
pub fn sealed_size_for(json_length: usize) -> Option<usize> {
    let needed = json_length.checked_add(LENGTH_PREFIX + SEAL_OVERHEAD)?;
    SEALED_SIZES.iter().copied().find(|&size| needed <= size)
}

/// `u32_be(len(json)) ‖ json ‖ 0x00…`, long enough that sealing it gives a
/// sealed size.
pub fn pad_document(json: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let sealed = sealed_size_for(json.len()).ok_or(CryptoError::TooLarge)?;
    let length = u32::try_from(json.len()).map_err(|_| CryptoError::TooLarge)?;
    let mut plain = Zeroizing::new(vec![0_u8; sealed - SEAL_OVERHEAD]);
    plain[..LENGTH_PREFIX].copy_from_slice(&length.to_be_bytes());
    plain[LENGTH_PREFIX..LENGTH_PREFIX + json.len()].copy_from_slice(json);
    Ok(plain)
}

/// The inverse of [`pad_document`]: the JSON, provided the length prefix fits
/// and every padding byte is zero.
pub fn unpad_document(plain: &[u8]) -> Result<&[u8], CryptoError> {
    let prefix: [u8; LENGTH_PREFIX] = plain
        .get(..LENGTH_PREFIX)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(CryptoError::Padding)?;
    let length = usize::try_from(u32::from_be_bytes(prefix)).map_err(|_| CryptoError::Padding)?;
    let end = LENGTH_PREFIX
        .checked_add(length)
        .filter(|&end| end <= plain.len())
        .ok_or(CryptoError::Padding)?;
    if plain[end..].iter().any(|&byte| byte != 0) {
        return Err(CryptoError::Padding);
    }
    Ok(&plain[LENGTH_PREFIX..end])
}

/// `person_salt`: 32 random bytes written into the document once, when the
/// vault is created, and never changed (§5.4). Not derived from `K`, so a key
/// rotation does not re-key every rule.
///
/// It exists on disk only inside the ciphertext, so it is treated as a secret
/// here: zeroized on drop, compared in constant time, redacted in `Debug`.
#[derive(Clone)]
pub struct PersonSalt(Zeroizing<[u8; KEY_LENGTH]>);

impl PersonSalt {
    pub fn generate() -> Result<Self, CryptoError> {
        let mut salt = Zeroizing::new([0_u8; KEY_LENGTH]);
        SystemRandom::new()
            .fill(&mut salt[..])
            .map_err(|_| CryptoError::Random)?;
        Ok(Self(salt))
    }

    pub fn from_bytes(bytes: [u8; KEY_LENGTH]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn from_base64(encoded: &str) -> Result<Self, CryptoError> {
        let bytes = Zeroizing::new(
            BASE64
                .decode(encoded.as_bytes())
                .map_err(|_| CryptoError::Encoding)?,
        );
        let salt: [u8; KEY_LENGTH] = bytes[..].try_into().map_err(|_| CryptoError::Encoding)?;
        Ok(Self(Zeroizing::new(salt)))
    }

    pub fn to_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(BASE64.encode(&self.0[..]))
    }
}

impl PartialEq for PersonSalt {
    fn eq(&self, other: &Self) -> bool {
        self.0[..].ct_eq(&other.0[..]).into()
    }
}

impl Eq for PersonSalt {}

impl fmt::Debug for PersonSalt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PersonSalt([redacted])")
    }
}

/// What a person key identifies, as its one-byte code in the MAC input (§5.3)
/// and its prefix in a rule key (§4.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PersonKeyKind {
    Site,
    Bundle,
    App,
}

impl PersonKeyKind {
    pub const ALL: [Self; 3] = [Self::Site, Self::Bundle, Self::App];

    pub fn code(self) -> u8 {
        match self {
            Self::Site => 1,
            Self::Bundle => 2,
            Self::App => 3,
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Bundle => "bundle",
            Self::App => "app",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.prefix() == prefix)
    }
}

/// `HMAC-SHA256(person_salt, "velvt:person-key:v1" ‖ kind ‖ digest)`: the same
/// value on every Mac of one person for one site, bundle or app name, and
/// meaningless without the salt inside the document (§5.4).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PersonKey([u8; 32]);

impl PersonKey {
    /// `digest` is the unsalted site, bundle or app digest `abstraction::key`
    /// computes. This function never sees the raw value.
    pub fn derive(salt: &PersonSalt, kind: PersonKeyKind, digest: &[u8; 32]) -> Self {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &salt.0[..]);
        let mut context = hmac::Context::with_key(&key);
        context.update(PERSON_KEY_DOMAIN);
        context.update(&[kind.code()]);
        context.update(digest);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(context.sign().as_ref());
        Self(bytes)
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }

    /// Exactly 64 lowercase hexadecimal characters.
    pub fn from_hex(encoded: &str) -> Option<Self> {
        decode_hex::<32>(encoded).map(Self)
    }
}

impl fmt::Debug for PersonKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PersonKey({})", self.to_hex())
    }
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    sha256_parts(&[bytes])
}

fn sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut context = ring::digest::Context::new(&SHA256);
    for part in parts {
        context.update(part);
    }
    let mut hash = [0_u8; 32];
    hash.copy_from_slice(context.finish().as_ref());
    hash
}

/// HKDF-SHA256 (RFC 5869) with an empty salt, which RFC 5869 §2.2 defines as
/// `HashLen` zero bytes.
pub(in crate::vault) fn hkdf_sha256<const N: usize>(
    ikm: &[u8],
    info: &[u8],
) -> Result<Zeroizing<[u8; N]>, CryptoError> {
    struct Length(usize);
    impl hkdf::KeyType for Length {
        fn len(&self) -> usize {
            self.0
        }
    }

    let info = [info];
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(ikm);
    let okm = prk
        .expand(&info, Length(N))
        .map_err(|_| CryptoError::Derivation)?;
    let mut out = Zeroizing::new([0_u8; N]);
    okm.fill(&mut out[..])
        .map_err(|_| CryptoError::Derivation)?;
    Ok(out)
}

fn write_key_pair(secret: &[u8; KEY_LENGTH]) -> Result<Ed25519KeyPair, CryptoError> {
    let seed = hkdf_sha256::<KEY_LENGTH>(secret, WRITE_SEED_INFO)?;
    Ed25519KeyPair::from_seed_unchecked(&seed[..]).map_err(|_| CryptoError::Derivation)
}

/// A fresh nonce from the operating system's CSPRNG.
pub(in crate::vault) fn random_nonce() -> Result<[u8; NONCE_LENGTH], CryptoError> {
    let mut nonce = [0_u8; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| CryptoError::Random)?;
    Ok(nonce)
}

/// `nonce ‖ ChaCha20-Poly1305(key, nonce, aad, plain)`. Every production
/// caller passes [`random_nonce`]; only the vectors fix one.
pub(in crate::vault) fn aead_seal(
    key: &[u8; KEY_LENGTH],
    nonce: [u8; NONCE_LENGTH],
    aad: &[u8],
    plain: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let key = aead_key(key)?;
    let mut in_out = Zeroizing::new(plain.to_vec());
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad),
        &mut *in_out,
    )
    .map_err(|_| CryptoError::Seal)?;
    let mut sealed = Vec::with_capacity(NONCE_LENGTH + in_out.len());
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&in_out);
    Ok(sealed)
}

/// The inverse of [`aead_seal`]; the plaintext comes back in a buffer that is
/// wiped when dropped.
pub(in crate::vault) fn aead_open(
    key: &[u8; KEY_LENGTH],
    aad: &[u8],
    sealed: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if sealed.len() < SEAL_OVERHEAD {
        return Err(CryptoError::Open);
    }
    let (nonce, body) = sealed.split_at(NONCE_LENGTH);
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| CryptoError::Open)?;
    let key = aead_key(key)?;
    let mut in_out = Zeroizing::new(body.to_vec());
    let plain_length = key
        .open_in_place(nonce, Aad::from(aad), &mut in_out[..])
        .map_err(|_| CryptoError::Open)?
        .len();
    in_out.truncate(plain_length);
    Ok(in_out)
}

fn aead_key(key: &[u8; KEY_LENGTH]) -> Result<LessSafeKey, CryptoError> {
    UnboundKey::new(&CHACHA20_POLY1305, key)
        .map(LessSafeKey::new)
        .map_err(|_| CryptoError::Derivation)
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Lowercase only, exactly `2 * N` characters: the one spelling
/// [`encode_hex`] produces.
pub(crate) fn decode_hex<const N: usize>(encoded: &str) -> Option<[u8; N]> {
    fn nibble(character: u8) -> Option<u8> {
        match character {
            b'0'..=b'9' => Some(character - b'0'),
            b'a'..=b'f' => Some(character - b'a' + 10),
            _ => None,
        }
    }
    if encoded.len() != 2 * N {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (byte, pair) in bytes.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    use super::statement::{
        CreateStatement, DeleteStatement, HandoffStatement, PutStatement, ResetCancelStatement,
        RotateStatement, Signed, SignedRotation, Statement,
    };
    use super::{
        aead_open, hkdf_sha256, pad_document, sealed_size_for, sha256, unpad_document, BlobName,
        CryptoError, DocumentContext, FormatError, KeyId, PersonKey, PersonKeyKind, PersonSalt,
        Signature, UserId, VaultKey, WritePublicKey, DOC_KEY_INFO, KEY_LENGTH,
        MAX_DOCUMENT_JSON_LENGTH, SEALED_SIZES,
    };
    use crate::vault::code::HandoffId;
    use crate::vault::test_support::{device, other_key, other_user, prefs, test_key, user, Rng};

    const JSON: &[u8] = br#"{"schema":1}"#;

    fn context<'a>(user_id: &'a UserId, name: &'a BlobName, key: &VaultKey) -> DocumentContext<'a> {
        DocumentContext {
            user_id,
            name,
            key_id: key.key_id(),
            schema: 1,
            version: 7,
        }
    }

    #[test]
    fn a_sealed_document_opens_under_its_own_key_and_context() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);

        let sealed = key.seal_document(&context, JSON).unwrap();

        assert_eq!(sealed.len(), SEALED_SIZES[0]);
        assert_eq!(&key.open_document(&context, &sealed).unwrap()[..], JSON);
    }

    #[test]
    fn changing_any_context_field_fails_to_open() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);
        let sealed = key.seal_document(&context, JSON).unwrap();
        let another_user = other_user();
        let ledger = BlobName::new("ledger").unwrap();

        let tampered = [
            DocumentContext {
                user_id: &another_user,
                ..context
            },
            DocumentContext {
                name: &ledger,
                ..context
            },
            DocumentContext {
                schema: 2,
                ..context
            },
            DocumentContext {
                version: 8,
                ..context
            },
        ];
        for tampered in tampered {
            assert_eq!(
                key.open_document(&tampered, &sealed),
                Err(CryptoError::Open),
                "{tampered:?}"
            );
        }
        let other_key_id = DocumentContext {
            key_id: other_key().key_id(),
            ..context
        };
        assert_eq!(
            key.open_document(&other_key_id, &sealed),
            Err(CryptoError::KeyMismatch)
        );
    }

    /// The key id is refused by name before decryption is tried. This shows the
    /// AAD binds it too: the same document key, with only the key id's bytes in
    /// the AAD changed, does not open.
    #[test]
    fn the_aad_binds_the_key_id_and_every_other_field() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);
        let sealed = key.seal_document(&context, JSON).unwrap();
        let doc_key = hkdf_sha256::<KEY_LENGTH>(key.secret_bytes(), DOC_KEY_INFO).unwrap();
        let aad = context.aad();
        assert!(aead_open(&doc_key, &aad, &sealed).is_ok());

        for index in 0..aad.len() {
            let mut changed = aad.clone();
            changed[index] ^= 0x01;
            assert_eq!(
                aead_open(&doc_key, &changed, &sealed).err(),
                Some(CryptoError::Open),
                "AAD byte {index}"
            );
        }
    }

    #[test]
    fn a_wrong_key_fails_to_open() {
        let (key, wrong) = (test_key(), other_key());
        let (user_id, name) = (user(), prefs());
        let sealed = key
            .seal_document(&context(&user_id, &name, &key), JSON)
            .unwrap();
        let wrong_context = DocumentContext {
            key_id: wrong.key_id(),
            ..context(&user_id, &name, &key)
        };

        assert_eq!(
            wrong.open_document(&wrong_context, &sealed),
            Err(CryptoError::Open)
        );
    }

    #[test]
    fn changing_any_sealed_byte_fails_to_open() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);
        let sealed = key.seal_document(&context, JSON).unwrap();

        let ends = (0..64).chain(sealed.len() - 64..sealed.len());
        for index in ends.chain((64..sealed.len() - 64).step_by(97)) {
            let mut changed = sealed.clone();
            changed[index] ^= 0x80;
            assert_eq!(
                key.open_document(&context, &changed),
                Err(CryptoError::Open),
                "sealed byte {index}"
            );
        }
    }

    #[test]
    fn a_nonce_is_never_repeated_in_ten_thousand_seals() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);
        let mut nonces = HashSet::new();

        for _ in 0..10_000 {
            let sealed = key.seal_document(&context, JSON).unwrap();
            nonces.insert(sealed[..12].to_vec());
        }

        assert_eq!(nonces.len(), 10_000);
    }

    #[test]
    fn every_sealed_document_is_exactly_one_of_six_sizes() {
        assert_eq!(sealed_size_for(0), Some(16 * 1024));
        for (index, &size) in SEALED_SIZES.iter().enumerate() {
            let largest = size - 32;
            assert_eq!(sealed_size_for(largest), Some(size));
            let next = SEALED_SIZES.get(index + 1).copied();
            assert_eq!(sealed_size_for(largest + 1), next);
        }
        assert_eq!(sealed_size_for(MAX_DOCUMENT_JSON_LENGTH), Some(512 * 1024));
        assert_eq!(sealed_size_for(MAX_DOCUMENT_JSON_LENGTH + 1), None);

        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);
        for json_length in [0, 16 * 1024 - 32, 16 * 1024 - 31, MAX_DOCUMENT_JSON_LENGTH] {
            let json = vec![b' '; json_length];
            let sealed = key.seal_document(&context, &json).unwrap();
            assert_eq!(Some(sealed.len()), sealed_size_for(json_length));
            assert_eq!(
                key.open_document(&context, &sealed).unwrap().len(),
                json_length
            );
        }
        assert_eq!(
            key.seal_document(&context, &vec![b' '; MAX_DOCUMENT_JSON_LENGTH + 1]),
            Err(CryptoError::TooLarge)
        );
    }

    #[test]
    fn a_sealed_input_of_any_other_size_is_refused_before_decryption() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = context(&user_id, &name, &key);
        let mut sealed = key.seal_document(&context, JSON).unwrap();

        sealed.push(0);
        assert_eq!(
            key.open_document(&context, &sealed),
            Err(CryptoError::SealedLength)
        );
        assert_eq!(
            key.open_document(&context, &[]),
            Err(CryptoError::SealedLength)
        );
    }

    #[test]
    fn padding_is_a_length_prefix_and_zeros_and_nothing_else_is_accepted() {
        let plain = pad_document(JSON).unwrap();
        assert_eq!(plain.len(), 16 * 1024 - 28);
        assert_eq!(&plain[..4], &(JSON.len() as u32).to_be_bytes());
        assert_eq!(unpad_document(&plain).unwrap(), JSON);

        let mut nonzero = plain.clone();
        *nonzero.last_mut().unwrap() = 1;
        assert_eq!(unpad_document(&nonzero), Err(CryptoError::Padding));

        let mut overlong = plain.clone();
        overlong[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(unpad_document(&overlong), Err(CryptoError::Padding));

        assert_eq!(unpad_document(&[0, 0, 0]), Err(CryptoError::Padding));
        assert_eq!(unpad_document(&[0, 0, 0, 0]).unwrap(), b"");
    }

    /// Recomputed with the `sha2` crate rather than `ring`, so the check does not
    /// share an implementation with the code it checks.
    #[test]
    fn the_key_id_and_fingerprint_are_fixed_functions_of_the_write_public_key() {
        let key = test_key();
        let write_pk = key.write_public_key();

        let id_hash = Sha256::new()
            .chain_update(b"velvt-vault-key-id-v1\0")
            .chain_update(write_pk.as_bytes())
            .finalize();
        assert_eq!(key.key_id().as_bytes()[..], id_hash[..8]);
        assert_eq!(key.key_id().to_hex().len(), 16);

        let fpr_hash = Sha256::new()
            .chain_update(b"velvt-vault-fpr-v1\0")
            .chain_update(write_pk.as_bytes())
            .finalize();
        let expected = u64::from_be_bytes(fpr_hash[..8].try_into().unwrap()) % 1_000_000_000_000;
        let fingerprint = key.fingerprint();
        assert_eq!(fingerprint.digits(), format!("{expected:012}"));
        let shown = fingerprint.to_string();
        assert_eq!(shown.len(), 14);
        assert_eq!(shown.replace(' ', ""), fingerprint.digits());

        let same = VaultKey::from_base64(&key.to_base64()).unwrap();
        assert_eq!(same.key_id(), key.key_id());
        assert_eq!(same.fingerprint(), key.fingerprint());
        assert_ne!(other_key().key_id(), key.key_id());
    }

    #[test]
    fn hex_ids_have_one_spelling() {
        let id = test_key().key_id();
        assert_eq!(KeyId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(KeyId::from_hex(&id.to_hex().to_uppercase()), None);
        assert_eq!(KeyId::from_hex(&id.to_hex()[1..]), None);
        let handoff = HandoffId::from_bytes([0xab; 16]);
        assert_eq!(HandoffId::from_hex(&handoff.to_hex()), Some(handoff));
        assert_eq!(handoff.to_hex().len(), 32);
    }

    #[test]
    fn a_vault_key_reads_only_its_own_encoding() {
        let key = test_key();
        let encoded = key.to_base64();
        assert_eq!(VaultKey::from_base64(&encoded).unwrap(), key);
        assert_ne!(VaultKey::from_base64(&encoded).unwrap(), other_key());

        let short = &encoded[..encoded.len() - 4];
        for bad in [short, "", "not base64!", &encoded.replace('=', "")] {
            assert_eq!(
                VaultKey::from_base64(bad).err(),
                Some(CryptoError::Encoding),
                "{bad}"
            );
        }
        assert_eq!(
            VaultKey::from_slice(&[0; 31]).err(),
            Some(CryptoError::Encoding)
        );
        assert_ne!(VaultKey::generate().unwrap(), VaultKey::generate().unwrap());
    }

    #[test]
    fn debug_output_never_shows_a_secret() {
        let key = test_key();
        let shown = format!("{key:?}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(!shown.contains(key.to_base64().as_str()), "{shown}");
        assert!(
            !shown.contains(&super::encode_hex(key.secret_bytes())),
            "{shown}"
        );

        let salt = PersonSalt::from_bytes([0x5a; 32]);
        let shown = format!("{salt:?}");
        assert!(!shown.contains(salt.to_base64().as_str()), "{shown}");
        assert!(!shown.contains("5a5a"), "{shown}");
    }

    /// Best effort, as the design says: the bytes `K` occupied read as zero once
    /// it has been dropped in place.
    #[test]
    #[allow(unsafe_code)]
    fn the_vault_key_is_wiped_when_dropped() {
        let mut key = std::mem::ManuallyDrop::new(test_key());
        let secret: *const [u8; KEY_LENGTH] = key.secret_bytes();
        // SAFETY: `ManuallyDrop` keeps the storage alive after the value is
        // dropped, and the bytes of a `[u8; 32]` are always valid to read.
        let after = unsafe {
            std::mem::ManuallyDrop::drop(&mut key);
            std::ptr::read_volatile(secret)
        };
        assert_eq!(after, [0_u8; KEY_LENGTH]);
    }

    #[test]
    fn person_keys_depend_on_the_salt_the_kind_and_the_digest() {
        let salt = PersonSalt::from_bytes([1; 32]);
        let digest = sha256(b"example.com");
        let key = PersonKey::derive(&salt, PersonKeyKind::Site, &digest);

        assert_eq!(key, PersonKey::derive(&salt, PersonKeyKind::Site, &digest));
        assert_ne!(key, PersonKey::derive(&salt, PersonKeyKind::App, &digest));
        assert_ne!(
            key,
            PersonKey::derive(
                &PersonSalt::from_bytes([2; 32]),
                PersonKeyKind::Site,
                &digest
            )
        );
        assert_ne!(
            key,
            PersonKey::derive(&salt, PersonKeyKind::Site, &sha256(b"example.org"))
        );
        assert_eq!(key.to_hex().len(), 64);
        assert_eq!(PersonKey::from_hex(&key.to_hex()), Some(key));

        let codes: Vec<u8> = PersonKeyKind::ALL.iter().map(|kind| kind.code()).collect();
        assert_eq!(codes, [1, 2, 3]);
        for kind in PersonKeyKind::ALL {
            assert_eq!(PersonKeyKind::from_prefix(kind.prefix()), Some(kind));
        }
    }

    #[test]
    fn ids_on_the_wire_are_bounded_and_printable() {
        assert!(UserId::new(&"u".repeat(64)).is_some());
        for bad in ["", "has space", "tab\t", "é"] {
            assert!(UserId::new(bad).is_none(), "{bad:?}");
        }
        assert!(UserId::new(&"u".repeat(65)).is_none());
        assert!(BlobName::new("prefs").is_some());
        assert!(BlobName::new("return_ledger_2").is_some());
        for bad in ["", "Prefs", "pre-fs", &"p".repeat(33)] {
            assert!(BlobName::new(bad).is_none(), "{bad:?}");
        }
    }

    // --- Statements -------------------------------------------------------------

    struct Statements {
        put: PutStatement,
        create: CreateStatement,
        rotate: SignedRotation,
        delete: Signed<DeleteStatement>,
        handoff: Signed<HandoffStatement>,
        cancel: Signed<ResetCancelStatement>,
    }

    fn statements(key: &VaultKey, next: &VaultKey) -> Statements {
        let ciphertext_sha256 = sha256(b"ciphertext");
        Statements {
            put: PutStatement {
                user_id: user(),
                name: prefs(),
                key_id: key.key_id(),
                schema: 1,
                base_version: 41,
                new_version: 42,
                ciphertext_sha256,
            },
            create: CreateStatement {
                user_id: user(),
                key_id: key.key_id(),
                write_pk: *key.write_public_key(),
                ciphertext_sha256,
            },
            rotate: SignedRotation::sign(user(), key, next, 43, device(5)).unwrap(),
            delete: Signed::sign(
                key,
                DeleteStatement {
                    user_id: user(),
                    key_id: key.key_id(),
                    at_version: 42,
                    requested_at_ms: 1_760_486_400_000,
                    issuer_tag: key.issuer_tag(device(5)).unwrap(),
                },
            )
            .unwrap(),
            handoff: Signed::sign(
                key,
                HandoffStatement {
                    user_id: user(),
                    handoff_id: HandoffId::from_bytes([7; 16]),
                    payload_sha256: sha256(b"payload"),
                    expires_at_ms: 1_760_487_000_000,
                },
            )
            .unwrap(),
            cancel: Signed::sign(
                key,
                ResetCancelStatement {
                    user_id: user(),
                    key_id: key.key_id(),
                    reset_request_id: Uuid::from_bytes([9; 16]),
                },
            )
            .unwrap(),
        }
    }

    /// Each stored form, and the check a reader runs on those bytes.
    fn stored_forms(key: &VaultKey, next: &VaultKey) -> Vec<(&'static str, Vec<u8>, Verifier)> {
        let all = statements(key, next);
        let put = Signed {
            signature: key.sign_statement(&all.put).unwrap(),
            statement: all.put,
        };
        let create = Signed {
            signature: key.sign_statement(&all.create).unwrap(),
            statement: all.create,
        };
        vec![
            ("put", put.to_bytes(), verify_put as Verifier),
            ("create", create.to_bytes(), verify_create),
            ("rotate", all.rotate.to_bytes(), verify_rotate),
            ("delete", all.delete.to_bytes(), verify_delete),
            ("handoff", all.handoff.to_bytes(), verify_handoff),
            ("reset cancel", all.cancel.to_bytes(), verify_cancel),
        ]
    }

    type Verifier = fn(&[u8], &WritePublicKey) -> Result<(), StatementError>;

    fn verify_put(bytes: &[u8], trusted: &WritePublicKey) -> Result<(), StatementError> {
        let signed = Signed::<PutStatement>::parse(bytes)?;
        Ok(signed
            .statement
            .verify(&signed.signature, &user(), trusted)?)
    }

    fn verify_create(bytes: &[u8], _: &WritePublicKey) -> Result<(), StatementError> {
        let signed = Signed::<CreateStatement>::parse(bytes)?;
        Ok(signed.statement.verify(&signed.signature, &user())?)
    }

    fn verify_rotate(bytes: &[u8], trusted: &WritePublicKey) -> Result<(), StatementError> {
        SignedRotation::parse(bytes)?.verify(&user(), trusted)?;
        Ok(())
    }

    fn verify_delete(bytes: &[u8], trusted: &WritePublicKey) -> Result<(), StatementError> {
        Ok(Signed::<DeleteStatement>::parse(bytes)?.verify(&user(), trusted)?)
    }

    fn verify_handoff(bytes: &[u8], trusted: &WritePublicKey) -> Result<(), StatementError> {
        Ok(Signed::<HandoffStatement>::parse(bytes)?.verify_handoff(&user(), trusted)?)
    }

    fn verify_cancel(bytes: &[u8], trusted: &WritePublicKey) -> Result<(), StatementError> {
        Ok(Signed::<ResetCancelStatement>::parse(bytes)?.verify(&user(), trusted)?)
    }

    #[derive(Debug)]
    #[allow(dead_code)]
    enum StatementError {
        Format(FormatError),
        Crypto(CryptoError),
    }

    impl From<FormatError> for StatementError {
        fn from(error: FormatError) -> Self {
            Self::Format(error)
        }
    }

    impl From<CryptoError> for StatementError {
        fn from(error: CryptoError) -> Self {
            Self::Crypto(error)
        }
    }

    #[test]
    fn every_statement_verifies_under_the_key_that_signed_it() {
        let (key, next) = (test_key(), other_key());
        for (name, bytes, verify) in stored_forms(&key, &next) {
            verify(&bytes, key.write_public_key())
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        }
    }

    #[test]
    fn changing_any_byte_of_a_signed_statement_fails_it() {
        let (key, next) = (test_key(), other_key());
        for (name, bytes, verify) in stored_forms(&key, &next) {
            for index in 0..bytes.len() {
                for bit in [0x01, 0x80] {
                    let mut changed = bytes.clone();
                    changed[index] ^= bit;
                    assert!(
                        verify(&changed, key.write_public_key()).is_err(),
                        "{name}: byte {index} bit {bit:#04x} still verifies"
                    );
                }
            }
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(
                verify(&longer, key.write_public_key()).is_err(),
                "{name}: trailing byte"
            );
            assert!(
                verify(&bytes[..bytes.len() - 1], key.write_public_key()).is_err(),
                "{name}: truncated"
            );
        }
    }

    #[test]
    fn a_statement_does_not_verify_under_another_key_or_for_another_account() {
        let (key, next) = (test_key(), other_key());
        for (name, bytes, verify) in stored_forms(&key, &next) {
            if name != "create" {
                assert!(verify(&bytes, next.write_public_key()).is_err(), "{name}");
            }
        }

        let all = statements(&key, &next);
        let signature = key.sign_statement(&all.put).unwrap();
        assert_eq!(
            all.put
                .verify(&signature, &other_user(), key.write_public_key()),
            Err(CryptoError::UserMismatch)
        );
        assert_eq!(
            all.delete.verify(&other_user(), key.write_public_key()),
            Err(CryptoError::UserMismatch)
        );
        assert_eq!(
            all.delete.verify(&user(), next.write_public_key()),
            Err(CryptoError::KeyMismatch)
        );
    }

    #[test]
    fn every_statement_round_trips_through_its_bytes() {
        let all = statements(&test_key(), &other_key());
        assert_eq!(PutStatement::parse(&all.put.to_bytes()).unwrap(), all.put);
        assert_eq!(
            CreateStatement::parse(&all.create.to_bytes()).unwrap(),
            all.create
        );
        assert_eq!(
            RotateStatement::parse(&all.rotate.statement.to_bytes()).unwrap(),
            all.rotate.statement
        );
        assert_eq!(
            SignedRotation::parse(&all.rotate.to_bytes()).unwrap(),
            all.rotate
        );
        assert_eq!(
            Signed::<DeleteStatement>::parse(&all.delete.to_bytes()).unwrap(),
            all.delete
        );
        assert_eq!(
            Signed::<HandoffStatement>::parse(&all.handoff.to_bytes()).unwrap(),
            all.handoff
        );
        assert_eq!(
            Signed::<ResetCancelStatement>::parse(&all.cancel.to_bytes()).unwrap(),
            all.cancel
        );
        assert_eq!(
            DeleteStatement::parse(&all.delete.to_bytes()),
            Err(FormatError::TrailingBytes)
        );
        assert_eq!(
            DeleteStatement::parse(&all.handoff.statement.to_bytes()),
            Err(FormatError::WrongTag)
        );
    }

    /// The stored forms stay inside velvt-core's column limits (§4.2): 512 bytes
    /// for a rotation, 256 for a tombstone.
    #[test]
    fn stored_statements_fit_the_server_columns() {
        let all = statements(&test_key(), &other_key());
        assert!(all.rotate.to_bytes().len() <= 512);
        assert!(all.delete.to_bytes().len() <= 256);
    }

    /// No tag is a prefix of another, and each is NUL-terminated, so a signature
    /// over one statement kind can never be read as a signature over another.
    #[test]
    fn statement_tags_are_distinct_and_terminated() {
        let all = statements(&test_key(), &other_key());
        let tags: Vec<Vec<u8>> = [
            all.put.to_bytes(),
            all.create.to_bytes(),
            all.rotate.statement.to_bytes(),
            all.delete.statement.to_bytes(),
            all.handoff.statement.to_bytes(),
            all.cancel.statement.to_bytes(),
        ]
        .iter()
        .map(|bytes| {
            let end = bytes.iter().position(|&byte| byte == 0).unwrap();
            bytes[..=end].to_vec()
        })
        .collect();
        for (index, tag) in tags.iter().enumerate() {
            assert!(tag.starts_with(b"velvt-vault-"));
            for (other_index, other) in tags.iter().enumerate() {
                if index != other_index {
                    assert!(!other.starts_with(tag), "{tag:?} prefixes {other:?}");
                }
            }
        }
    }

    #[test]
    fn a_rotation_must_be_signed_by_both_keys_and_name_both_correctly() {
        let (old, new) = (test_key(), other_key());
        let rotation = SignedRotation::sign(user(), &old, &new, 43, device(5)).unwrap();
        assert_eq!(
            rotation.verify(&user(), old.write_public_key()),
            Ok(*new.write_public_key())
        );

        let only_old = SignedRotation {
            new_signature: rotation.old_signature,
            ..rotation.clone()
        };
        assert_eq!(
            only_old.verify(&user(), old.write_public_key()),
            Err(CryptoError::BadSignature)
        );

        let mut unrelated_pk = rotation.clone();
        unrelated_pk.statement.new_write_pk = *test_key().write_public_key();
        assert_eq!(
            unrelated_pk.verify(&user(), old.write_public_key()),
            Err(CryptoError::KeyMismatch)
        );

        let to_itself = SignedRotation::sign(user(), &old, &old, 43, device(5)).unwrap();
        assert_eq!(
            to_itself.verify(&user(), old.write_public_key()),
            Err(CryptoError::KeyMismatch)
        );

        assert_eq!(
            rotation.verify(&user(), new.write_public_key()),
            Err(CryptoError::KeyMismatch)
        );
    }

    /// J6 and J9 name the Mac that rotated or deleted, so the statements carry
    /// a tag only a holder of `K` can map back to a device: velvt-core, which
    /// stores them, sees no device id, and cannot change the tag without
    /// breaking the signature.
    #[test]
    fn a_rotation_or_deletion_names_its_mac_to_holders_of_the_key_only() {
        let (key, next) = (test_key(), other_key());
        let all = statements(&key, &next);
        let tag = key.issuer_tag(device(5)).unwrap();
        assert_eq!(all.rotate.statement.issuer_tag, tag);
        assert_eq!(all.delete.statement.issuer_tag, tag);

        let devices = [device(1), device(5), device(9)];
        assert_eq!(key.identify_issuer(&tag, &devices), Ok(Some(device(5))));
        assert_eq!(key.identify_issuer(&tag, &devices[..1]), Ok(None));
        assert_eq!(next.identify_issuer(&tag, &devices), Ok(None));

        assert_eq!(key.issuer_tag(device(5)), Ok(tag), "deterministic");
        assert_ne!(key.issuer_tag(device(6)).unwrap(), tag);
        assert_ne!(next.issuer_tag(device(5)).unwrap(), tag);
        let stored = all.delete.to_bytes();
        let id = device(5);
        assert!(
            !stored.windows(16).any(|window| window == id.as_bytes()),
            "the stored statement holds no sync device id"
        );
        assert!(!stored
            .windows(36)
            .any(|window| window == id.to_string().as_bytes()));
        assert!(format!("{tag:?}").starts_with("IssuerTag("));
    }

    #[test]
    fn a_create_statement_must_name_the_key_id_its_own_public_key_derives() {
        let key = test_key();
        let mut create = statements(&key, &other_key()).create;
        create.key_id = other_key().key_id();
        let signature = key.sign_statement(&create).unwrap();
        assert_eq!(
            create.verify(&signature, &user()),
            Err(CryptoError::KeyMismatch)
        );
    }

    #[test]
    fn a_signature_is_sixty_four_bytes_in_base64() {
        let key = test_key();
        let signature = key
            .sign_statement(&statements(&key, &other_key()).put)
            .unwrap();
        assert_eq!(
            Signature::from_base64(&signature.to_base64()).unwrap(),
            signature
        );
        assert_eq!(
            Signature::from_slice(&[0; 63]).err(),
            Some(CryptoError::Encoding)
        );
        let write_pk = key.write_public_key();
        assert_eq!(
            WritePublicKey::from_base64(&write_pk.to_base64()).unwrap(),
            *write_pk
        );
    }

    #[test]
    fn random_bytes_never_parse_as_a_statement_and_never_panic() {
        let mut rng = Rng::new(17);
        for _ in 0..2_000 {
            let length = rng.below(300) as usize;
            let bytes: Vec<u8> = (0..length).map(|_| rng.next() as u8).collect();
            assert!(PutStatement::parse(&bytes).is_err());
            assert!(SignedRotation::parse(&bytes).is_err());
            assert!(Signed::<DeleteStatement>::parse(&bytes).is_err());
        }
    }

    /// Compiles only while none of these types implements `Serialize`: with an
    /// implementation, both impls below apply and the call is ambiguous (the
    /// technique `static_assertions::assert_not_impl_any!` uses).
    #[test]
    fn a_key_a_code_and_an_opened_handoff_cannot_be_serialized() {
        trait AmbiguousIfSerialize<Marker> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
        struct Serializable;
        impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Serializable> for T {}

        <VaultKey as AmbiguousIfSerialize<_>>::check();
        <crate::vault::code::SetupCode as AmbiguousIfSerialize<_>>::check();
        <crate::vault::code::HandoffContents as AmbiguousIfSerialize<_>>::check();
    }
}
