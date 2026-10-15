//! velvt-core's `/v1/vault` routes and `GET /v1/devices`, as C2 (#46) and C1
//! (#45) answer them (design §8.3).
//!
//! Every request goes through the `HttpClient` the engine is given: in the
//! service that is `AuthManager`, so each one carries this device's token and
//! is written to the egress ledger before it is sent. Nothing here judges a
//! response: it parses what the server said, strictly, and `verify` decides
//! whether to believe it.
//!
//! Error bodies are velvt-core's envelope, `{"error": {"code", "message",
//! "details"?}}`. A `404 vault_not_found` carries the reset and tombstone
//! fields in `details` (§6.1, F5); `412 vault_version_conflict` carries
//! `current_version`.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::DateTime;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{AuthError, HttpClient, HttpRequest, HttpResponse, Precondition};
use crate::vault::crypto::{KeyId, Signature, WritePublicKey};

pub const VAULT_PATH: &str = "/v1/vault";
pub const PREFS_BLOB_PATH: &str = "/v1/vault/blobs/prefs";
pub const DEVICES_PATH: &str = "/v1/devices";

/// Why a request got no answer this Mac can use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("velvt-core could not be reached")]
    Transport,
    #[error("velvt-core asked this Mac to wait")]
    RateLimited { retry_after: Option<String> },
    /// `404 feature_disabled`: `VELVT_VAULT_ENABLED` is off. Sync pauses
    /// silently (§14).
    #[error("the vault is turned off on this server")]
    FeatureDisabled,
    #[error("this device was removed from the account")]
    DeviceRevoked,
    #[error("the account was deleted")]
    AccountDeleted,
    #[error("the session needs a sign-in")]
    NeedsReauth,
    #[error("velvt-core answered {status} ({code:?})")]
    Unexpected { status: u16, code: Option<String> },
    /// A body that is not what C2 sends. Named by field, never by content.
    #[error("velvt-core's answer is malformed: {0}")]
    Malformed(&'static str),
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::DeviceRevoked => Self::DeviceRevoked,
            AuthError::AccountDeleted => Self::AccountDeleted,
            AuthError::NeedsReauth | AuthError::Transition(_) => Self::NeedsReauth,
            AuthError::RateLimited { retry_after } => Self::RateLimited { retry_after },
            AuthError::Transport | AuthError::InvalidResponse | AuthError::TokenStore(_) => {
                Self::Transport
            }
        }
    }
}

impl ApiError {
    /// The code stored in `vault_state.last_error`: a fixed token.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::RateLimited { .. } => "rate_limited",
            Self::FeatureDisabled => "feature_disabled",
            Self::DeviceRevoked => "device_revoked",
            Self::AccountDeleted => "account_deleted",
            Self::NeedsReauth => "needs_reauth",
            Self::Unexpected { .. } => "server_unexpected",
            Self::Malformed(_) => "server_malformed",
        }
    }
}

/// A reset request as the server describes it. Times are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerReset {
    pub id: String,
    pub key_id: KeyId,
    pub requested_at_ms: i64,
    pub effective_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
}

/// A signed deletion the server kept: the delete statement and its signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTombstone {
    pub key_id: KeyId,
    pub deleted_at_ms: i64,
    pub statement: Vec<u8>,
}

/// The fields §6.1 reads on both a 200 and a 404.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VaultFacts {
    pub pending_reset: Option<ServerReset>,
    pub completed_reset: Option<ServerReset>,
    pub tombstone: Option<ServerTombstone>,
}

/// `GET /v1/vault`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultView {
    Present {
        key_id: KeyId,
        write_pk: WritePublicKey,
        created_at_ms: i64,
        /// Each stored rotation, oldest first: statement ‖ old signature ‖
        /// new signature.
        rotations: Vec<Vec<u8>>,
        facts: VaultFacts,
    },
    Missing {
        facts: VaultFacts,
    },
}

impl VaultView {
    pub fn facts(&self) -> &VaultFacts {
        match self {
            Self::Present { facts, .. } | Self::Missing { facts } => facts,
        }
    }
}

/// `GET /v1/vault/blobs/prefs`, 200.
#[derive(Clone, PartialEq, Eq)]
pub struct ServedBlob {
    pub version: u64,
    pub base_version: u64,
    pub key_id: KeyId,
    pub schema: u16,
    pub ciphertext: Vec<u8>,
    pub put_signature: Signature,
}

