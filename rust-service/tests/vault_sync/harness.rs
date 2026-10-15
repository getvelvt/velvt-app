//! A Mac running the sync engine against the fake, and the other actors a
//! test needs: a Mac that rotates or deletes (PRs A8 and A4 make those
//! initiators; the bytes they send are A1's), and someone who has only the
//! password.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::json;
use velvt_service::auth::{AuthError, HttpClient, HttpRequest, HttpResponse, Precondition};
use velvt_service::persistence::{
    KnownDeviceRecord, LocalSyncSettings, PersistenceError, SqlitePersistence, SyncNoticeRecord,
    VaultRuleLinkRecord, VaultStateRecord, VaultStateRepo,
};
use velvt_service::vault::crypto::statement::{
    DeleteStatement, PutStatement, ResetCancelStatement, Signed, SignedRotation,
};
use velvt_service::vault::crypto::{sha256, BlobName, DocumentContext, UserId, VaultKey};
use velvt_service::vault::doc::{self, DecodedDocument, DeviceName, PrefsDocument, ServerDeviceId};
use velvt_service::vault::sync::host::HostIdentity;
use velvt_service::vault::sync::keys::MemoryKeyStore;
use velvt_service::vault::sync::{BlockActivity, SyncDeps, SyncEngine, SyncRouting, SyncSnapshot};

use super::fake_core::{Clock, FakeClient, FakeCore};

/// A work block that a test starts and ends.
#[derive(Default)]
pub struct BlockSwitch(AtomicBool);

impl BlockSwitch {
    pub fn set(&self, active: bool) {
        self.0.store(active, Ordering::SeqCst);
    }
}

