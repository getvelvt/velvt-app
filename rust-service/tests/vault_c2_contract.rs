//! A1's bytes, checked by velvt-core's own rules (C2, velvt-core #46).
//!
//! The module `c2` below is a port of velvt-core's
//! `app/services/vault_formats.py` and of the checks that
//! `app/services/vault_service.py` and `app/schemas/vault.py` make on each
//! request: the six ciphertext lengths (`check_ciphertext`), each statement
//! rebuilt from the request and compared or verified, `parse_delete_statement`
//! at its exact length, the delete statement's ten-minute clock margin, the
//! handoff's expiry in whole seconds, and the version step. It is written from
//! velvt-core's code, not from A1's, and shares nothing with `vault::crypto`
//! except `ring`.
//!
//! Three sources go through it:
//! - velvt-core's own known answers (`tests/test_vault_formats.py`), built with
//!   A1's types;
//! - the published vectors (`docs/architecture/vault-v1-test-vectors.json`),
//!   read as an outside implementation would;
//! - blobs and statements A1 makes now, at every size.
//!
//! A layout, a length or a time unit on which A1 and C2 disagree fails here,
//! rather than as a Mac's write refused with `422 vault_blob_invalid` or
//! `422 vault_statement_invalid`.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::Value;
use uuid::Uuid;

use velvt_service::vault::code::{seal_handoff, SetupCode};
use velvt_service::vault::crypto::statement::{
    CreateStatement, DeleteStatement, HandoffStatement, PutStatement, ResetCancelStatement,
    RotateStatement, Signed, SignedRotation, Statement,
};
use velvt_service::vault::crypto::{
    sealed_size_for, sha256, BlobName, CryptoError, DocumentContext, KeyId, Signature, UserId,
    VaultKey, WritePublicKey, MAX_DOCUMENT_JSON_LENGTH, SEALED_SIZES,
};
use velvt_service::vault::doc::SyncDeviceId;

mod c2 {
    //! velvt-core's vault checks, ported from velvt-core #46 (`feat/vault-store`
    //! at f448a15): `vault_formats.py` function for function, and the parts of
    //! `vault_service.py` and `schemas/vault.py` that look at bytes a Mac signs
    //! or seals. The database, the rate limits and the audit rows are left out.

    use ring::digest::{digest, SHA256};
    use ring::signature::{UnparsedPublicKey, ED25519};
    use uuid::Uuid;

    pub const KEY_ID_TAG: &[u8] = b"velvt-vault-key-id-v1\0";
    pub const PUT_TAG: &[u8] = b"velvt-vault-put-v1\0";
    pub const CREATE_TAG: &[u8] = b"velvt-vault-create-v1\0";
    pub const ROTATE_TAG: &[u8] = b"velvt-vault-rotate-v1\0";
    pub const DELETE_TAG: &[u8] = b"velvt-vault-delete-v1\0";
    pub const HANDOFF_TAG: &[u8] = b"velvt-vault-handoff-v1\0";
    pub const RESET_CANCEL_TAG: &[u8] = b"velvt-vault-reset-cancel-v1\0";

    const PUBLIC_KEY_BYTES: usize = 32;
    const SIGNATURE_BYTES: usize = 64;
    const KEY_ID_BYTES: usize = 8;
    const HANDOFF_ID_BYTES: usize = 16;

    /// `AEAD_OVERHEAD_BYTES = 12 + 16`.
    pub const AEAD_OVERHEAD_BYTES: usize = 12 + 16;
    /// `schemas/vault.py`: `MAX_STATEMENT_BYTES`, `MAX_HANDOFF_PAYLOAD_BYTES`,
    /// `MAX_SCHEMA`.
    pub const MAX_STATEMENT_BYTES: usize = 512;
    pub const MAX_HANDOFF_PAYLOAD_BYTES: usize = 256;
    pub const MAX_SCHEMA: u16 = 32767;
    /// `VERSION_STEP_LIMIT = 2**32`.
    pub const VERSION_STEP_LIMIT: u64 = 1 << 32;
    /// `STATEMENT_CLOCK_MARGIN = timedelta(minutes=10)`.
    pub const STATEMENT_CLOCK_MARGIN_SECONDS: i64 = 600;
    /// `HANDOFF_MAX_TTL = timedelta(seconds=600)`.
    pub const HANDOFF_MAX_TTL_SECONDS: i64 = 600;

    /// `PADDED_DOCUMENT_BYTES = tuple(2**power * 1024 for power in range(4, 10))`.
    pub fn padded_document_bytes() -> Vec<usize> {
        (4..10).map(|power| (1_usize << power) * 1024).collect()
    }

    /// `CIPHERTEXT_BYTES`: each padded size plus the nonce and the tag.
    pub fn ciphertext_bytes() -> Vec<usize> {
        padded_document_bytes()
            .into_iter()
            .map(|size| size + AEAD_OVERHEAD_BYTES)
            .collect()
    }

    /// What velvt-core answers instead of accepting.
    #[derive(Debug, PartialEq, Eq)]
    pub enum Refused {
        /// `413 vault_too_large`.
        TooLarge,
        /// `422 vault_blob_invalid`.
        BlobInvalid,
        /// `422 vault_statement_invalid`.
        StatementInvalid,
        /// `422 vault_key_id_invalid`.
        KeyIdInvalid,
        /// `422 vault_signature_invalid` (create) or `403
        /// vault_write_unauthorized` (every other write).
        Signature,
        /// `422 vault_version_invalid`.
        VersionInvalid,
        /// `412 vault_version_conflict`.
        VersionConflict,
        /// `409 vault_key_mismatch`.
        KeyMismatch,
        /// `422 vault_handoff_expired`.
        HandoffExpired,
        /// `422 validation_error`: a body field outside its schema.
        Validation,
        /// `VaultFormatError` from a builder or the delete parser.
        Format,
    }

    pub fn sha256(value: &[u8]) -> [u8; 32] {
        let mut out = [0_u8; 32];
        out.copy_from_slice(digest(&SHA256, value).as_ref());
        out
    }

    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Python's `bytes.fromhex`, for the even-length hex this file feeds it.
    fn from_hex(text: &str) -> Result<Vec<u8>, Refused> {
        if !text.len().is_multiple_of(2) {
            return Err(Refused::Format);
        }
        (0..text.len())
            .step_by(2)
            .map(|index| {
                u8::from_str_radix(&text[index..index + 2], 16).map_err(|_| Refused::Format)
            })
            .collect()
    }

    fn lp(value: &[u8]) -> Result<Vec<u8>, Refused> {
        let length = u16::try_from(value.len()).map_err(|_| Refused::Format)?;
        let mut out = length.to_be_bytes().to_vec();
        out.extend_from_slice(value);
        Ok(out)
    }