impl std::fmt::Debug for ServedBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServedBlob")
            .field("version", &self.version)
            .field("base_version", &self.base_version)
            .field("key_id", &self.key_id)
            .field("schema", &self.schema)
            .field("ciphertext_len", &self.ciphertext.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobFetch {
    /// `304`: the copy this Mac holds is current.
    NotModified,
    Blob(ServedBlob),
    /// `404 vault_not_found`: the vault went since `GET /v1/vault`.
    VaultMissing(VaultFacts),
    /// `404 vault_blob_not_found`.
    BlobMissing,
}

/// What `PUT /v1/vault/blobs/prefs` and `POST /v1/vault` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Stored {
        version: u64,
    },
    /// `412 vault_version_conflict`: someone wrote first.
    Conflict {
        current_version: u64,
    },
    /// `409 vault_key_mismatch`: the vault's key changed underneath.
    KeyMismatch,
    /// `409 vault_exists`: another Mac created the vault first.
    Exists,
    /// `409 vault_reset_pending`.
    ResetPending,
    /// `403 vault_write_unauthorized`: the server does not take this key's
    /// signature.
    Unauthorized,
    VaultMissing(VaultFacts),
    /// `413`, or a `422` the server explains with a code.
    Refused {
        status: u16,
        code: Option<String>,
    },
}

/// One row of `GET /v1/devices`: server facts only, never a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDevice {
    pub id: String,
    pub active: bool,
    pub created_at_ms: i64,
    pub last_seen_at_ms: Option<i64>,
    pub client_version: String,
    pub is_current: bool,
}

/// The body of `PUT /v1/vault/blobs/prefs`.
#[derive(Debug, Clone)]
pub struct PutBody<'a> {
    pub key_id: KeyId,
    pub schema: u16,
    pub new_version: u64,
    pub ciphertext: &'a [u8],
    pub put_signature: Signature,
}

/// The body of `POST /v1/vault`.
#[derive(Debug, Clone)]
pub struct CreateBody<'a> {
    pub key_id: KeyId,
    pub write_pk: WritePublicKey,
    pub schema: u16,
    pub ciphertext: &'a [u8],
    pub create_signature: Signature,
    pub put_signature: Signature,
}

/// The vault routes, over the client the engine was given.
#[derive(Clone)]
pub struct VaultApi {
    http: Arc<dyn HttpClient>,
}

/// `If-Match: "<n>"` and `If-None-Match: "<n>"`, as velvt-core's `etag()`
/// writes a version.
pub fn entity_tag(version: u64) -> String {
    format!("\"{version}\"")
}

impl VaultApi {
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ApiError> {
        let response = self.http.send(request).await?;
        if response.status == 429 {
            return Err(ApiError::RateLimited {
                retry_after: response.retry_after,
            });
        }
        if response.status == 404 && response.error_code.as_deref() == Some("feature_disabled") {
            return Err(ApiError::FeatureDisabled);
        }
        Ok(response)
    }

    pub async fn get_vault(&self) -> Result<VaultView, ApiError> {
        let response = self.send(HttpRequest::get(VAULT_PATH)).await?;
        match (response.status, response.error_code.as_deref()) {
            (200, _) => parse_vault(body(&response)?),
            (404, Some("vault_not_found")) => Ok(VaultView::Missing {
                facts: error_facts(&response)?,
            }),
            (status, code) => Err(unexpected(status, code)),
        }
    }

    pub async fn get_blob(&self, if_none_match: Option<u64>) -> Result<BlobFetch, ApiError> {
        let mut request = HttpRequest::get(PREFS_BLOB_PATH);
        if let Some(version) = if_none_match {
            request = request.with_precondition(Precondition::IfNoneMatch(entity_tag(version)));
        }
        let response = self.send(request).await?;
        match (response.status, response.error_code.as_deref()) {
            (200, _) => Ok(BlobFetch::Blob(parse_blob(body(&response)?)?)),
            (304, _) => Ok(BlobFetch::NotModified),
            (404, Some("vault_not_found")) => Ok(BlobFetch::VaultMissing(error_facts(&response)?)),
            (404, Some("vault_blob_not_found")) => Ok(BlobFetch::BlobMissing),
            (status, code) => Err(unexpected(status, code)),
        }
    }