impl BlockActivity for BlockSwitch {
    fn block_active(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The host UUID, which a test can swap to play Migration Assistant.
pub struct Host(Mutex<Option<[u8; 16]>>);

impl Host {
    pub fn set(&self, uuid: [u8; 16]) {
        *self.0.lock().unwrap() = Some(uuid);
    }
}

impl HostIdentity for Host {
    fn host_uuid(&self) -> Option<[u8; 16]> {
        *self.0.lock().unwrap()
    }
}

/// The Mac's database, which a test can make refuse to save the sync row.
pub struct Repo {
    inner: Arc<dyn VaultStateRepo>,
    refuse_saves: AtomicBool,
}

impl Repo {
    pub fn refuse_saves(&self, refuse: bool) {
        self.refuse_saves.store(refuse, Ordering::SeqCst);
    }
}

impl VaultStateRepo for Repo {
    fn vault_state(&self, user_id: &str) -> Result<Option<VaultStateRecord>, PersistenceError> {
        self.inner.vault_state(user_id)
    }
    fn vault_states(&self) -> Result<Vec<VaultStateRecord>, PersistenceError> {
        self.inner.vault_states()
    }
    fn save_vault_state(&self, record: &VaultStateRecord) -> Result<(), PersistenceError> {
        if self.refuse_saves.load(Ordering::SeqCst) {
            return Err(PersistenceError::InvalidTimestamp);
        }
        self.inner.save_vault_state(record)
    }
    fn clear_sessions(&self, keep: Option<&str>) -> Result<bool, PersistenceError> {
        self.inner.clear_sessions(keep)
    }
    fn delete_vault_state(&self, user_id: &str) -> Result<(), PersistenceError> {
        self.inner.delete_vault_state(user_id)
    }
    fn rule_links(&self) -> Result<Vec<VaultRuleLinkRecord>, PersistenceError> {
        self.inner.rule_links()
    }
    fn upsert_rule_link(&self, link: &VaultRuleLinkRecord) -> Result<(), PersistenceError> {
        self.inner.upsert_rule_link(link)
    }
    fn delete_pushed_tombstones(
        &self,
        pushed: &[VaultRuleLinkRecord],
    ) -> Result<u64, PersistenceError> {
        self.inner.delete_pushed_tombstones(pushed)
    }
    fn clear_rule_links(&self) -> Result<(), PersistenceError> {
        self.inner.clear_rule_links()
    }
    fn known_devices(&self) -> Result<Vec<KnownDeviceRecord>, PersistenceError> {
        self.inner.known_devices()
    }
    fn insert_known_devices(&self, devices: &[KnownDeviceRecord]) -> Result<(), PersistenceError> {
        self.inner.insert_known_devices(devices)
    }
    fn local_sync_settings(&self) -> Result<LocalSyncSettings, PersistenceError> {
        self.inner.local_sync_settings()
    }
    fn sync_notices(&self, user_id: &str) -> Result<Vec<SyncNoticeRecord>, PersistenceError> {
        self.inner.sync_notices(user_id)
    }
    fn save_sync_notice(&self, notice: &SyncNoticeRecord) -> Result<(), PersistenceError> {
        self.inner.save_sync_notice(notice)
    }
    fn delete_sync_notices(&self, keep: Option<&str>) -> Result<(), PersistenceError> {
        self.inner.delete_sync_notices(keep)
    }
    fn prune_sync_notices(&self, before_ms: i64) -> Result<u64, PersistenceError> {
        self.inner.prune_sync_notices(before_ms)
    }
}

/// The session the service holds: whichever account is signed in.
pub struct SessionClient(Mutex<Arc<FakeClient>>);

impl HttpClient for SessionClient {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, AuthError>> + Send + 'a>> {
        let client = Arc::clone(&self.0.lock().unwrap());
        Box::pin(async move { client.send(request).await })
    }
}

pub struct Mac {
    pub engine: SyncEngine,
    pub persistence: SqlitePersistence,
    pub keychain: MemoryKeyStore,
    pub user: UserId,
    pub device: ServerDeviceId,
    pub blocks: Arc<BlockSwitch>,
    pub routing: Arc<SyncRouting>,
    pub host: Arc<Host>,
    pub repo: Arc<Repo>,
    session: Arc<SessionClient>,
    core: FakeCore,
    clock: Clock,
}

pub fn user(name: &str) -> UserId {
    let digest = sha256(name.as_bytes());
    let bytes: [u8; 16] = digest[..16].try_into().unwrap();
    UserId::new(&uuid::Uuid::from_bytes(bytes).to_string()).unwrap()
}

impl Mac {
    /// A Mac signed in to `user`, whose iCloud Keychain is `keychain`.
    pub async fn new(
        core: &FakeCore,
        clock: &Clock,
        user: &UserId,
        keychain: &MemoryKeyStore,
        host: u8,
    ) -> Self {
        let device = ServerDeviceId::parse(&uuid::Uuid::new_v4().to_string()).unwrap();
        let client = core.sign_in(user.as_str(), &device.to_string());
        let persistence = SqlitePersistence::open_in_memory().unwrap();
        let blocks = Arc::new(BlockSwitch::default());
        let routing = Arc::new(SyncRouting::default());
        let host = Arc::new(Host(Mutex::new(Some([host; 16]))));
        let session = Arc::new(SessionClient(Mutex::new(client)));
        let repo = Arc::new(Repo {
            inner: persistence.vault_state_repo(),
            refuse_saves: AtomicBool::new(false),
        });
        let engine = engine(&session, keychain, &repo, &host, clock, &blocks, &routing);
        engine.session_started(user.clone(), Some(device)).await;
        Self {
            engine,
            persistence,
            keychain: keychain.clone(),
            user: user.clone(),
            device,
            blocks,
            routing,
            host,
            repo,
            session,
            core: core.clone(),
            clock: clock.clone(),
        }
    }

    /// The service restarting: a new engine over the same database.
    pub async fn restart(&mut self) {
        self.engine = engine(
            &self.session,
            &self.keychain,
            &self.repo,
            &self.host,
            &self.clock,
            &self.blocks,
            &self.routing,
        );
        self.engine
            .session_started(self.user.clone(), Some(self.device))
            .await;
    }