    fn user(user_id: &str) -> Result<Vec<u8>, Refused> {
        lp(user_id.as_bytes())
    }

    /// `hex(SHA-256("velvt-vault-key-id-v1\0" ‖ write_pk)[0..8])`.
    pub fn key_id_for(write_pk: &[u8]) -> String {
        hex(&sha256(&[KEY_ID_TAG, write_pk].concat())[..KEY_ID_BYTES])
    }

    fn key_id_bytes(key_id: &str) -> Result<Vec<u8>, Refused> {
        let raw = from_hex(key_id)?;
        if raw.len() != KEY_ID_BYTES {
            return Err(Refused::Format);
        }
        Ok(raw)
    }

    /// `KEY_ID_PATTERN = r"^[0-9a-f]{16}$"`.
    fn check_key_id_field(key_id: &str) -> Result<(), Refused> {
        let ok = key_id.len() == 16
            && key_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        ok.then_some(()).ok_or(Refused::Validation)
    }

    pub fn put_statement(
        user_id: &str,
        name: &str,
        key_id: &str,
        schema: u16,
        base_version: u64,
        new_version: u64,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, Refused> {
        Ok([
            PUT_TAG.to_vec(),
            user(user_id)?,
            lp(name.as_bytes())?,
            key_id_bytes(key_id)?,
            schema.to_be_bytes().to_vec(),
            base_version.to_be_bytes().to_vec(),
            new_version.to_be_bytes().to_vec(),
            sha256(ciphertext).to_vec(),
        ]
        .concat())
    }

    pub fn create_statement(
        user_id: &str,
        key_id: &str,
        write_pk: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, Refused> {
        Ok([
            CREATE_TAG.to_vec(),
            user(user_id)?,
            key_id_bytes(key_id)?,
            write_pk.to_vec(),
            sha256(ciphertext).to_vec(),
        ]
        .concat())
    }

    pub fn rotate_statement(
        user_id: &str,
        old_key_id: &str,
        new_key_id: &str,
        new_write_pk: &[u8],
        new_version: u64,
    ) -> Result<Vec<u8>, Refused> {
        Ok([
            ROTATE_TAG.to_vec(),
            user(user_id)?,
            key_id_bytes(old_key_id)?,
            key_id_bytes(new_key_id)?,
            new_write_pk.to_vec(),
            new_version.to_be_bytes().to_vec(),
        ]
        .concat())
    }

    pub fn handoff_statement(
        user_id: &str,
        handoff_id: &str,
        payload: &[u8],
        expires_at: i64,
    ) -> Result<Vec<u8>, Refused> {
        let raw_id = from_hex(handoff_id)?;
        if raw_id.len() != HANDOFF_ID_BYTES {
            return Err(Refused::Format);
        }
        Ok([
            HANDOFF_TAG.to_vec(),
            user(user_id)?,
            raw_id,
            sha256(payload).to_vec(),
            expires_at.to_be_bytes().to_vec(),
        ]
        .concat())
    }

    pub fn reset_cancel_statement(
        user_id: &str,
        key_id: &str,
        reset_request_id: &str,
    ) -> Result<Vec<u8>, Refused> {
        let id = Uuid::parse_str(reset_request_id).map_err(|_| Refused::Format)?;
        Ok([
            RESET_CANCEL_TAG.to_vec(),
            user(user_id)?,
            key_id_bytes(key_id)?,
            id.as_bytes().to_vec(),
        ]
        .concat())
    }

    #[derive(Debug, PartialEq, Eq)]
    pub struct DeleteStatement {
        pub user_id: String,
        pub key_id: String,
        pub at_version: u64,
        pub requested_at: i64,
    }

    /// `"velvt-vault-delete-v1\0" ‖ lp(user_id) ‖ key_id ‖ u64_be(at_version) ‖
    /// i64_be(requested_at)`, with nothing after it.
    pub fn parse_delete_statement(statement: &[u8]) -> Result<DeleteStatement, Refused> {
        if !statement.starts_with(DELETE_TAG) {
            return Err(Refused::Format);
        }
        let mut offset = DELETE_TAG.len();
        if statement.len() < offset + 2 {
            return Err(Refused::Format);
        }
        let user_length = usize::from(u16::from_be_bytes([
            statement[offset],
            statement[offset + 1],
        ]));
        offset += 2;
        if statement.len() != offset + user_length + KEY_ID_BYTES + 8 + 8 {
            return Err(Refused::Format);
        }
        let user_id = std::str::from_utf8(&statement[offset..offset + user_length])
            .map_err(|_| Refused::Format)?
            .to_owned();
        offset += user_length;
        let key_id = hex(&statement[offset..offset + KEY_ID_BYTES]);
        offset += KEY_ID_BYTES;
        let at_version = u64::from_be_bytes(statement[offset..offset + 8].try_into().unwrap());
        let requested_at =
            i64::from_be_bytes(statement[offset + 8..offset + 16].try_into().unwrap());
        Ok(DeleteStatement {
            user_id,
            key_id,
            at_version,
            requested_at,
        })
    }

    pub fn signature_is_valid(public_key: &[u8], signature: &[u8], message: &[u8]) -> bool {
        public_key.len() == PUBLIC_KEY_BYTES
            && signature.len() == SIGNATURE_BYTES
            && UnparsedPublicKey::new(&ED25519, public_key)
                .verify(message, signature)
                .is_ok()
    }

    /// `check_ciphertext`.
    pub fn check_ciphertext(ciphertext: &[u8]) -> Result<(), Refused> {
        let allowed = ciphertext_bytes();
        if ciphertext.len() > *allowed.iter().max().unwrap() {
            return Err(Refused::TooLarge);
        }
        if !allowed.contains(&ciphertext.len()) {
            return Err(Refused::BlobInvalid);
        }
        Ok(())
    }

    /// `_check_new_version`.
    pub fn check_new_version(base_version: u64, new_version: u64) -> Result<(), Refused> {
        let ok = base_version < new_version
            && u128::from(new_version) <= u128::from(base_version) + u128::from(VERSION_STEP_LIMIT);
        ok.then_some(()).ok_or(Refused::VersionInvalid)
    }

    /// `Statement = …_between(1, MAX_STATEMENT_BYTES)`.
    fn check_statement_field(statement: &[u8]) -> Result<(), Refused> {
        (1..=MAX_STATEMENT_BYTES)
            .contains(&statement.len())
            .then_some(())
            .ok_or(Refused::Validation)
    }

    /// `SchemaVersion = Field(ge=1, le=MAX_SCHEMA)`.
    fn check_schema_field(schema: u16) -> Result<(), Refused> {
        (1..=MAX_SCHEMA)
            .contains(&schema)
            .then_some(())
            .ok_or(Refused::Validation)
    }

    /// One account's row in `vaults`, with its blob's version.
    #[derive(Clone, Debug)]
    pub struct Vault {
        pub user_id: String,
        pub key_id: String,
        pub write_pk: Vec<u8>,
        pub version: u64,
    }

    /// The body of `POST /v1/vault`, decoded.
    pub struct CreateRequest<'a> {
        pub key_id: &'a str,
        pub write_pk: &'a [u8],
        pub schema: u16,
        pub ciphertext: &'a [u8],
        pub create_sig: &'a [u8],
        pub put_sig: &'a [u8],
    }

    /// `create_vault`: the vault it would store, at version 1.
    pub fn create_vault(user_id: &str, request: &CreateRequest<'_>) -> Result<Vault, Refused> {
        check_key_id_field(request.key_id)?;
        check_schema_field(request.schema)?;
        check_ciphertext(request.ciphertext)?;
        if key_id_for(request.write_pk) != request.key_id {
            return Err(Refused::KeyIdInvalid);
        }
        let signed_create = create_statement(
            user_id,
            request.key_id,
            request.write_pk,
            request.ciphertext,
        )?;
        let signed_put = put_statement(
            user_id,
            "prefs",
            request.key_id,
            request.schema,
            0,
            1,
            request.ciphertext,
        )?;
        if !(signature_is_valid(request.write_pk, request.create_sig, &signed_create)
            && signature_is_valid(request.write_pk, request.put_sig, &signed_put))
        {
            return Err(Refused::Signature);
        }
        Ok(Vault {
            user_id: user_id.to_owned(),
            key_id: request.key_id.to_owned(),
            write_pk: request.write_pk.to_vec(),
            version: 1,
        })
    }

    /// The body of `PUT /v1/vault/blobs/prefs`, decoded, and its `If-Match`.
    pub struct PutRequest<'a> {
        pub if_match: u64,
        pub key_id: &'a str,
        pub schema: u16,
        pub new_version: u64,
        pub ciphertext: &'a [u8],
        pub put_sig: &'a [u8],
    }

