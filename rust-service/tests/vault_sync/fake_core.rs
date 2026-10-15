//! An in-memory velvt-core that answers `/v1/vault` and `GET /v1/devices` as
//! C2 (#46) and C1 (#45) do, and that a test can make lie.
//!
//! Honest by default, and as strict as C2: every write's signature is checked
//! against the stored write key over §5.3's statement, `If-Match` names the
//! version replaced, `new_version` must lie in `(n, n + 2^32]`, ciphertext
//! must be one of the six sealed sizes, a rotation must be signed by the old
//! key and the new one, a reset waits 72 hours and completes on the next
//! request after it is due. The response bodies are C2's: the 404 carries the
//! reset and tombstone fields in `error.details`, and so on.
//!
//! Every account and every device share one clock, the test's.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use velvt_service::auth::{
    AuthError, HttpClient, HttpMethod, HttpRequest, HttpResponse, Precondition,
};
use velvt_service::vault::crypto::statement::{
    CreateStatement, DeleteStatement, PutStatement, ResetCancelStatement, RotateStatement, Signed,
    SignedRotation, Statement,
};
use velvt_service::vault::crypto::{
    sha256, BlobName, KeyId, Signature, UserId, WritePublicKey, SEALED_SIZES,
};
use velvt_service::vault::sync::SyncClock;

pub const RESET_DELAY_MS: i64 = 72 * 3_600_000;

/// The test's clock: the server's and every Mac's.
#[derive(Clone, Default)]
pub struct Clock(Arc<AtomicI64>);

impl Clock {
    pub fn at(ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(ms)))
    }

    pub fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl SyncClock for Clock {
    fn now_ms(&self) -> i64 {
        self.now()
    }
}