    pub async fn put_blob(
        &self,
        base_version: u64,
        put: &PutBody<'_>,
    ) -> Result<WriteOutcome, ApiError> {
        let request = HttpRequest::put(
            PREFS_BLOB_PATH,
            json!({
                "key_id": put.key_id.to_hex(),
                "schema": put.schema,
                "new_version": put.new_version,
                "ciphertext_b64": BASE64.encode(put.ciphertext),
                "put_sig_b64": put.put_signature.to_base64(),
            }),
        )
        .with_precondition(Precondition::IfMatch(entity_tag(base_version)));
        let response = self.send(request).await?;
        write_outcome(&response)
    }

    pub async fn create_vault(&self, create: &CreateBody<'_>) -> Result<WriteOutcome, ApiError> {
        let mut request = HttpRequest::post(VAULT_PATH);
        request.json_body = Some(json!({
            "key_id": create.key_id.to_hex(),
            "write_pk_b64": create.write_pk.to_base64(),
            "blob": {
                "name": "prefs",
                "schema": create.schema,
                "ciphertext_b64": BASE64.encode(create.ciphertext),
            },
            "create_sig_b64": create.create_signature.to_base64(),
            "put_sig_b64": create.put_signature.to_base64(),
        }));
        let response = self.send(request).await?;
        if response.status == 201 {
            return Ok(WriteOutcome::Stored { version: 1 });
        }
        write_outcome(&response)
    }

    pub async fn list_devices(&self) -> Result<Vec<ServerDevice>, ApiError> {
        let response = self.send(HttpRequest::get(DEVICES_PATH)).await?;
        if response.status != 200 {
            return Err(unexpected(response.status, response.error_code.as_deref()));
        }
        parse_devices(body(&response)?)
    }
}

fn unexpected(status: u16, code: Option<&str>) -> ApiError {
    ApiError::Unexpected {
        status,
        code: code.map(str::to_owned),
    }
}

fn body(response: &HttpResponse) -> Result<&Value, ApiError> {
    response
        .raw_body
        .as_ref()
        .ok_or(ApiError::Malformed("body"))
}

fn write_outcome(response: &HttpResponse) -> Result<WriteOutcome, ApiError> {
    match (response.status, response.error_code.as_deref()) {
        (200 | 201, _) => {
            let version = body(response)?
                .get("version")
                .and_then(Value::as_u64)
                .ok_or(ApiError::Malformed("version"))?;
            Ok(WriteOutcome::Stored { version })
        }
        (412, Some("vault_version_conflict")) => {
            let current_version = details(response)
                .and_then(|details| details.get("current_version"))
                .and_then(Value::as_u64)
                .ok_or(ApiError::Malformed("current_version"))?;
            Ok(WriteOutcome::Conflict { current_version })
        }
        (409, Some("vault_key_mismatch")) => Ok(WriteOutcome::KeyMismatch),
        (409, Some("vault_exists")) => Ok(WriteOutcome::Exists),
        (409, Some("vault_reset_pending")) => Ok(WriteOutcome::ResetPending),
        (403, Some("vault_write_unauthorized")) => Ok(WriteOutcome::Unauthorized),
        (404, Some("vault_not_found")) => Ok(WriteOutcome::VaultMissing(error_facts(response)?)),
        (status @ (413 | 422), code) => Ok(WriteOutcome::Refused {
            status,
            code: code.map(str::to_owned),
        }),
        (status, code) => Err(unexpected(status, code)),
    }
}

fn details(response: &HttpResponse) -> Option<&Value> {
    response.raw_body.as_ref()?.get("error")?.get("details")
}

fn error_facts(response: &HttpResponse) -> Result<VaultFacts, ApiError> {
    match details(response) {
        Some(details) => parse_facts(details),
        None => Ok(VaultFacts::default()),
    }
}

/// An RFC 3339 time, as pydantic writes a `datetime`, in Unix milliseconds.
fn parse_time(value: &str, field: &'static str) -> Result<i64, ApiError> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp_millis())
        .map_err(|_| ApiError::Malformed(field))
}