    /// `put_blob`: the new version.
    pub fn put_blob(vault: &mut Vault, request: &PutRequest<'_>) -> Result<u64, Refused> {
        check_key_id_field(request.key_id)?;
        check_schema_field(request.schema)?;
        check_ciphertext(request.ciphertext)?;
        if request.key_id != vault.key_id {
            return Err(Refused::KeyMismatch);
        }
        if request.if_match != vault.version {
            return Err(Refused::VersionConflict);
        }
        check_new_version(request.if_match, request.new_version)?;
        let signed = put_statement(
            &vault.user_id,
            "prefs",
            &vault.key_id,
            request.schema,
            request.if_match,
            request.new_version,
            request.ciphertext,
        )?;
        if !signature_is_valid(&vault.write_pk, request.put_sig, &signed) {
            return Err(Refused::Signature);
        }
        vault.version = request.new_version;
        Ok(vault.version)
    }

    /// The body of `POST /v1/vault/rotate`, decoded, and its `If-Match`.
    pub struct RotateRequest<'a> {
        pub if_match: u64,
        pub old_key_id: &'a str,
        pub new_key_id: &'a str,
        pub new_write_pk: &'a [u8],
        pub rotate_stmt: &'a [u8],
        pub old_sig: &'a [u8],
        pub new_sig: &'a [u8],
        pub schema: u16,
        pub new_version: u64,
        pub ciphertext: &'a [u8],
        pub put_sig: &'a [u8],
    }

    /// `rotate_vault`: the record it appends to `vaults.rotations`
    /// (statement ‖ old signature ‖ new signature).
    pub fn rotate_vault(
        vault: &mut Vault,
        request: &RotateRequest<'_>,
    ) -> Result<Vec<u8>, Refused> {
        check_key_id_field(request.old_key_id)?;
        check_key_id_field(request.new_key_id)?;
        check_statement_field(request.rotate_stmt)?;
        check_schema_field(request.schema)?;
        check_ciphertext(request.ciphertext)?;
        if key_id_for(request.new_write_pk) != request.new_key_id
            || request.new_key_id == request.old_key_id
        {
            return Err(Refused::KeyIdInvalid);
        }
        if request.old_key_id != vault.key_id {
            return Err(Refused::KeyMismatch);
        }
        if request.if_match != vault.version {
            return Err(Refused::VersionConflict);
        }
        check_new_version(request.if_match, request.new_version)?;
        let signed_rotation = rotate_statement(
            &vault.user_id,
            &vault.key_id,
            request.new_key_id,
            request.new_write_pk,
            request.new_version,
        )?;
        if request.rotate_stmt != signed_rotation {
            return Err(Refused::StatementInvalid);
        }
        let signed_put = put_statement(
            &vault.user_id,
            "prefs",
            request.new_key_id,
            request.schema,
            request.if_match,
            request.new_version,
            request.ciphertext,
        )?;
        if !(signature_is_valid(&vault.write_pk, request.old_sig, &signed_rotation)
            && signature_is_valid(request.new_write_pk, request.new_sig, &signed_rotation)
            && signature_is_valid(request.new_write_pk, request.put_sig, &signed_put))
        {
            return Err(Refused::Signature);
        }
        vault.key_id = request.new_key_id.to_owned();
        vault.write_pk = request.new_write_pk.to_vec();
        vault.version = request.new_version;
        Ok([
            signed_rotation,
            request.old_sig.to_vec(),
            request.new_sig.to_vec(),
        ]
        .concat())
    }

    /// The body of `DELETE /v1/vault`, decoded.
    pub struct DeleteRequest<'a> {
        pub key_id: &'a str,
        pub delete_stmt: &'a [u8],
        pub sig: &'a [u8],
    }

    /// `delete_vault` at the server's clock `now` (Unix seconds): the
    /// tombstone it keeps (statement ‖ signature).
    pub fn delete_vault(
        vault: &Vault,
        request: &DeleteRequest<'_>,
        now: i64,
    ) -> Result<Vec<u8>, Refused> {
        check_key_id_field(request.key_id)?;
        check_statement_field(request.delete_stmt)?;
        let statement =
            parse_delete_statement(request.delete_stmt).map_err(|_| Refused::StatementInvalid)?;
        if statement.user_id != vault.user_id || statement.key_id != request.key_id {
            return Err(Refused::StatementInvalid);
        }
        if (i128::from(statement.requested_at) - i128::from(now)).abs()
            > i128::from(STATEMENT_CLOCK_MARGIN_SECONDS)
        {
            return Err(Refused::StatementInvalid);
        }
        if request.key_id != vault.key_id {
            return Err(Refused::KeyMismatch);
        }
        if statement.at_version != vault.version {
            return Err(Refused::VersionConflict);
        }
        if !signature_is_valid(&vault.write_pk, request.sig, request.delete_stmt) {
            return Err(Refused::Signature);
        }
        Ok([request.delete_stmt, request.sig].concat())
    }

    /// The body of `DELETE /v1/vault/reset-requests/{id}`, decoded, and the id.
    pub struct ResetCancelRequest<'a> {
        pub reset_request_id: &'a str,
        pub cancel_stmt: &'a [u8],
        pub sig: &'a [u8],
    }

    /// `cancel_reset`, for a pending reset with this id.
    pub fn cancel_reset(vault: &Vault, request: &ResetCancelRequest<'_>) -> Result<(), Refused> {
        check_statement_field(request.cancel_stmt)?;
        let signed =
            reset_cancel_statement(&vault.user_id, &vault.key_id, request.reset_request_id)?;
        if request.cancel_stmt != signed {
            return Err(Refused::StatementInvalid);
        }
        if !signature_is_valid(&vault.write_pk, request.sig, &signed) {
            return Err(Refused::Signature);
        }
        Ok(())
    }

    /// The body of `POST /v1/vault/handoffs`, decoded.
    pub struct HandoffRequest<'a> {
        pub handoff_id: &'a str,
        pub payload: &'a [u8],
        pub expires_at: i64,
        pub sig: &'a [u8],
    }

    /// `create_handoff` at the server's clock `now` (Unix seconds): when the
    /// stored handoff expires.
    pub fn create_handoff(
        vault: &Vault,
        request: &HandoffRequest<'_>,
        now: i64,
    ) -> Result<i64, Refused> {
        let id_ok = request.handoff_id.len() == 32
            && request
                .handoff_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !id_ok
            || !(1..=MAX_HANDOFF_PAYLOAD_BYTES).contains(&request.payload.len())
            || request.expires_at < 0
        {
            return Err(Refused::Validation);
        }
        if request.expires_at <= now {
            return Err(Refused::HandoffExpired);
        }
        let signed = handoff_statement(
            &vault.user_id,
            request.handoff_id,
            request.payload,
            request.expires_at,
        )?;
        if !signature_is_valid(&vault.write_pk, request.sig, &signed) {
            return Err(Refused::Signature);
        }
        Ok(request.expires_at.min(now + HANDOFF_MAX_TTL_SECONDS))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/architecture/vault-v1-test-vectors.json"
);

