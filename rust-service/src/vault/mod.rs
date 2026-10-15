//! Sync v1's cryptography, document and merge: the personal vault
//! (`docs/architecture/personal-vault-and-devices.md`, PR A1).
//!
//! Nothing in the shipped path calls this module. It makes no network call,
//! touches no SQLite table and sends nothing over IPC; `vault::sync` (PR A3)
//! is what will. Until then it is a library with tests, test vectors
//! (`docs/architecture/vault-v1-test-vectors.json`) and fuzz targets
//! (`rust-service/fuzz`).
//!
//! - [`crypto`]: the vault key `K`, the keys derived from it, sealing and
//!   opening the document, person keys, and the signed statements.
//! - [`doc`]: the `prefs` document's schema and validation.
//! - [`merge`]: hybrid logical clocks and the register merge.
//! - [`code`]: the setup code and the handoff payload it opens.
//!
//! Wire formats: `docs/architecture/vault-v1-formats.md`.

#![deny(unsafe_code)]

pub mod code;
pub mod crypto;
pub mod doc;
pub mod merge;
mod wire;

#[cfg(test)]
mod test_support {
    //! Fixtures shared by the vault's unit tests.

    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::crypto::{BlobName, PersonKey, PersonKeyKind, PersonSalt, UserId, VaultKey};
    use super::doc::{
        Category, DeviceEntry, DeviceName, JoinedVia, PrefsDocument, QuietHours, Register, RuleKey,
        RuleRegister, ServerDeviceId, Settings, SyncDeviceId,
    };
    use super::merge::Hlc;

    /// 2026-10-15T00:00:00Z.
    pub(crate) const NOW_MS: u64 = 1_760_486_400_000;

    /// A deterministic version-4 sync device id.
    pub(crate) fn device(seed: u8) -> SyncDeviceId {
        let mut bytes = [seed; 16];
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        SyncDeviceId::from_bytes(bytes).expect("version 4")
    }

    pub(crate) fn server_device(seed: u8) -> ServerDeviceId {
        ServerDeviceId::parse(&Uuid::from_bytes([seed; 16]).hyphenated().to_string())
            .expect("canonical")
    }

    pub(crate) fn hlc(wall_ms: u64, counter: u16, seed: u8) -> Hlc {
        Hlc::new(wall_ms, counter, device(seed)).expect("valid HLC")
    }

    pub(crate) fn register<T>(value: T, hlc: Hlc) -> Register<T> {
        Register { value, hlc }
    }

    pub(crate) fn name(value: &str) -> DeviceName {
        DeviceName::new(value).expect("valid name")
    }

    pub(crate) fn salt(seed: u8) -> PersonSalt {
        PersonSalt::from_bytes([seed; 32])
    }

    pub(crate) fn rule_key(kind: PersonKeyKind, seed: u8) -> RuleKey {
        RuleKey {
            kind,
            key: PersonKey::from_hex(&format!("{seed:02x}").repeat(32)).expect("hex"),
        }
    }

    pub(crate) fn rule(category: Category, deleted: bool, hlc: Hlc) -> RuleRegister {
        RuleRegister {
            category,
            deleted,
            hlc,
        }
    }

    pub(crate) fn device_entry(seed: u8, display: &str, at: Hlc) -> DeviceEntry {
        DeviceEntry {
            joined_via: JoinedVia::Created,
            joined_at_ms: at.wall_ms(),
            server_device_id: register(server_device(seed), at),
            name: register(name(display), at),
            notify_daily_insight: register(true, at),
            removed: register(false, at),
        }
    }

    /// Two Macs, one rule of each kind, and both settings.
    pub(crate) fn sample_document() -> PrefsDocument {
        let at = |counter| hlc(NOW_MS, counter, 1);
        let mut devices = BTreeMap::new();
        devices.insert(device(1), device_entry(1, "Studio Mac", at(0)));
        let mut laptop = device_entry(2, "MacBook Pro", hlc(NOW_MS + 5, 0, 2));
        laptop.joined_via = JoinedVia::Code;
        devices.insert(device(2), laptop);
        let mut rules = BTreeMap::new();
        rules.insert(
            rule_key(PersonKeyKind::Site, 0xa1),
            rule(Category::SocialFeed, false, at(1)),
        );
        rules.insert(
            rule_key(PersonKeyKind::Bundle, 0xb2),
            rule(Category::FocusWork, false, at(2)),
        );
        rules.insert(
            rule_key(PersonKeyKind::App, 0xc3),
            rule(Category::Communication, true, at(3)),
        );
        PrefsDocument {
            schema: 1,
            person_salt: salt(7),
            devices,
            rules,
            settings: Settings {
                invitations_enabled: Some(register(false, at(4))),
                quiet_hours: Some(register(
                    Some(QuietHours {
                        start_local_minutes: 1320,
                        end_local_minutes: 420,
                        rule_version: 1,
                    }),
                    at(5),
                )),
            },
        }
    }