fn parse_key_id(value: &str, field: &'static str) -> Result<KeyId, ApiError> {
    KeyId::from_hex(value).ok_or(ApiError::Malformed(field))
}

fn decode_base64(value: &str, field: &'static str) -> Result<Vec<u8>, ApiError> {
    BASE64
        .decode(value.as_bytes())
        .map_err(|_| ApiError::Malformed(field))
}

#[derive(Deserialize)]
struct WireReset {
    id: String,
    key_id: String,
    requested_at: String,
    effective_at: String,
    #[serde(default)]
    resolved_at: Option<String>,
}

#[derive(Deserialize)]
struct WireTombstone {
    key_id: String,
    deleted_at: String,
    statement_b64: String,
}

#[derive(Deserialize)]
struct WireFacts {
    #[serde(default)]
    pending_reset: Option<WireReset>,
    #[serde(default)]
    completed_reset: Option<WireReset>,
    #[serde(default)]
    tombstone: Option<WireTombstone>,
}

#[derive(Deserialize)]
struct WireVault {
    key_id: String,
    write_pk_b64: String,
    created_at: String,
    rotations_b64: Vec<String>,
    #[serde(flatten)]
    facts: WireFacts,
}

#[derive(Deserialize)]
struct WireBlob {
    version: u64,
    key_id: String,
    schema: u16,
    ciphertext_b64: String,
    put_sig_b64: String,
    base_version: u64,
}

#[derive(Deserialize)]
struct WireDevice {
    id: String,
    status: String,
    created_at: String,
    #[serde(default)]
    last_seen_at: Option<String>,
    client_version: String,
    is_current: bool,
}

fn reset(wire: WireReset) -> Result<ServerReset, ApiError> {
    if uuid::Uuid::try_parse(&wire.id).is_err() {
        return Err(ApiError::Malformed("reset id"));
    }
    Ok(ServerReset {
        id: wire.id.to_ascii_lowercase(),
        key_id: parse_key_id(&wire.key_id, "reset key_id")?,
        requested_at_ms: parse_time(&wire.requested_at, "requested_at")?,
        effective_at_ms: parse_time(&wire.effective_at, "effective_at")?,
        resolved_at_ms: wire
            .resolved_at
            .as_deref()
            .map(|value| parse_time(value, "resolved_at"))
            .transpose()?,
    })
}

fn facts(wire: WireFacts) -> Result<VaultFacts, ApiError> {
    Ok(VaultFacts {
        pending_reset: wire.pending_reset.map(reset).transpose()?,
        completed_reset: wire.completed_reset.map(reset).transpose()?,
        tombstone: wire
            .tombstone
            .map(|tombstone| {
                Ok::<_, ApiError>(ServerTombstone {
                    key_id: parse_key_id(&tombstone.key_id, "tombstone key_id")?,
                    deleted_at_ms: parse_time(&tombstone.deleted_at, "deleted_at")?,
                    statement: decode_base64(&tombstone.statement_b64, "statement_b64")?,
                })
            })
            .transpose()?,
    })
}

pub(crate) fn parse_facts(value: &Value) -> Result<VaultFacts, ApiError> {
    facts(WireFacts::deserialize(value).map_err(|_| ApiError::Malformed("vault facts"))?)
}

pub(crate) fn parse_vault(value: &Value) -> Result<VaultView, ApiError> {
    let wire = WireVault::deserialize(value).map_err(|_| ApiError::Malformed("vault"))?;
    Ok(VaultView::Present {
        key_id: parse_key_id(&wire.key_id, "key_id")?,
        write_pk: WritePublicKey::from_base64(&wire.write_pk_b64)
            .map_err(|_| ApiError::Malformed("write_pk_b64"))?,
        created_at_ms: parse_time(&wire.created_at, "created_at")?,
        rotations: wire
            .rotations_b64
            .iter()
            .map(|rotation| decode_base64(rotation, "rotations_b64"))
            .collect::<Result<_, _>>()?,
        facts: facts(wire.facts)?,
    })
}