    /// Signs this Mac in to another account (an account switch), as a new
    /// device of that account.
    pub async fn switch_account(&mut self, user: &UserId) {
        let device = ServerDeviceId::parse(&uuid::Uuid::new_v4().to_string()).unwrap();
        *self.session.0.lock().unwrap() = self.core.sign_in(user.as_str(), &device.to_string());
        self.user = user.clone();
        self.device = device;
        self.engine
            .session_started(user.clone(), Some(device))
            .await;
    }

    /// The service's session moves to `user` before the engine is told: a
    /// pass already running when the person switches accounts.
    pub fn session_moves_to(&self, user: &UserId) {
        let device = uuid::Uuid::new_v4().to_string();
        *self.session.0.lock().unwrap() = self.core.sign_in(user.as_str(), &device);
    }

    /// Signs back in to the same account after Log Out, on the same device.
    pub async fn sign_back_in(&mut self) {
        self.engine
            .session_started(self.user.clone(), Some(self.device))
            .await;
    }

    pub async fn turn_on(&self, name: &str) -> SyncSnapshot {
        self.engine.turn_on(DeviceName::new(name)).await
    }

    pub async fn sync(&self) -> SyncSnapshot {
        self.engine.sync_now().await
    }

    pub fn row(&self) -> Option<VaultStateRecord> {
        self.persistence
            .vault_state_repo()
            .vault_state(self.user.as_str())
            .unwrap()
    }