fn time(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[derive(Clone, Debug)]
pub struct StoredVault {
    pub key_id: KeyId,
    pub write_pk: WritePublicKey,
    pub created_at_ms: i64,
    pub rotations: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct StoredBlob {
    pub version: u64,
    pub base_version: u64,
    pub schema: u16,
    pub ciphertext: Vec<u8>,
    pub put_sig: Signature,
}

#[derive(Clone, Debug)]
pub struct StoredReset {
    pub id: String,
    pub key_id: KeyId,
    pub status: &'static str,
    pub requested_at_ms: i64,
    pub effective_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct StoredDevice {
    pub id: String,
    pub active: bool,
    pub created_at_ms: i64,
    pub last_seen_at_ms: Option<i64>,
}

#[derive(Default)]
pub struct Account {
    pub vault: Option<StoredVault>,
    pub blob: Option<StoredBlob>,
    pub tombstone: Option<(KeyId, i64, Vec<u8>)>,
    pub resets: Vec<StoredReset>,
    pub devices: Vec<StoredDevice>,
    pub deleted: bool,
    pub last_removal_ms: Option<i64>,
}

/// One request as the server received it.
#[derive(Clone, Debug)]
pub struct Received {
    pub user: String,
    pub device: String,
    pub method: &'static str,
    pub path: String,
    pub body: Option<Value>,
}

/// A response the test puts in place of the honest one.
pub type Lie = Box<dyn Fn(&Received, &Account) -> Option<HttpResponse> + Send>;

#[derive(Default)]
struct State {
    accounts: BTreeMap<String, Account>,
    received: Vec<Received>,
    enabled: bool,
    lie: Option<Lie>,
}

#[derive(Clone)]
pub struct FakeCore {
    state: Arc<Mutex<State>>,
    clock: Clock,
}

pub fn response(status: u16, body: Value) -> HttpResponse {
    let error_code = body
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    HttpResponse {
        status,
        error_code,
        tokens: None,
        retry_after: None,
        message: None,
        raw_body: Some(body),
        user_id: None,
        device_id: None,
    }
}

pub fn error(status: u16, code: &str, details: Option<Value>) -> HttpResponse {
    let mut error = json!({"code": code, "message": code});
    if let Some(details) = details {
        error["details"] = details;
    }
    response(status, json!({ "error": error }))
}

fn not_modified() -> HttpResponse {
    HttpResponse {
        raw_body: None,
        ..response(304, Value::Null)
    }
}

fn b64(field: &Value) -> Option<Vec<u8>> {
    BASE64.decode(field.as_str()?.as_bytes()).ok()
}

fn tag(precondition: &Option<Precondition>, if_match: bool) -> Option<u64> {
    let value = match (precondition, if_match) {
        (Some(Precondition::IfMatch(tag)), true)
        | (Some(Precondition::IfNoneMatch(tag)), false) => tag,
        _ => return None,
    };
    value.trim().trim_matches('"').parse().ok()
}

impl FakeCore {
    pub fn new(clock: Clock) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                enabled: true,
                ..State::default()
            })),
            clock,
        }
    }

    /// `VELVT_VAULT_ENABLED`.
    pub fn set_enabled(&self, enabled: bool) {
        self.state.lock().unwrap().enabled = enabled;
    }

    /// Registers a signed-in device and returns the client it sends with.
    pub fn sign_in(&self, user: &str, device: &str) -> Arc<FakeClient> {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        let account = state.accounts.entry(user.to_owned()).or_default();
        if !account.devices.iter().any(|known| known.id == device) {
            account.devices.push(StoredDevice {
                id: device.to_owned(),
                active: true,
                created_at_ms: now,
                last_seen_at_ms: Some(now),
            });
        }
        Arc::new(FakeClient {
            core: self.clone(),
            user: user.to_owned(),
            device: device.to_owned(),
        })
    }

    /// Another device removes `device` (C1 `DELETE /v1/devices/{id}`): it is
    /// revoked, and a pending reset moves to 72 hours from now.
    pub fn remove_device(&self, user: &str, device: &str) {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        let account = state.accounts.get_mut(user).unwrap();
        for known in &mut account.devices {
            if known.id == device {
                known.active = false;
            }
        }
        account.last_removal_ms = Some(now);
        for reset in &mut account.resets {
            if reset.status == "pending" && reset.effective_at_ms < now + RESET_DELAY_MS {
                reset.effective_at_ms = now + RESET_DELAY_MS;
            }
        }
    }

    /// `DELETE /v1/account`, from anywhere.
    pub fn delete_account(&self, user: &str) {
        let mut state = self.state.lock().unwrap();
        let account = state.accounts.entry(user.to_owned()).or_default();
        *account = Account {
            deleted: true,
            ..Account::default()
        };
    }

    /// `POST /v1/vault/reset-requests`, by someone with the password.
    pub fn request_reset(&self, user: &str) -> String {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        let account = state.accounts.get_mut(user).unwrap();
        let key_id = account.vault.as_ref().expect("a vault").key_id;
        let id = uuid::Uuid::new_v4().to_string();
        let from = account
            .last_removal_ms
            .map_or(now, |removal| removal.max(now));
        account.resets.push(StoredReset {
            id: id.clone(),
            key_id,
            status: "pending",
            requested_at_ms: now,
            effective_at_ms: from + RESET_DELAY_MS,
            resolved_at_ms: None,
        });
        id
    }

    /// Puts a lie in front of every route until cleared.
    pub fn lie(&self, lie: impl Fn(&Received, &Account) -> Option<HttpResponse> + Send + 'static) {
        self.state.lock().unwrap().lie = Some(Box::new(lie));
    }

    pub fn tell_the_truth(&self) {
        self.state.lock().unwrap().lie = None;
    }

    /// Edits the stored state directly: a database restore, a swapped blob.
    pub fn with_account<T>(&self, user: &str, edit: impl FnOnce(&mut Account) -> T) -> T {
        let mut state = self.state.lock().unwrap();
        edit(state.accounts.entry(user.to_owned()).or_default())
    }

    pub fn received(&self) -> Vec<Received> {
        self.state.lock().unwrap().received.clone()
    }

    pub fn clear_received(&self) {
        self.state.lock().unwrap().received.clear();
    }

    fn handle(
        &self,
        client: &FakeClient,
        request: &HttpRequest,
    ) -> Result<HttpResponse, AuthError> {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap();
        let received = Received {
            user: client.user.clone(),
            device: client.device.clone(),
            method: request.method.as_str(),
            path: request.path.clone(),
            body: request.json_body.clone(),
        };
        state.received.push(received.clone());
        let enabled = state.enabled;
        let account = state.accounts.entry(client.user.clone()).or_default();
        // What `AuthManager` turns these answers into.
        if account.deleted {
            return Err(AuthError::AccountDeleted);
        }
        match account
            .devices
            .iter_mut()
            .find(|device| device.id == client.device)
        {
            Some(device) if !device.active => return Err(AuthError::DeviceRevoked),
            Some(device) => device.last_seen_at_ms = Some(now),
            None => return Err(AuthError::NeedsReauth),
        }
        let account = state.accounts.get(&client.user).unwrap();
        if let Some(lie) = &state.lie {
            if let Some(answer) = lie(&received, account) {
                return Ok(answer);
            }
        }
        if request.path.starts_with("/v1/vault") && !enabled {
            return Ok(error(404, "feature_disabled", None));
        }
        let account = state.accounts.get_mut(&client.user).unwrap();
        complete_due_reset(account, now);
        let user = UserId::new(&client.user).unwrap();
        Ok(match (request.method, request.path.as_str()) {
            (HttpMethod::Get, "/v1/vault") => get_vault(account, now),
            (HttpMethod::Post, "/v1/vault") => create_vault(account, &user, request, now),
            (HttpMethod::Get, "/v1/vault/blobs/prefs") => get_blob(account, request, now),
            (HttpMethod::Put, "/v1/vault/blobs/prefs") => put_blob(account, &user, request, now),
            (HttpMethod::Post, "/v1/vault/rotate") => rotate(account, &user, request, now),
            (HttpMethod::Delete, "/v1/vault") => delete_vault(account, &user, request, now),
            (HttpMethod::Delete, path) if path.starts_with("/v1/vault/reset-requests/") => {
                cancel_reset(
                    account,
                    &user,
                    request,
                    &path["/v1/vault/reset-requests/".len()..],
                    now,
                )
            }
            (HttpMethod::Get, "/v1/devices") => list_devices(account, &client.device),
            _ => error(404, "not_found", None),
        })
    }
}