/// The vectors' clock, 1,760,486,400 Unix seconds: the delete statement's
/// `requested_at`, and ten minutes before the handoff's `expires_at`.
const VECTORS_NOW: i64 = 1_760_486_400;

fn vectors() -> Value {
    serde_json::from_str(&std::fs::read_to_string(VECTORS).expect("the published vectors"))
        .expect("JSON")
}

fn decode_hex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "odd-length hex");
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

fn unhex(value: &Value) -> Vec<u8> {
    decode_hex(text(value))
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("a string, got {value}"))
}

fn integer(value: &Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| panic!("an integer, got {value}"))
}

/// `HKDF-SHA256(ikm = K, salt = ∅, info, L = 32)`, by `ring` directly.
fn hkdf32(ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let prk = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &[]).extract(ikm);
    let mut out = [0_u8; 32];
    prk.expand(&[info], ring::hkdf::HKDF_SHA256)
        .expect("32 bytes")
        .fill(&mut out)
        .expect("32 bytes");
    out
}

/// §5.3's sealing, done here from the design text rather than by A1:
/// `nonce ‖ ChaCha20-Poly1305(K_doc, nonce, aad, u32_be(len) ‖ json ‖ 0…)`,
/// with the plaintext padded to the smallest of velvt-core's six padded sizes.
fn seal_independently(k: &[u8], nonce: &[u8], aad: &[u8], json: &[u8]) -> Vec<u8> {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
    let padded = c2::padded_document_bytes()
        .into_iter()
        .find(|&size| json.len() + 4 <= size)
        .expect("fits");
    let mut plain = vec![0_u8; padded];
    plain[..4].copy_from_slice(&u32::try_from(json.len()).unwrap().to_be_bytes());
    plain[4..4 + json.len()].copy_from_slice(json);
    let doc_key = hkdf32(k, b"velvt-vault-doc-v1");
    let key = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &doc_key).unwrap());
    key.seal_in_place_append_tag(
        Nonce::try_assume_unique_for_key(nonce).unwrap(),
        Aad::from(aad),
        &mut plain,
    )
    .unwrap();
    [nonce, &plain[..]].concat()
}

// ---------------------------------------------------------------------------
// velvt-core's own known answers (tests/test_vault_formats.py)
// ---------------------------------------------------------------------------

/// `test_known_answer_key_id_put_statement_and_signature`: an RFC 8032 key
/// from the seed `00 01 … 1f`, its key id, and a put statement over a 16 KiB
/// ciphertext of zeros with its deterministic signature.
const C2_USER_ID: &str = "00000000-0000-4000-8000-000000000001";
const C2_WRITE_PK: &str = "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8";
const C2_KEY_ID: &str = "c05e0fccb0eb6bfd";
const C2_PUT_STATEMENT: &str = concat!(
    "76656c76742d7661756c742d7075742d763100",
    "0024",
    "30303030303030302d303030302d343030302d383030302d303030303030303030303031",
    "0005",
    "7072656673",
    "c05e0fccb0eb6bfd",
    "0001",
    "0000000000000000",
    "0000000000000001",
    "9cca0796d7a25ae35f742f96dee6e620444e3364bce4f9c57307cfd6c1800802",
);
const C2_PUT_SIGNATURE: &str = concat!(
    "9a0b592f926e5eae599713be81ca4c111167209291a3220f4a3fc38de4c0af38",
    "2db00749fa193ecb33bb6828ee10ff542bd708c318bac6cbaccee55bf1533b08",
);

fn c2_user() -> UserId {
    UserId::new(C2_USER_ID).unwrap()
}

fn c2_key_id() -> KeyId {
    KeyId::from_hex(C2_KEY_ID).unwrap()
}

/// C2's known answer is a put over `EMPTY_16_KIB_CIPHERTEXT`, 16,412 zero
/// bytes: 16 KiB and 28, its smallest accepted ciphertext. A1 builds the same
/// statement from the same fields, accepts C2's signature over it, and its
/// smallest sealed size is that length.
#[test]
fn a1_builds_velvt_cores_known_answer_put_and_accepts_its_signature() {
    let write_pk = WritePublicKey::from_slice(&decode_hex(C2_WRITE_PK)).unwrap();
    assert_eq!(write_pk.key_id().to_hex(), C2_KEY_ID);
    assert_eq!(c2::key_id_for(write_pk.as_bytes()), C2_KEY_ID);

    let empty_16_kib_ciphertext = vec![0_u8; 16 * 1024 + 28];
    assert_eq!(
        SEALED_SIZES[0],
        empty_16_kib_ciphertext.len(),
        "A1's smallest sealed document is not the smallest ciphertext velvt-core stores"
    );
    let put = PutStatement {
        user_id: c2_user(),
        name: BlobName::prefs(),
        key_id: c2_key_id(),
        schema: 1,
        base_version: 0,
        new_version: 1,
        ciphertext_sha256: c2::sha256(&empty_16_kib_ciphertext),
    };
    assert_eq!(c2::hex(&put.to_bytes()), C2_PUT_STATEMENT);
    assert_eq!(put.to_bytes().len(), 122);
    let signature = Signature::from_slice(&decode_hex(C2_PUT_SIGNATURE)).unwrap();
    put.verify(&signature, &c2_user(), &write_pk).unwrap();
    assert_eq!(PutStatement::parse(&put.to_bytes()).unwrap(), put);
}