    /// `K = 00 01 02 … 1f`.
    pub(crate) fn test_key() -> VaultKey {
        let bytes: Vec<u8> = (0..32).collect();
        VaultKey::from_slice(&bytes).expect("32 bytes")
    }

    pub(crate) fn other_key() -> VaultKey {
        let bytes: Vec<u8> = (0..32).map(|byte| 0xff - byte).collect();
        VaultKey::from_slice(&bytes).expect("32 bytes")
    }

    pub(crate) fn user() -> UserId {
        UserId::new("8f14e45f-ceea-467a-9575-1f2ab6c9d0e1").expect("user id")
    }

    pub(crate) fn other_user() -> UserId {
        UserId::new("c9f0f895-fb98-4b91-99f5-1e9a3c2f4d7b").expect("user id")
    }

    pub(crate) fn prefs() -> BlobName {
        BlobName::prefs()
    }

    /// xorshift64*: a small, seeded generator, so a failing case reproduces from
    /// its seed.
    pub(crate) struct Rng(u64);

    impl Rng {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
        }

        pub(crate) fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        pub(crate) fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }

        pub(crate) fn bytes<const N: usize>(&mut self) -> [u8; N] {
            let mut bytes = [0_u8; N];
            for byte in &mut bytes {
                *byte = self.next() as u8;
            }
            bytes
        }
    }

    /// Every ordering of `items`, by Heap's algorithm.
    pub(crate) fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        fn heap<T: Clone>(k: usize, items: &mut Vec<T>, out: &mut Vec<Vec<T>>) {
            if k <= 1 {
                out.push(items.clone());
                return;
            }
            heap(k - 1, items, out);
            for index in 0..k - 1 {
                if k.is_multiple_of(2) {
                    items.swap(index, k - 1);
                } else {
                    items.swap(0, k - 1);
                }
                heap(k - 1, items, out);
            }
        }
        let mut items = items.to_vec();
        let mut out = Vec::new();
        heap(items.len(), &mut items, &mut out);
        out
    }
}

#[cfg(test)]
mod vectors {
    //! The published test vectors (`docs/architecture/vault-v1-test-vectors.json`).
    //!
    //! Two tests: one rebuilds the file from fixed inputs and fails if a single
    //! byte differs, so the file cannot drift from the code; the other reads the
    //! file as an outside implementation would and checks it opens and verifies.
    //! To rewrite the file after an intended format change, run
    //! `VELVT_WRITE_VAULT_VECTORS=1 cargo test vault::vectors`.

    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;
    use serde_json::{json, Value};
    use uuid::Uuid;

    use super::code::{open_handoff, seal_handoff_with_nonce, SetupCode};
    use super::crypto::statement::{
        CreateStatement, DeleteStatement, HandoffStatement, PutStatement, ResetCancelStatement,
        Signed, SignedRotation, Statement,
    };
    use super::crypto::{
        encode_hex, hkdf_sha256, sealed_size_for, sha256, BlobName, DocumentContext, PersonKey,
        PersonKeyKind, PersonSalt, UserId, VaultKey, WritePublicKey, KEY_LENGTH,
        MAX_DOCUMENT_JSON_LENGTH,
    };
    use super::doc::{decode, DecodedDocument, RuleKey, SyncDeviceId};
    use super::merge::{Hlc, HlcClock};
    use super::test_support::{device, other_key, sample_document, test_key, user};