    pub fn client(&self) -> Arc<FakeClient> {
        Arc::clone(&self.session.0.lock().unwrap())
    }
}

fn engine(
    session: &Arc<SessionClient>,
    keychain: &MemoryKeyStore,
    repo: &Arc<Repo>,
    host: &Arc<Host>,
    clock: &Clock,
    blocks: &Arc<BlockSwitch>,
    routing: &Arc<SyncRouting>,
) -> SyncEngine {
    SyncEngine::new(SyncDeps {
        http: Arc::clone(session) as Arc<dyn HttpClient>,
        keys: Arc::new(keychain.clone()),
        repo: Arc::clone(repo) as Arc<dyn VaultStateRepo>,
        host: Arc::clone(host) as Arc<dyn HostIdentity>,
        clock: Arc::new(clock.clone()),
        blocks: Arc::clone(blocks) as Arc<dyn BlockActivity>,
        routing: Arc::clone(routing),
        conflict_jitter: Duration::ZERO,
    })
}

// --- Other actors ----------------------------------------------------------------

async fn current_blob(client: &FakeClient) -> (u64, Vec<u8>) {
    let response = client
        .send(HttpRequest::get("/v1/vault/blobs/prefs"))
        .await
        .unwrap();
    let body = response.raw_body.unwrap();
    (
        body["version"].as_u64().unwrap(),
        BASE64
            .decode(body["ciphertext_b64"].as_str().unwrap())
            .unwrap(),
    )
}

pub fn open_copy(key: &VaultKey, user: &UserId, version: u64, ciphertext: &[u8]) -> PrefsDocument {
    let name = BlobName::prefs();
    let context = DocumentContext {
        user_id: user,
        name: &name,
        key_id: key.key_id(),
        schema: 1,
        version,
    };
    match doc::open(key, &context, ciphertext).unwrap() {
        DecodedDocument::Current(document) => document,
        DecodedDocument::NewerSchema(_) => panic!("schema 1"),
    }
}

pub fn seal_copy(key: &VaultKey, user: &UserId, version: u64, document: &PrefsDocument) -> Vec<u8> {
    let name = BlobName::prefs();
    let context = DocumentContext {
        user_id: user,
        name: &name,
        key_id: key.key_id(),
        schema: 1,
        version,
    };
    doc::seal(key, &context, document).unwrap()
}

/// The put signature a holder of `key` makes for `ciphertext`.
pub fn put_signature(
    key: &VaultKey,
    user: &UserId,
    base: u64,
    version: u64,
    ciphertext: &[u8],
) -> String {
    let put = PutStatement {
        user_id: user.clone(),
        name: BlobName::prefs(),
        key_id: key.key_id(),
        schema: 1,
        base_version: base,
        new_version: version,
        ciphertext_sha256: sha256(ciphertext),
    };
    key.sign_statement(&put).unwrap().to_base64()
}

/// "Lost or stolen: change the sync key" on another Mac (PR A8): the copy
/// re-sealed under `new` and `POST /v1/vault/rotate`, signed by both keys.
pub async fn rotate(
    client: &FakeClient,
    user: &UserId,
    old: &VaultKey,
    new: &VaultKey,
) -> HttpResponse {
    let (version, ciphertext) = current_blob(client).await;
    let document = open_copy(old, user, version, &ciphertext);
    let next = version + 1;
    let sealed = seal_copy(new, user, next, &document);
    let rotation = SignedRotation::sign(user.clone(), old, new, next).unwrap();
    let mut request = HttpRequest::post("/v1/vault/rotate")
        .with_precondition(Precondition::IfMatch(format!("\"{version}\"")));
    request.json_body = Some(json!({
        "old_key_id": old.key_id().to_hex(),
        "new_key_id": new.key_id().to_hex(),
        "new_write_pk_b64": new.write_public_key().to_base64(),
        "rotate_stmt_b64": BASE64.encode(velvt_service::vault::crypto::statement::Statement::to_bytes(&rotation.statement)),
        "old_sig_b64": rotation.old_signature.to_base64(),
        "new_sig_b64": rotation.new_signature.to_base64(),
        "blob": {
            "name": "prefs",
            "schema": 1,
            "new_version": next,
            "ciphertext_b64": BASE64.encode(&sealed),
            "put_sig_b64": put_signature(new, user, version, next, &sealed),
        },
    }));
    client.send(request).await.unwrap()
}

/// Delete Synced Data on another Mac (J9): a signed deletion.
pub async fn delete_synced_data(
    client: &FakeClient,
    user: &UserId,
    key: &VaultKey,
    now_ms: i64,
) -> HttpResponse {
    let (version, _) = current_blob(client).await;
    let signed = Signed::sign(
        key,
        DeleteStatement {
            user_id: user.clone(),
            key_id: key.key_id(),
            at_version: version,
            requested_at: now_ms / 1_000,
        },
    )
    .unwrap();
    let mut request = HttpRequest::delete("/v1/vault");
    request.json_body = Some(json!({
        "key_id": key.key_id().to_hex(),
        "delete_stmt_b64": BASE64.encode(velvt_service::vault::crypto::statement::Statement::to_bytes(&signed.statement)),
        "sig_b64": signed.signature.to_base64(),
    }));
    client.send(request).await.unwrap()
}

/// [Cancel Reset] on a Mac holding `key` (PR A8).
pub async fn cancel_reset(
    client: &FakeClient,
    user: &UserId,
    key: &VaultKey,
    reset_id: &str,
) -> HttpResponse {
    let signed = Signed::sign(
        key,
        ResetCancelStatement {
            user_id: user.clone(),
            key_id: key.key_id(),
            reset_request_id: uuid::Uuid::parse_str(reset_id).unwrap(),
        },
    )
    .unwrap();
    let mut request = HttpRequest::delete(format!("/v1/vault/reset-requests/{reset_id}"));
    request.json_body = Some(json!({
        "cancel_stmt_b64": BASE64.encode(velvt_service::vault::crypto::statement::Statement::to_bytes(&signed.statement)),
        "sig_b64": signed.signature.to_base64(),
    }));
    client.send(request).await.unwrap()
}

/// The vault key a Mac's Keychain holds for `user`, live.
pub async fn live_key(keychain: &MemoryKeyStore, user: &UserId) -> VaultKey {
    use velvt_service::vault::sync::keys::VaultKeyStore;
    let held = keychain.held(user);
    let live = held
        .iter()
        .find(|held| held.retired_by.is_none())
        .expect("a live key item");
    keychain.load(user, live.key_id).await.unwrap()
}