/// `test_a_delete_statement_parses_back_to_its_fields`,
/// `test_a_delete_statement_with_trailing_missing_or_foreign_bytes_is_refused`
/// and `test_the_cancel_statement_carries_the_reset_ids_16_bytes`: A1 writes
/// velvt-core's delete statement byte for byte, with `requested_at` in whole
/// seconds and nothing after it, and refuses every delete statement velvt-core
/// refuses. The last refused input is the layout A1 first shipped, §5.3's
/// statement followed by a 16-byte issuer tag (design §17.1 D13).
#[test]
fn a1_agrees_with_velvt_cores_delete_and_cancel_known_answers() {
    let delete = DeleteStatement {
        user_id: c2_user(),
        key_id: c2_key_id(),
        at_version: 42,
        requested_at: 1_760_500_000,
    };
    let expected = [
        c2::DELETE_TAG,
        &[0x00, 0x24],
        C2_USER_ID.as_bytes(),
        &decode_hex(C2_KEY_ID),
        &42_u64.to_be_bytes(),
        &1_760_500_000_i64.to_be_bytes(),
    ]
    .concat();
    assert_eq!(delete.to_bytes(), expected);
    assert_eq!(DeleteStatement::parse(&expected).unwrap(), delete);
    assert_eq!(
        c2::parse_delete_statement(&expected).unwrap(),
        c2::DeleteStatement {
            user_id: C2_USER_ID.to_owned(),
            key_id: C2_KEY_ID.to_owned(),
            at_version: 42,
            requested_at: 1_760_500_000,
        }
    );

    let user = C2_USER_ID.as_bytes();
    let refused: [Vec<u8>; 8] = [
        Vec::new(),
        c2::DELETE_TAG.to_vec(),
        [c2::DELETE_TAG, &[0x00]].concat(),
        [
            c2::DELETE_TAG,
            &[0x00, 0x24],
            user,
            &[0; 8],
            &[0; 16],
            &[0x00],
        ]
        .concat(),
        [c2::DELETE_TAG, &[0x00, 0x24], user, &[0; 8], &[0; 15]].concat(),
        [c2::DELETE_TAG, &[0x00, 0x02, 0xff, 0xfe], &[0; 8], &[0; 16]].concat(),
        [
            b"velvt-vault-delete-v2\0".as_slice(),
            &[0x00, 0x24],
            user,
            &[0; 24],
        ]
        .concat(),
        [expected.as_slice(), &[0x5e; 16]].concat(),
    ];
    for bytes in refused {
        assert!(c2::parse_delete_statement(&bytes).is_err(), "{bytes:02x?}");
        assert!(DeleteStatement::parse(&bytes).is_err(), "{bytes:02x?}");
    }

    let reset_id = "6f1c1a1e-1111-4222-8333-944455556666";
    let cancel = ResetCancelStatement {
        user_id: c2_user(),
        key_id: c2_key_id(),
        reset_request_id: Uuid::parse_str(reset_id).unwrap(),
    };
    let tail = [
        decode_hex(C2_KEY_ID),
        Uuid::parse_str(reset_id).unwrap().as_bytes().to_vec(),
    ]
    .concat();
    assert!(cancel.to_bytes().ends_with(&tail));
    assert_eq!(
        cancel.to_bytes(),
        c2::reset_cancel_statement(C2_USER_ID, C2_KEY_ID, reset_id).unwrap()
    );
}

// ---------------------------------------------------------------------------
// The published vectors, through velvt-core's checks
// ---------------------------------------------------------------------------

/// Every sealed length the vectors publish is one velvt-core stores: the
/// document itself, and each row of the padding table.
#[test]
fn every_published_sealed_length_is_one_velvt_core_stores() {
    let vectors = vectors();
    let document = &vectors["document"];
    let sealed = BASE64.decode(text(&document["sealed_b64"])).unwrap();
    assert_eq!(sealed.len() as i64, integer(&document["sealed_length"]));
    assert_eq!(c2::check_ciphertext(&sealed), Ok(()), "the document vector");
    assert_eq!(
        c2::hex(&c2::sha256(&sealed)),
        text(&document["sealed_sha256_hex"])
    );

    let allowed = c2::ciphertext_bytes();
    let rows = vectors["padding"].as_array().unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        let json_length = integer(&row["json_length"]) as usize;
        let sealed_length = integer(&row["sealed_length"]) as usize;
        assert!(
            allowed.contains(&sealed_length),
            "a {json_length}-byte document seals to {sealed_length} bytes, which velvt-core refuses"
        );
        let padded = sealed_length - c2::AEAD_OVERHEAD_BYTES;
        assert!(
            json_length + 4 <= padded,
            "{json_length} does not fit {padded}"
        );
        let smaller = c2::padded_document_bytes()
            .into_iter()
            .filter(|&size| size < padded)
            .max();
        assert!(
            smaller.is_none_or(|size| json_length + 4 > size),
            "a {json_length}-byte document is padded past the smallest size that holds it"
        );
    }
    let largest = rows.last().unwrap();
    assert_eq!(
        integer(&largest["sealed_length"]) as usize,
        *allowed.iter().max().unwrap()
    );
}