fn complete_due_reset(account: &mut Account, now: i64) {
    let due = account
        .resets
        .iter()
        .position(|reset| reset.status == "pending" && reset.effective_at_ms <= now);
    if let Some(index) = due {
        if let Some(vault) = &account.vault {
            account.resets[index].key_id = vault.key_id;
        }
        account.resets[index].status = "completed";
        account.resets[index].resolved_at_ms = Some(now);
        account.vault = None;
        account.blob = None;
    }
}

fn reset_view(reset: &StoredReset) -> Value {
    let mut view = json!({
        "id": reset.id,
        "key_id": reset.key_id.to_hex(),
        "requested_at": time(reset.requested_at_ms),
        "effective_at": time(reset.effective_at_ms),
    });
    if let Some(resolved) = reset.resolved_at_ms {
        view["resolved_at"] = json!(time(resolved));
    }
    view
}

/// `_state_views`: the pending reset, the latest completed one (30 days) and
/// the tombstone (30 days).
fn facts(account: &Account, now: i64) -> serde_json::Map<String, Value> {
    let mut facts = serde_json::Map::new();
    if let Some(pending) = account
        .resets
        .iter()
        .find(|reset| reset.status == "pending")
    {
        facts.insert("pending_reset".into(), reset_view(pending));
    }
    if let Some(completed) = account
        .resets
        .iter()
        .filter(|reset| {
            reset.status == "completed"
                && reset
                    .resolved_at_ms
                    .is_some_and(|at| now - at <= 30 * 86_400_000)
        })
        .max_by_key(|reset| reset.resolved_at_ms)
    {
        facts.insert("completed_reset".into(), reset_view(completed));
    }
    if let Some((key_id, deleted_at, statement)) = &account.tombstone {
        if now - deleted_at <= 30 * 86_400_000 {
            facts.insert(
                "tombstone".into(),
                json!({
                    "key_id": key_id.to_hex(),
                    "deleted_at": time(*deleted_at),
                    "statement_b64": BASE64.encode(statement),
                }),
            );
        }
    }
    facts
}

fn vault_not_found(account: &Account, now: i64) -> HttpResponse {
    error(
        404,
        "vault_not_found",
        Some(Value::Object(facts(account, now))),
    )
}