pub(crate) fn parse_blob(value: &Value) -> Result<ServedBlob, ApiError> {
    let wire = WireBlob::deserialize(value).map_err(|_| ApiError::Malformed("blob"))?;
    Ok(ServedBlob {
        version: wire.version,
        base_version: wire.base_version,
        key_id: parse_key_id(&wire.key_id, "blob key_id")?,
        schema: wire.schema,
        ciphertext: decode_base64(&wire.ciphertext_b64, "ciphertext_b64")?,
        put_signature: Signature::from_base64(&wire.put_sig_b64)
            .map_err(|_| ApiError::Malformed("put_sig_b64"))?,
    })
}

pub(crate) fn parse_devices(value: &Value) -> Result<Vec<ServerDevice>, ApiError> {
    let wire = Vec::<WireDevice>::deserialize(value).map_err(|_| ApiError::Malformed("devices"))?;
    wire.into_iter()
        .map(|device| {
            Ok(ServerDevice {
                active: device.status == "active",
                created_at_ms: parse_time(&device.created_at, "device created_at")?,
                last_seen_at_ms: device
                    .last_seen_at
                    .as_deref()
                    .map(|value| parse_time(value, "last_seen_at"))
                    .transpose()?,
                id: device.id,
                client_version: device.client_version,
                is_current: device.is_current,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_blob, parse_devices, parse_facts, parse_vault, ApiError, VaultView};
    use crate::vault::test_support::test_key;

    /// The bodies below are C2's own response models as pydantic serializes
    /// them (`app/schemas/vault.py`, `DeviceListItem`).
    #[test]
    fn a_vault_body_parses_with_its_reset_and_tombstone_fields() {
        let key = test_key();
        let body = json!({
            "key_id": key.key_id().to_hex(),
            "write_pk_b64": key.write_public_key().to_base64(),
            "created_at": "2025-10-15T10:42:00Z",
            "rotations_b64": [],
            "pending_reset": {
                "id": "6F9619FF-8B86-D011-B42D-00C04FC964FF",
                "key_id": key.key_id().to_hex(),
                "requested_at": "2025-10-15T10:42:00.250000Z",
                "effective_at": "2025-10-18T10:42:00.250000+00:00"
            }
        });
        let VaultView::Present {
            key_id,
            created_at_ms,
            facts,
            ..
        } = parse_vault(&body).unwrap()
        else {
            panic!("present");
        };
        assert_eq!(key_id, key.key_id());
        assert_eq!(created_at_ms, 1_760_524_920_000);
        let pending = facts.pending_reset.unwrap();
        assert_eq!(pending.id, "6f9619ff-8b86-d011-b42d-00c04fc964ff");
        assert_eq!(
            pending.effective_at_ms - pending.requested_at_ms,
            72 * 3_600_000
        );
        assert!(facts.completed_reset.is_none() && facts.tombstone.is_none());
    }

    #[test]
    fn a_body_that_is_not_c2_s_is_refused_by_field_name() {
        let key = test_key();
        let wrong_key = json!({
            "key_id": "NOT-HEX",
            "write_pk_b64": key.write_public_key().to_base64(),
            "created_at": "2026-10-15T10:42:00Z",
            "rotations_b64": [],
        });
        assert_eq!(parse_vault(&wrong_key), Err(ApiError::Malformed("key_id")));
        assert_eq!(
            parse_facts(
                &json!({"tombstone": {"key_id": key.key_id().to_hex(), "deleted_at": "yesterday", "statement_b64": ""}})
            ),
            Err(ApiError::Malformed("deleted_at"))
        );
        assert_eq!(
            parse_blob(&json!({"version": 1})).err(),
            Some(ApiError::Malformed("blob"))
        );
    }

    #[test]
    fn the_device_list_keeps_server_facts_only() {
        let devices = parse_devices(&json!([
            {"id": "8f14e45f-ceea-467a-9575-1f2ab6c9d0e1", "status": "active",
             "created_at": "2025-10-01T00:00:00Z", "last_seen_at": null,
             "client_version": "1.0.14", "is_current": true},
            {"id": "45c48cce-2e2d-4fbd-b6a0-3f0bd1b6f5e2", "status": "revoked",
             "created_at": "2025-09-01T00:00:00Z", "last_seen_at": "2025-10-01T00:00:00Z",
             "client_version": "1.0.13", "is_current": false}
        ]))
        .unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices[0].active && devices[0].is_current);
        assert!(!devices[1].active);
        assert_eq!(devices[1].last_seen_at_ms, Some(1_759_276_800_000));
    }
}