/// The create and rotation blobs are published only by their SHA-256. Sealing
/// the published document under the published key, nonce and AAD, padded to
/// velvt-core's sizes, gives exactly those hashes, so those blobs have lengths
/// velvt-core stores too.
#[test]
fn the_published_create_and_rotation_blobs_seal_to_lengths_velvt_core_stores() {
    let vectors = vectors();
    let json = text(&vectors["document"]["json"]).as_bytes();
    let k = unhex(&vectors["key"]["k_hex"]);
    assert_eq!(
        c2::hex(&hkdf32(&k, b"velvt-vault-doc-v1")),
        text(&vectors["key"]["k_doc_hex"])
    );
    let next_k = unhex(&vectors["next_key"]["k_hex"]);
    let document_aad = unhex(&vectors["document"]["aad_hex"]);
    let document_nonce = unhex(&vectors["document"]["nonce_hex"]);
    let statements = &vectors["statements"];
    for (label, k, nonce, aad, digest, published) in [
        (
            "document",
            &k,
            document_nonce,
            document_aad,
            &vectors["document"]["sealed_sha256_hex"],
            Some(
                BASE64
                    .decode(text(&vectors["document"]["sealed_b64"]))
                    .unwrap(),
            ),
        ),
        (
            "create",
            &k,
            unhex(&statements["create"]["create_nonce_hex"]),
            unhex(&statements["create"]["aad_hex"]),
            &statements["create"]["sealed_sha256_hex"],
            None,
        ),
        (
            "rotate_put",
            &next_k,
            unhex(&statements["rotate_put"]["rotate_nonce_hex"]),
            unhex(&statements["rotate_put"]["aad_hex"]),
            &statements["rotate_put"]["sealed_sha256_hex"],
            None,
        ),
    ] {
        let sealed = seal_independently(k, &nonce, &aad, json);
        assert_eq!(c2::check_ciphertext(&sealed), Ok(()), "{label}");
        assert_eq!(
            c2::hex(&c2::sha256(&sealed)),
            text(digest),
            "{label}: the published blob is not the document padded to velvt-core's sizes"
        );
        if let Some(published) = published {
            assert_eq!(sealed, published, "{label}");
        }
    }
}

/// `POST /v1/vault`, `PUT /v1/vault/blobs/prefs` and the rotation's put: each
/// published put and create statement is the one velvt-core builds from the
/// request, and its signature verifies under the key velvt-core holds.
#[test]
fn the_published_put_and_create_statements_are_the_ones_velvt_core_builds() {
    let vectors = vectors();
    let statements = &vectors["statements"];
    let user_id = text(&vectors["document"]["user_id"]);
    let name = text(&vectors["document"]["name"]);
    assert_eq!(name, "prefs");
    let write_pk = unhex(&vectors["key"]["write_pk_hex"]);
    let key_id = text(&vectors["key"]["key_id"]);
    assert_eq!(c2::key_id_for(&write_pk), key_id);
    let next_write_pk = unhex(&vectors["next_key"]["write_pk_hex"]);
    let next_key_id = text(&vectors["next_key"]["key_id"]);
    assert_eq!(c2::key_id_for(&next_write_pk), next_key_id);
    let sealed = BASE64
        .decode(text(&vectors["document"]["sealed_b64"]))
        .unwrap();

    // The put of the document vector: base 6, new 7.
    let put = &statements["put"];
    c2::check_new_version(
        integer(&put["base_version"]) as u64,
        integer(&put["new_version"]) as u64,
    )
    .unwrap();
    let built = c2::put_statement(
        user_id,
        name,
        key_id,
        1,
        integer(&put["base_version"]) as u64,
        integer(&put["new_version"]) as u64,
        &sealed,
    )
    .unwrap();
    assert_eq!(c2::hex(&built), text(&put["statement_hex"]), "put");
    assert!(c2::signature_is_valid(
        &write_pk,
        &unhex(&put["signature_hex"]),
        &built
    ));

    // The create and its put (base 0, new 1), and the rotation's put (under the
    // next key): published by the blob's hash, so the statements are checked
    // field by field against the hash.
    let for_hash = |tag: &[u8], statement: &[u8], digest: &Value| {
        assert!(statement.starts_with(tag));
        assert_eq!(c2::hex(&statement[statement.len() - 32..]), text(digest));
    };
    let create = &statements["create"];
    let create_statement = unhex(&create["statement_hex"]);
    for_hash(
        c2::CREATE_TAG,
        &create_statement,
        &create["sealed_sha256_hex"],
    );
    let prefix_len = create_statement.len() - 32;
    let built_prefix = c2::create_statement(user_id, key_id, &write_pk, b"").unwrap();
    assert_eq!(
        create_statement[..prefix_len],
        built_prefix[..prefix_len],
        "create"
    );
    assert!(c2::signature_is_valid(
        &write_pk,
        &unhex(&create["signature_hex"]),
        &create_statement
    ));

    for (label, put, signer, signer_key_id, digest) in [
        (
            "create_put",
            &statements["create_put"],
            &write_pk,
            key_id,
            &create["sealed_sha256_hex"],
        ),
        (
            "rotate_put",
            &statements["rotate_put"]["put"],
            &next_write_pk,
            next_key_id,
            &statements["rotate_put"]["sealed_sha256_hex"],
        ),
    ] {
        assert_eq!(text(&put["key_id"]), signer_key_id, "{label}");
        let base = integer(&put["base_version"]) as u64;
        let new = integer(&put["new_version"]) as u64;
        if label == "create_put" {
            assert_eq!((base, new), (0, 1), "POST /v1/vault puts from 0 to 1");
        } else {
            c2::check_new_version(base, new).unwrap();
        }
        let statement = unhex(&put["statement_hex"]);
        for_hash(c2::PUT_TAG, &statement, digest);
        let built = c2::put_statement(user_id, name, signer_key_id, 1, base, new, b"").unwrap();
        let prefix_len = statement.len() - 32;
        assert_eq!(statement.len(), built.len(), "{label}");
        assert_eq!(statement[..prefix_len], built[..prefix_len], "{label}");
        assert!(
            c2::signature_is_valid(signer, &unhex(&put["signature_hex"]), &statement),
            "{label}"
        );
    }
}

/// `POST /v1/vault/rotate`: velvt-core rebuilds the rotate statement from the
/// request and compares bytes, then checks both signatures over it; the
/// record it keeps is the statement and the two signatures.
#[test]
fn the_published_rotation_is_the_statement_velvt_core_rebuilds() {
    let vectors = vectors();
    let rotate = &vectors["statements"]["rotate"];
    let user_id = text(&vectors["document"]["user_id"]);
    let write_pk = unhex(&vectors["key"]["write_pk_hex"]);
    let next_write_pk = unhex(&vectors["next_key"]["write_pk_hex"]);
    let statement = unhex(&rotate["statement_hex"]);
    let rebuilt = c2::rotate_statement(
        user_id,
        text(&vectors["key"]["key_id"]),
        text(&vectors["next_key"]["key_id"]),
        &next_write_pk,
        integer(&rotate["new_version"]) as u64,
    )
    .unwrap();
    assert_eq!(
        c2::hex(&statement),
        c2::hex(&rebuilt),
        "velvt-core refuses this rotate statement (422 vault_statement_invalid)"
    );
    let old_signature = unhex(&rotate["old_signature_hex"]);
    let new_signature = unhex(&rotate["new_signature_hex"]);
    assert!(c2::signature_is_valid(&write_pk, &old_signature, &rebuilt));
    assert!(c2::signature_is_valid(
        &next_write_pk,
        &new_signature,
        &rebuilt
    ));
    assert_eq!(
        unhex(&rotate["stored_hex"]),
        [rebuilt, old_signature, new_signature].concat()
    );
}