fn get_vault(account: &Account, now: i64) -> HttpResponse {
    let Some(vault) = &account.vault else {
        return vault_not_found(account, now);
    };
    let mut body = json!({
        "key_id": vault.key_id.to_hex(),
        "write_pk_b64": vault.write_pk.to_base64(),
        "created_at": time(vault.created_at_ms),
        "rotations_b64": vault.rotations.iter().map(|rotation| BASE64.encode(rotation)).collect::<Vec<_>>(),
    });
    for (field, value) in facts(account, now) {
        body[field] = value;
    }
    response(200, body)
}

fn check_ciphertext(ciphertext: &[u8]) -> Option<HttpResponse> {
    if ciphertext.len() > SEALED_SIZES[SEALED_SIZES.len() - 1] {
        return Some(error(413, "vault_too_large", None));
    }
    (!SEALED_SIZES.contains(&ciphertext.len())).then(|| error(422, "vault_blob_invalid", None))
}

fn put_statement(
    user: &UserId,
    key_id: KeyId,
    schema: u16,
    base_version: u64,
    new_version: u64,
    ciphertext: &[u8],
) -> PutStatement {
    PutStatement {
        user_id: user.clone(),
        name: BlobName::prefs(),
        key_id,
        schema,
        base_version,
        new_version,
        ciphertext_sha256: sha256(ciphertext),
    }
}

/// The put signature over a statement the server built from the request
/// and the authenticated account, never from statement bytes it was sent.
fn verifies(write_pk: &WritePublicKey, signature: &Signature, put: &PutStatement) -> bool {
    put.verify(signature, &put.user_id, write_pk).is_ok()
}