    const PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/architecture/vault-v1-test-vectors.json"
    );
    const VERSION: u64 = 7;
    const NONCE: [u8; 12] = [0x0c; 12];
    const HANDOFF_NONCE: [u8; 12] = [0x0d; 12];
    const SETUP_SECRET: [u8; 13] = [
        0x0a, 0x5c, 0x31, 0x9e, 0x07, 0xd2, 0x44, 0xb8, 0x6f, 0x13, 0xe9, 0x20, 0x8b,
    ];
    const REQUESTED_AT_MS: i64 = 1_760_486_400_000;
    const EXPIRES_AT_MS: i64 = 1_760_487_000_000;

    fn hex(bytes: &[u8]) -> String {
        encode_hex(bytes)
    }

    fn unhex(value: &Value) -> Vec<u8> {
        let text = value.as_str().expect("hex string");
        (0..text.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
            .collect()
    }

    fn issuer() -> SyncDeviceId {
        device(0x5e)
    }

    fn build() -> Value {
        let key = test_key();
        let next = other_key();
        let user_id = user();
        let name = BlobName::prefs();
        let context = DocumentContext {
            user_id: &user_id,
            name: &name,
            key_id: key.key_id(),
            schema: 1,
            version: VERSION,
        };
        let json = sample_document().to_json().unwrap();
        let sealed = key
            .seal_document_with_nonce(&context, &json, NONCE)
            .unwrap();
        let ciphertext_sha256 = sha256(&sealed);

        let put = PutStatement {
            user_id: user_id.clone(),
            name: name.clone(),
            key_id: key.key_id(),
            schema: 1,
            base_version: VERSION - 1,
            new_version: VERSION,
            ciphertext_sha256,
        };
        let create = CreateStatement {
            user_id: user_id.clone(),
            key_id: key.key_id(),
            write_pk: *key.write_public_key(),
            ciphertext_sha256,
        };
        let rotation = SignedRotation::sign(user_id.clone(), &key, &next, VERSION + 1).unwrap();
        let delete = Signed::sign(
            &key,
            DeleteStatement {
                user_id: user_id.clone(),
                key_id: key.key_id(),
                at_version: VERSION,
                requested_at_ms: REQUESTED_AT_MS,
            },
        )
        .unwrap();
        let code = SetupCode::from_secret_bytes(&SETUP_SECRET).unwrap();
        let payload =
            seal_handoff_with_nonce(&code, &user_id, &key, issuer(), HANDOFF_NONCE).unwrap();
        let okm = hkdf_sha256::<48>(&SETUP_SECRET, b"velvt-vault-handoff-v1").unwrap();
        let handoff = Signed::sign(
            &key,
            HandoffStatement {
                user_id: user_id.clone(),
                handoff_id: code.handoff_id().unwrap(),
                payload_sha256: sha256(&payload),
                expires_at_ms: EXPIRES_AT_MS,
            },
        )
        .unwrap();
        let cancel = Signed::sign(
            &key,
            ResetCancelStatement {
                user_id: user_id.clone(),
                key_id: key.key_id(),
                reset_request_id: Uuid::from_bytes([0x3c; 16]),
            },
        )
        .unwrap();

        let salt = PersonSalt::from_bytes([0x11; 32]);
        let person_keys: Vec<Value> = [
            (PersonKeyKind::Site, "example.com"),
            (PersonKeyKind::Bundle, "com.example.editor"),
            (PersonKeyKind::App, "Example Editor"),
        ]
        .into_iter()
        .map(|(kind, label)| {
            let digest = sha256(label.as_bytes());
            let person_key = PersonKey::derive(&salt, kind, &digest);
            json!({
                "kind": kind.prefix(),
                "kind_code": kind.code(),
                "digest_hex": hex(&digest),
                "person_key_hex": person_key.to_hex(),
                "rule_key": RuleKey { kind, key: person_key }.to_string(),
            })
        })
        .collect();

        let mut clock = HlcClock::new(device(0x09));
        let observed = Hlc::new(1_760_486_460_000, Hlc::MAX_COUNTER, device(0xff)).unwrap();
        clock.observe(&observed);
        let next_hlc = clock.tick(1_760_486_400_000).unwrap();

        let padding: Vec<Value> = [0, 16_352, 16_353, 32_736, 32_737, MAX_DOCUMENT_JSON_LENGTH]
            .into_iter()
            .map(|length| json!({"json_length": length, "sealed_length": sealed_size_for(length)}))
            .collect();

        json!({
            "description": "Velvt vault v1 test vectors (design §5.1-§5.4, §10.3). Byte strings are lowercase hex unless a field name ends in _b64 (standard base64, padded). Ed25519 signatures are deterministic, so every value here is reproducible. Formats: docs/architecture/vault-v1-formats.md. Generated by rust-service/src/vault/vectors.rs, whose tests fail if the code and this file disagree.",
            "format": "velvt-vault-v1-test-vectors",
            "key": {
                "k_hex": hex(key.secret_bytes()),
                "k_b64": key.to_base64().as_str(),
                "k_doc_hex": hex(&hkdf_sha256::<KEY_LENGTH>(key.secret_bytes(), b"velvt-vault-doc-v1").unwrap()[..]),
                "w_seed_hex": hex(&hkdf_sha256::<KEY_LENGTH>(key.secret_bytes(), b"velvt-vault-write-v1").unwrap()[..]),
                "write_pk_hex": hex(key.write_public_key().as_bytes()),
                "write_pk_b64": key.write_public_key().to_base64(),
                "key_id": key.key_id().to_hex(),
                "fingerprint": key.fingerprint().to_string(),
            },
            "next_key": {
                "k_hex": hex(next.secret_bytes()),
                "write_pk_hex": hex(next.write_public_key().as_bytes()),
                "key_id": next.key_id().to_hex(),
            },
            "document": {
                "user_id": user_id.as_str(),
                "name": name.as_str(),
                "key_id": key.key_id().to_hex(),
                "schema": 1,
                "version": VERSION,
                "aad_hex": hex(&context.aad()),
                "nonce_hex": hex(&NONCE),
                "json": String::from_utf8(json.to_vec()).unwrap(),
                "json_length": json.len(),
                "sealed_length": sealed.len(),
                "sealed_sha256_hex": hex(&ciphertext_sha256),
                "sealed_b64": BASE64.encode(&sealed),
            },
            "statements": {
                "put": {
                    "base_version": VERSION - 1,
                    "new_version": VERSION,
                    "statement_hex": hex(&put.to_bytes()),
                    "signature_hex": hex(key.sign_statement(&put).unwrap().as_bytes()),
                },
                "create": {
                    "statement_hex": hex(&create.to_bytes()),
                    "signature_hex": hex(key.sign_statement(&create).unwrap().as_bytes()),
                },
                "rotate": {
                    "new_version": VERSION + 1,
                    "statement_hex": hex(&rotation.statement.to_bytes()),
                    "old_signature_hex": hex(rotation.old_signature.as_bytes()),
                    "new_signature_hex": hex(rotation.new_signature.as_bytes()),
                    "stored_hex": hex(&rotation.to_bytes()),
                },
                "delete": {
                    "at_version": VERSION,
                    "requested_at_ms": REQUESTED_AT_MS,
                    "statement_hex": hex(&delete.statement.to_bytes()),
                    "signature_hex": hex(delete.signature.as_bytes()),
                    "stored_hex": hex(&delete.to_bytes()),
                },
                "handoff": {
                    "expires_at_ms": EXPIRES_AT_MS,
                    "statement_hex": hex(&handoff.statement.to_bytes()),
                    "signature_hex": hex(handoff.signature.as_bytes()),
                },
                "reset_cancel": {
                    "reset_request_id": cancel.statement.reset_request_id.hyphenated().to_string(),
                    "statement_hex": hex(&cancel.statement.to_bytes()),
                    "signature_hex": hex(cancel.signature.as_bytes()),
                },
            },
            "setup_code": {
                "secret_hex": hex(&SETUP_SECRET),
                "code": code.display().as_str(),
                "handoff_id_hex": hex(&okm[..16]),
                "k_wrap_hex": hex(&okm[16..]),
                "issuer_sync_device_id": issuer().to_string(),
                "nonce_hex": hex(&HANDOFF_NONCE),
                "payload_hex": hex(&payload),
            },
            "person_keys": {
                "salt_hex": hex(&[0x11; 32]),
                "digest_note": "digest_hex stands in for the unsalted abstraction::key digest; here it is SHA-256 of a label",
                "keys": person_keys,
            },
            "padding": padding,
            "hlc_overflow": {
                "observed": observed.to_string(),
                "now_ms": 1_760_486_400_000_u64,
                "issuer": device(0x09).to_string(),
                "next": next_hlc.to_string(),
            },
        })
    }

    fn render(vectors: &Value) -> String {
        let mut text = serde_json::to_string_pretty(vectors).unwrap();
        text.push('\n');
        text
    }

    #[test]
    fn the_published_vectors_are_what_the_code_produces() {
        let built = render(&build());
        if std::env::var_os("VELVT_WRITE_VAULT_VECTORS").is_some() {
            std::fs::write(PATH, &built).unwrap();
        }
        let published = std::fs::read_to_string(PATH).unwrap_or_default();
        assert!(
            published == built,
            "{PATH} differs from what the code produces; if the format change is \
             intended, rerun with VELVT_WRITE_VAULT_VECTORS=1 and review the diff"
        );
    }

    /// Reads the published file the way another implementation would, using only
    /// the inputs it states.
    #[test]
    fn the_published_vectors_open_and_verify() {
        let text = std::fs::read_to_string(PATH).unwrap();
        let vectors: Value = serde_json::from_str(&text).unwrap();
        let key = VaultKey::from_slice(&unhex(&vectors["key"]["k_hex"])).unwrap();
        let write_pk = WritePublicKey::from_slice(&unhex(&vectors["key"]["write_pk_hex"])).unwrap();
        assert_eq!(*key.write_public_key(), write_pk);
        assert_eq!(key.key_id().to_hex(), vectors["key"]["key_id"]);
        assert_eq!(key.fingerprint().to_string(), vectors["key"]["fingerprint"]);

        let document = &vectors["document"];
        let user_id = UserId::new(document["user_id"].as_str().unwrap()).unwrap();
        let name = BlobName::new(document["name"].as_str().unwrap()).unwrap();
        let context = DocumentContext {
            user_id: &user_id,
            name: &name,
            key_id: key.key_id(),
            schema: 1,
            version: document["version"].as_u64().unwrap(),
        };
        assert_eq!(hex(&context.aad()), document["aad_hex"]);
        let sealed = BASE64
            .decode(document["sealed_b64"].as_str().unwrap())
            .unwrap();
        let json = key.open_document(&context, &sealed).unwrap();
        assert_eq!(
            json.as_slice(),
            document["json"].as_str().unwrap().as_bytes()
        );
        assert!(matches!(decode(&json), Ok(DecodedDocument::Current(_))));

        let statements = &vectors["statements"];
        let put = PutStatement::parse(&unhex(&statements["put"]["statement_hex"])).unwrap();
        assert_eq!(hex(&put.ciphertext_sha256), document["sealed_sha256_hex"]);
        let signature =
            super::crypto::Signature::from_slice(&unhex(&statements["put"]["signature_hex"]))
                .unwrap();
        put.verify(&signature, &user_id, &write_pk).unwrap();

        let create =
            CreateStatement::parse(&unhex(&statements["create"]["statement_hex"])).unwrap();
        let signature =
            super::crypto::Signature::from_slice(&unhex(&statements["create"]["signature_hex"]))
                .unwrap();
        create.verify(&signature, &user_id).unwrap();

        let rotation = SignedRotation::parse(&unhex(&statements["rotate"]["stored_hex"])).unwrap();
        let next_pk = rotation.verify(&user_id, &write_pk).unwrap();
        assert_eq!(hex(next_pk.as_bytes()), vectors["next_key"]["write_pk_hex"]);

        Signed::<DeleteStatement>::parse(&unhex(&statements["delete"]["stored_hex"]))
            .unwrap()
            .verify(&user_id, &write_pk)
            .unwrap();

        let code = SetupCode::parse(vectors["setup_code"]["code"].as_str().unwrap()).unwrap();
        assert_eq!(
            code.handoff_id().unwrap().to_hex(),
            vectors["setup_code"]["handoff_id_hex"]
        );
        let opened = open_handoff(
            &code,
            &user_id,
            &unhex(&vectors["setup_code"]["payload_hex"]),
        )
        .unwrap();
        assert_eq!(opened.key, key);
        assert_eq!(
            opened.issuer_sync_device_id.to_string(),
            vectors["setup_code"]["issuer_sync_device_id"]
        );

        let mut handoff_bytes = unhex(&statements["handoff"]["statement_hex"]);
        handoff_bytes.extend(unhex(&statements["handoff"]["signature_hex"]));
        let handoff = Signed::<HandoffStatement>::parse(&handoff_bytes).unwrap();
        handoff.verify_handoff(&user_id, &write_pk).unwrap();
        assert_eq!(
            hex(handoff.statement.handoff_id.as_bytes()),
            vectors["setup_code"]["handoff_id_hex"]
        );

        let mut cancel_bytes = unhex(&statements["reset_cancel"]["statement_hex"]);
        cancel_bytes.extend(unhex(&statements["reset_cancel"]["signature_hex"]));
        Signed::<ResetCancelStatement>::parse(&cancel_bytes)
            .unwrap()
            .verify(&user_id, &write_pk)
            .unwrap();

        let salt = PersonSalt::from_bytes(
            unhex(&vectors["person_keys"]["salt_hex"])
                .try_into()
                .unwrap(),
        );
        for entry in vectors["person_keys"]["keys"].as_array().unwrap() {
            let kind = PersonKeyKind::from_prefix(entry["kind"].as_str().unwrap()).unwrap();
            let digest: [u8; 32] = unhex(&entry["digest_hex"]).try_into().unwrap();
            assert_eq!(
                PersonKey::derive(&salt, kind, &digest).to_hex(),
                entry["person_key_hex"]
            );
        }

        let overflow = &vectors["hlc_overflow"];
        let observed = Hlc::parse(overflow["observed"].as_str().unwrap()).unwrap();
        let next = Hlc::parse(overflow["next"].as_str().unwrap()).unwrap();
        assert!(next > observed && next.counter() == 0);
        assert!(overflow["next"].as_str() > overflow["observed"].as_str());
    }
}