/// `DELETE /v1/vault`: velvt-core parses the statement at exactly §5.3's
/// length, requires `requested_at` within ten minutes of its clock in whole
/// seconds, and keeps statement ‖ signature as the tombstone.
#[test]
fn the_published_deletion_parses_at_velvt_cores_length_and_clock() {
    let vectors = vectors();
    let delete = &vectors["statements"]["delete"];
    let statement = unhex(&delete["statement_hex"]);
    let parsed = c2::parse_delete_statement(&statement)
        .expect("velvt-core refuses this delete statement (422 vault_statement_invalid)");
    assert_eq!(parsed.user_id, text(&vectors["document"]["user_id"]));
    assert_eq!(parsed.key_id, text(&vectors["key"]["key_id"]));
    assert_eq!(parsed.at_version as i64, integer(&delete["at_version"]));
    assert!(
        (parsed.requested_at - VECTORS_NOW).abs() <= c2::STATEMENT_CLOCK_MARGIN_SECONDS,
        "requested_at {} is not within ten minutes of {VECTORS_NOW} (whole seconds)",
        parsed.requested_at
    );
    let signature = unhex(&delete["signature_hex"]);
    let write_pk = unhex(&vectors["key"]["write_pk_hex"]);
    assert!(c2::signature_is_valid(&write_pk, &signature, &statement));
    assert_eq!(
        unhex(&delete["stored_hex"]),
        [statement, signature].concat()
    );
}

/// `POST /v1/vault/handoffs` and `DELETE /v1/vault/reset-requests/{id}`:
/// velvt-core builds both statements from the request; the handoff's
/// `expires_at` is the body's integer of whole seconds.
#[test]
fn the_published_handoff_and_cancel_are_the_statements_velvt_core_builds() {
    let vectors = vectors();
    let statements = &vectors["statements"];
    let user_id = text(&vectors["document"]["user_id"]);
    let write_pk = unhex(&vectors["key"]["write_pk_hex"]);

    let handoff = &statements["handoff"];
    let expires_at = integer(&handoff["expires_at"]);
    assert!(
        expires_at > VECTORS_NOW && expires_at <= VECTORS_NOW + c2::HANDOFF_MAX_TTL_SECONDS,
        "expires_at {expires_at} is not within ten minutes after {VECTORS_NOW} (whole seconds)"
    );
    let built = c2::handoff_statement(
        user_id,
        text(&vectors["setup_code"]["handoff_id_hex"]),
        &unhex(&vectors["setup_code"]["payload_hex"]),
        expires_at,
    )
    .unwrap();
    assert_eq!(c2::hex(&built), text(&handoff["statement_hex"]), "handoff");
    assert!(c2::signature_is_valid(
        &write_pk,
        &unhex(&handoff["signature_hex"]),
        &built
    ));
    assert!(unhex(&vectors["setup_code"]["payload_hex"]).len() <= c2::MAX_HANDOFF_PAYLOAD_BYTES);

    let cancel = &statements["reset_cancel"];
    let built = c2::reset_cancel_statement(
        user_id,
        text(&vectors["key"]["key_id"]),
        text(&cancel["reset_request_id"]),
    )
    .unwrap();
    assert_eq!(
        c2::hex(&built),
        text(&cancel["statement_hex"]),
        "reset_cancel"
    );
    assert!(c2::signature_is_valid(
        &write_pk,
        &unhex(&cancel["signature_hex"]),
        &built
    ));
}

// ---------------------------------------------------------------------------
// What A1 seals now
// ---------------------------------------------------------------------------

/// Every document A1 seals, at each step's smallest and largest JSON, has a
/// length velvt-core stores; a document past the largest is refused by A1
/// before velvt-core could answer `413`.
#[test]
fn every_document_a1_seals_has_a_length_velvt_core_stores() {
    assert_eq!(SEALED_SIZES.to_vec(), c2::ciphertext_bytes());
    let key = VaultKey::generate().unwrap();
    let user_id = c2_user();
    let name = BlobName::prefs();
    let context = DocumentContext {
        user_id: &user_id,
        name: &name,
        key_id: key.key_id(),
        schema: 1,
        version: 1,
    };
    let mut lengths = vec![0];
    for padded in c2::padded_document_bytes() {
        lengths.push(padded - 4);
        lengths.push(padded - 3);
    }
    lengths.pop();
    assert_eq!(*lengths.last().unwrap(), MAX_DOCUMENT_JSON_LENGTH);
    for json_length in lengths {
        let sealed = key
            .seal_document(&context, &vec![b' '; json_length])
            .unwrap();
        assert_eq!(c2::check_ciphertext(&sealed), Ok(()), "{json_length}");
        assert_eq!(Some(sealed.len()), sealed_size_for(json_length));
    }
    assert_eq!(
        key.seal_document(&context, &vec![b' '; MAX_DOCUMENT_JSON_LENGTH + 1]),
        Err(CryptoError::TooLarge)
    );
    assert_eq!(
        c2::check_ciphertext(&vec![0; c2::ciphertext_bytes()[5] + 1]),
        Err(c2::Refused::TooLarge)
    );
}