fn create_vault(
    account: &mut Account,
    user: &UserId,
    request: &HttpRequest,
    now: i64,
) -> HttpResponse {
    let body = request.json_body.clone().unwrap_or_default();
    let (Some(write_pk), Some(ciphertext), Some(create_sig), Some(put_sig)) = (
        b64(&body["write_pk_b64"]).and_then(|bytes| WritePublicKey::from_slice(&bytes).ok()),
        b64(&body["blob"]["ciphertext_b64"]),
        b64(&body["create_sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
        b64(&body["put_sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
    ) else {
        return error(422, "validation_error", None);
    };
    if let Some(refusal) = check_ciphertext(&ciphertext) {
        return refusal;
    }
    if body["key_id"].as_str() != Some(write_pk.key_id().to_hex().as_str()) {
        return error(422, "vault_key_id_invalid", None);
    }
    let schema = body["blob"]["schema"].as_u64().unwrap_or(0) as u16;
    let create = CreateStatement {
        user_id: user.clone(),
        key_id: write_pk.key_id(),
        write_pk,
        ciphertext_sha256: sha256(&ciphertext),
    };
    let put = put_statement(user, write_pk.key_id(), schema, 0, 1, &ciphertext);
    if create.verify(&create_sig, user).is_err() || put.verify(&put_sig, user, &write_pk).is_err() {
        return error(422, "vault_signature_invalid", None);
    }
    if account.resets.iter().any(|reset| reset.status == "pending") {
        return error(409, "vault_reset_pending", None);
    }
    if account.vault.is_some() {
        return error(409, "vault_exists", None);
    }
    account.vault = Some(StoredVault {
        key_id: write_pk.key_id(),
        write_pk,
        created_at_ms: now,
        rotations: Vec::new(),
    });
    account.blob = Some(StoredBlob {
        version: 1,
        base_version: 0,
        schema,
        ciphertext,
        put_sig,
    });
    response(201, json!({"version": 1}))
}

fn get_blob(account: &Account, request: &HttpRequest, now: i64) -> HttpResponse {
    let Some(vault) = &account.vault else {
        return vault_not_found(account, now);
    };
    let Some(blob) = &account.blob else {
        return error(404, "vault_blob_not_found", None);
    };
    if tag(&request.precondition, false) == Some(blob.version) {
        return not_modified();
    }
    blob_response(vault.key_id, blob, now)
}

/// C2's `VaultBlobResponse` for `blob`.
pub fn blob_response(key_id: KeyId, blob: &StoredBlob, now: i64) -> HttpResponse {
    response(
        200,
        json!({
            "version": blob.version,
            "key_id": key_id.to_hex(),
            "schema": blob.schema,
            "ciphertext_b64": BASE64.encode(&blob.ciphertext),
            "put_sig_b64": blob.put_sig.to_base64(),
            "base_version": blob.base_version,
            "updated_at": time(now),
        }),
    )
}

fn put_blob(account: &mut Account, user: &UserId, request: &HttpRequest, now: i64) -> HttpResponse {
    let Some(base_version) = tag(&request.precondition, true) else {
        return error(428, "vault_if_match_required", None);
    };
    let body = request.json_body.clone().unwrap_or_default();
    let (Some(ciphertext), Some(put_sig), Some(new_version)) = (
        b64(&body["ciphertext_b64"]),
        b64(&body["put_sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
        body["new_version"].as_u64(),
    ) else {
        return error(422, "validation_error", None);
    };
    if let Some(refusal) = check_ciphertext(&ciphertext) {
        return refusal;
    }
    let Some(vault) = account.vault.clone() else {
        return vault_not_found(account, now);
    };
    if body["key_id"].as_str() != Some(vault.key_id.to_hex().as_str()) {
        return error(409, "vault_key_mismatch", None);
    }
    let Some(blob) = account.blob.as_mut() else {
        return error(404, "vault_blob_not_found", None);
    };
    if base_version != blob.version {
        return error(
            412,
            "vault_version_conflict",
            Some(json!({"current_version": blob.version})),
        );
    }
    if !(base_version < new_version && new_version <= base_version + (1 << 32)) {
        return error(422, "vault_version_invalid", None);
    }
    let schema = body["schema"].as_u64().unwrap_or(0) as u16;
    let put = put_statement(
        user,
        vault.key_id,
        schema,
        base_version,
        new_version,
        &ciphertext,
    );
    if !verifies(&vault.write_pk, &put_sig, &put) {
        return error(403, "vault_write_unauthorized", None);
    }
    *blob = StoredBlob {
        version: new_version,
        base_version,
        schema,
        ciphertext,
        put_sig,
    };
    response(200, json!({"version": new_version}))
}

fn rotate(account: &mut Account, user: &UserId, request: &HttpRequest, now: i64) -> HttpResponse {
    let Some(base_version) = tag(&request.precondition, true) else {
        return error(428, "vault_if_match_required", None);
    };
    let body = request.json_body.clone().unwrap_or_default();
    let (
        Some(new_pk),
        Some(statement),
        Some(old_sig),
        Some(new_sig),
        Some(ciphertext),
        Some(put_sig),
    ) = (
        b64(&body["new_write_pk_b64"]).and_then(|bytes| WritePublicKey::from_slice(&bytes).ok()),
        b64(&body["rotate_stmt_b64"]),
        b64(&body["old_sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
        b64(&body["new_sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
        b64(&body["blob"]["ciphertext_b64"]),
        b64(&body["blob"]["put_sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
    )
    else {
        return error(422, "validation_error", None);
    };
    let new_version = body["blob"]["new_version"].as_u64().unwrap_or(0);
    let Some(vault) = account.vault.clone() else {
        return vault_not_found(account, now);
    };
    let Some(blob) = account.blob.clone() else {
        return error(404, "vault_blob_not_found", None);
    };
    if base_version != blob.version {
        return error(
            412,
            "vault_version_conflict",
            Some(json!({"current_version": blob.version})),
        );
    }
    let expected = RotateStatement {
        user_id: user.clone(),
        old_key_id: vault.key_id,
        new_key_id: new_pk.key_id(),
        new_write_pk: new_pk,
        new_version,
    };
    if statement != expected.to_bytes() {
        return error(422, "vault_statement_invalid", None);
    }
    let rotation = SignedRotation {
        statement: expected,
        old_signature: old_sig,
        new_signature: new_sig,
    };
    let put = put_statement(
        user,
        new_pk.key_id(),
        1,
        base_version,
        new_version,
        &ciphertext,
    );
    if rotation.verify(user, &vault.write_pk).is_err() || !verifies(&new_pk, &put_sig, &put) {
        return error(403, "vault_write_unauthorized", None);
    }
    let mut stored = vault;
    stored.rotations.push(rotation.to_bytes());
    stored.key_id = new_pk.key_id();
    stored.write_pk = new_pk;
    account.vault = Some(stored);
    account.blob = Some(StoredBlob {
        version: new_version,
        base_version,
        schema: 1,
        ciphertext,
        put_sig,
    });
    response(200, json!({"version": new_version}))
}

fn delete_vault(
    account: &mut Account,
    user: &UserId,
    request: &HttpRequest,
    now: i64,
) -> HttpResponse {
    let body = request.json_body.clone().unwrap_or_default();
    let (Some(statement), Some(signature)) = (
        b64(&body["delete_stmt_b64"]),
        b64(&body["sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
    ) else {
        return error(422, "validation_error", None);
    };
    let Ok(parsed) = DeleteStatement::parse(&statement) else {
        return error(422, "vault_statement_invalid", None);
    };
    if (parsed.requested_at - now / 1_000).abs() > 600 {
        return error(422, "vault_statement_invalid", None);
    }
    let Some(vault) = account.vault.clone() else {
        return vault_not_found(account, now);
    };
    if body["key_id"].as_str() != Some(vault.key_id.to_hex().as_str()) {
        return error(409, "vault_key_mismatch", None);
    }
    let current = account.blob.as_ref().map_or(0, |blob| blob.version);
    if parsed.at_version != current {
        return error(
            412,
            "vault_version_conflict",
            Some(json!({"current_version": current})),
        );
    }
    let signed = Signed {
        statement: parsed,
        signature,
    };
    if signed.verify(user, &vault.write_pk).is_err() {
        return error(403, "vault_write_unauthorized", None);
    }
    account.tombstone = Some((vault.key_id, now, signed.to_bytes()));
    for reset in &mut account.resets {
        if reset.status == "pending" {
            reset.status = "cancelled";
            reset.resolved_at_ms = Some(now);
        }
    }
    account.vault = None;
    account.blob = None;
    HttpResponse {
        raw_body: None,
        ..response(204, Value::Null)
    }
}

fn cancel_reset(
    account: &mut Account,
    user: &UserId,
    request: &HttpRequest,
    id: &str,
    now: i64,
) -> HttpResponse {
    let body = request.json_body.clone().unwrap_or_default();
    let (Some(statement), Some(signature)) = (
        b64(&body["cancel_stmt_b64"]),
        b64(&body["sig_b64"]).and_then(|bytes| Signature::from_slice(&bytes).ok()),
    ) else {
        return error(422, "validation_error", None);
    };
    let Ok(parsed) = ResetCancelStatement::parse(&statement) else {
        return error(422, "vault_statement_invalid", None);
    };
    let Some(vault) = account.vault.clone() else {
        return vault_not_found(account, now);
    };
    if parsed.reset_request_id.to_string() != id {
        return error(422, "vault_statement_invalid", None);
    }
    let signed = Signed {
        statement: parsed,
        signature,
    };
    if signed.verify(user, &vault.write_pk).is_err() {
        return error(403, "vault_write_unauthorized", None);
    }
    let Some(reset) = account
        .resets
        .iter_mut()
        .find(|reset| reset.id == id && reset.status == "pending")
    else {
        return error(404, "vault_reset_not_found", None);
    };
    reset.status = "cancelled";
    reset.resolved_at_ms = Some(now);
    HttpResponse {
        raw_body: None,
        ..response(204, Value::Null)
    }
}

fn list_devices(account: &Account, current: &str) -> HttpResponse {
    let devices: Vec<Value> = account
        .devices
        .iter()
        .map(|device| {
            json!({
                "id": device.id,
                "status": if device.active { "active" } else { "revoked" },
                "created_at": time(device.created_at_ms),
                "last_seen_at": device.last_seen_at_ms.map(time),
                "client_version": "1.0.15",
                "is_current": device.id == current,
            })
        })
        .collect();
    response(200, Value::Array(devices))
}

/// One device's connection to the fake: what `AuthManager` would be.
pub struct FakeClient {
    core: FakeCore,
    pub user: String,
    pub device: String,
}

impl HttpClient for FakeClient {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        Box::pin(async move { self.core.handle(self, &request) })
    }
}

/// The server's raw stored state for one account, for assertions.
pub fn stored_version(core: &FakeCore, user: &str) -> Option<u64> {
    core.with_account(user, |account| {
        account.blob.as_ref().map(|blob| blob.version)
    })
}