/// A vault's life as a Mac drives it with A1, each request checked as
/// velvt-core checks it: turning sync on at version 1, a write, a setup code, a
/// cancelled reset, a rotation with its blob under the new key, and a deletion.
/// The rotation record and the tombstone velvt-core keeps and hands back parse
/// and verify in A1.
#[test]
fn a1s_requests_pass_velvt_cores_checks_from_creation_to_deletion() {
    const NOW: i64 = 1_760_486_400;
    let user_id = c2_user();
    let name = BlobName::prefs();
    let json = br#"{"schema":1}"#;
    let seal = |key: &VaultKey, version: u64| {
        let context = DocumentContext {
            user_id: &user_id,
            name: &name,
            key_id: key.key_id(),
            schema: 1,
            version,
        };
        key.seal_document(&context, json).unwrap()
    };
    let key = VaultKey::generate().unwrap();
    let key_id = key.key_id().to_hex();

    // POST /v1/vault: the create statement and the put from 0 to 1.
    let ciphertext = seal(&key, 1);
    let create = Signed::sign(
        &key,
        CreateStatement {
            user_id: user_id.clone(),
            key_id: key.key_id(),
            write_pk: *key.write_public_key(),
            ciphertext_sha256: sha256(&ciphertext),
        },
    )
    .unwrap();
    let first_put = PutStatement {
        user_id: user_id.clone(),
        name: name.clone(),
        key_id: key.key_id(),
        schema: 1,
        base_version: 0,
        new_version: 1,
        ciphertext_sha256: sha256(&ciphertext),
    };
    let mut vault = c2::create_vault(
        C2_USER_ID,
        &c2::CreateRequest {
            key_id: &key_id,
            write_pk: key.write_public_key().as_bytes(),
            schema: 1,
            ciphertext: &ciphertext,
            create_sig: create.signature.as_bytes(),
            put_sig: key.sign_statement(&first_put).unwrap().as_bytes(),
        },
    )
    .unwrap();
    assert_eq!(vault.version, 1);

    // PUT /v1/vault/blobs/prefs, If-Match "1", to version 2.
    let ciphertext = seal(&key, 2);
    let put = PutStatement {
        base_version: 1,
        new_version: 2,
        ciphertext_sha256: sha256(&ciphertext),
        ..first_put
    };
    let put_sig = key.sign_statement(&put).unwrap();
    let request = c2::PutRequest {
        if_match: 1,
        key_id: &key_id,
        schema: 1,
        new_version: 2,
        ciphertext: &ciphertext,
        put_sig: put_sig.as_bytes(),
    };
    assert_eq!(c2::put_blob(&mut vault, &request), Ok(2));

    // POST /v1/vault/handoffs: expires_at is the body's whole seconds, and
    // what the statement signs.
    let code = SetupCode::generate().unwrap();
    let issuer = SyncDeviceId::parse("5e5e5e5e-5e5e-4e5e-9e5e-5e5e5e5e5e5e").unwrap();
    let payload = seal_handoff(&code, &user_id, &key, issuer).unwrap();
    let handoff = Signed::sign(
        &key,
        HandoffStatement {
            user_id: user_id.clone(),
            handoff_id: code.handoff_id().unwrap(),
            payload_sha256: sha256(&payload),
            expires_at: NOW + 600,
        },
    )
    .unwrap();
    let handoff_id = code.handoff_id().unwrap().to_hex();
    let request = c2::HandoffRequest {
        handoff_id: &handoff_id,
        payload: &payload,
        expires_at: handoff.statement.expires_at,
        sig: handoff.signature.as_bytes(),
    };
    assert_eq!(c2::create_handoff(&vault, &request, NOW), Ok(NOW + 600));
    assert_eq!(
        c2::handoff_statement(C2_USER_ID, &handoff_id, &payload, NOW + 600).unwrap(),
        handoff.statement.to_bytes()
    );

    // DELETE /v1/vault/reset-requests/{id}.
    let reset_id = Uuid::new_v4();
    let cancel = Signed::sign(
        &key,
        ResetCancelStatement {
            user_id: user_id.clone(),
            key_id: key.key_id(),
            reset_request_id: reset_id,
        },
    )
    .unwrap();
    let request = c2::ResetCancelRequest {
        reset_request_id: &reset_id.to_string(),
        cancel_stmt: &cancel.statement.to_bytes(),
        sig: cancel.signature.as_bytes(),
    };
    assert_eq!(c2::cancel_reset(&vault, &request), Ok(()));

    // POST /v1/vault/rotate, If-Match "2", to version 3 under the next key.
    let next = VaultKey::generate().unwrap();
    let next_key_id = next.key_id().to_hex();
    let rotation = SignedRotation::sign(user_id.clone(), &key, &next, 3).unwrap();
    let ciphertext = seal(&next, 3);
    let rotate_put = PutStatement {
        key_id: next.key_id(),
        base_version: 2,
        new_version: 3,
        ciphertext_sha256: sha256(&ciphertext),
        ..put
    };
    let rotate_put_sig = next.sign_statement(&rotate_put).unwrap();
    let rotate_stmt = rotation.statement.to_bytes();
    let rotate_request = |rotate_stmt| c2::RotateRequest {
        if_match: 2,
        old_key_id: &key_id,
        new_key_id: &next_key_id,
        new_write_pk: next.write_public_key().as_bytes(),
        rotate_stmt,
        old_sig: rotation.old_signature.as_bytes(),
        new_sig: rotation.new_signature.as_bytes(),
        schema: 1,
        new_version: 3,
        ciphertext: &ciphertext,
        put_sig: rotate_put_sig.as_bytes(),
    };
    // The layout A1 first shipped, with a 16-byte issuer tag after §5.3's
    // statement, is refused by both sides (design §17.1 D13).
    let tagged = [rotate_stmt.as_slice(), &[0x5e; 16]].concat();
    assert_eq!(
        c2::rotate_vault(&mut vault.clone(), &rotate_request(&tagged)),
        Err(c2::Refused::StatementInvalid)
    );
    assert!(RotateStatement::parse(&tagged).is_err());
    let record = c2::rotate_vault(&mut vault, &rotate_request(&rotate_stmt)).unwrap();
    assert_eq!(record, rotation.to_bytes());
    assert!(record.len() <= 512, "design §4.2's rotations column");
    let handed_back = SignedRotation::parse(&record).unwrap();
    assert_eq!(
        handed_back.verify(&user_id, key.write_public_key()),
        Ok(*next.write_public_key())
    );
    assert_eq!(
        (vault.key_id.as_str(), vault.version),
        (next_key_id.as_str(), 3)
    );

    // DELETE /v1/vault at version 3: requested_at in whole seconds, within
    // ten minutes of velvt-core's clock.
    let delete_at = |requested_at| {
        Signed::sign(
            &next,
            DeleteStatement {
                user_id: user_id.clone(),
                key_id: next.key_id(),
                at_version: 3,
                requested_at,
            },
        )
        .unwrap()
    };
    let delete = delete_at(NOW);
    let delete_stmt = delete.statement.to_bytes();
    let request = c2::DeleteRequest {
        key_id: &next_key_id,
        delete_stmt: &delete_stmt,
        sig: delete.signature.as_bytes(),
    };
    for server_now in [NOW - 600, NOW + 600] {
        assert!(c2::delete_vault(&vault, &request, server_now).is_ok());
    }
    for server_now in [NOW - 601, NOW + 601] {
        assert_eq!(
            c2::delete_vault(&vault, &request, server_now),
            Err(c2::Refused::StatementInvalid)
        );
    }
    let in_milliseconds = delete_at(NOW * 1000);
    let in_milliseconds_stmt = in_milliseconds.statement.to_bytes();
    let refused = c2::DeleteRequest {
        key_id: &next_key_id,
        delete_stmt: &in_milliseconds_stmt,
        sig: in_milliseconds.signature.as_bytes(),
    };
    assert_eq!(
        c2::delete_vault(&vault, &refused, NOW),
        Err(c2::Refused::StatementInvalid)
    );
    let tombstone = c2::delete_vault(&vault, &request, NOW).unwrap();
    assert_eq!(tombstone, delete.to_bytes());
    assert!(tombstone.len() <= 256, "design §4.2's tombstone column");
    let handed_back = Signed::<DeleteStatement>::parse(&tombstone).unwrap();
    handed_back
        .verify(&user_id, next.write_public_key())
        .unwrap();
    assert_eq!(handed_back.statement.requested_at, NOW);
}
