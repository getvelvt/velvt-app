//! One entry point per vault fuzz target (design §16.1).
//!
//! Shared by `fuzz_targets/*.rs`, which cargo-fuzz runs under libFuzzer, and
//! by `tests/vault_fuzz_corpus.rs`, which replays every committed corpus input
//! on each `cargo test`, with no nightly toolchain. Each entry point must
//! never panic on any input; the assertions inside check that whatever parses
//! re-encodes to what was parsed, and that a decoded document merges with
//! itself and with an empty copy as the merge laws say.

#![allow(dead_code)]

use velvt_service::vault::code::{open_handoff, parse_handoff_plaintext, SetupCode};
use velvt_service::vault::crypto::statement::{
    CreateStatement, DeleteStatement, HandoffStatement, PutStatement, ResetCancelStatement,
    RotateStatement, Signed, SignedRotation, Statement,
};
use velvt_service::vault::crypto::{unpad_document, UserId, VaultKey, WritePublicKey};
use velvt_service::vault::doc::{decode, DecodedDocument, DocumentError, PrefsDocument};
use velvt_service::vault::merge::{merge, merge_older_copy, HlcClock};

/// The code and account the published vectors seal their handoff payload
/// under, so the committed payload seed opens and the fuzzer starts past the
/// AEAD.
const SETUP_SECRET: [u8; 13] = [
    0x0a, 0x5c, 0x31, 0x9e, 0x07, 0xd2, 0x44, 0xb8, 0x6f, 0x13, 0xe9, 0x20, 0x8b,
];
const USER_ID: &str = "8f14e45f-ceea-467a-9575-1f2ab6c9d0e1";
/// The published vectors' `K` (`00 01 … 1f`): statements in the committed
/// corpus are signed by it, so a mutated one reaches the signature check.
const VECTOR_KEY: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];
/// 2025-10-15T00:00:00Z, the vectors' clock.
const NOW_MS: u64 = 1_760_486_400_000;

/// The opened document's plaintext (`u32_be(len) ‖ json ‖ zeros`), then the
/// same bytes as bare JSON.
pub fn vault_document(data: &[u8]) {
    if let Ok(json) = unpad_document(data) {
        document_json(json);
    }
    document_json(data);
}

fn document_json(json: &[u8]) {
    let Ok(DecodedDocument::Current(document)) = decode(json) else {
        return;
    };
    merge_laws(&document);
    match document.to_json() {
        Ok(encoded) => assert_eq!(
            decode(&encoded),
            Ok(DecodedDocument::Current(document)),
            "a decoded document does not read back from its own encoding"
        ),
        // Canonical JSON can be longer than the input only by an explicit
        // `"value":null`, which matters only within bytes of the limit.
        Err(DocumentError::TooLarge) => {}
        Err(error) => panic!("a decoded document failed to encode: {error}"),
    }
}

/// The merge surface a buggy Mac's document reaches (T18): merging a valid
/// copy with itself, or with an empty copy under the same salt, gives it back
/// either way round, an older copy of itself adds nothing, and an issuing
/// clock that has seen it either issues a later HLC or refuses, never panics.
fn merge_laws(document: &PrefsDocument) {
    let empty = PrefsDocument::new(document.person_salt.clone());
    for merged in [
        merge(document, document),
        merge(document, &empty),
        merge(&empty, document),
    ] {
        assert_eq!(&merged.document, document, "a merge changed a valid copy");
        assert!(merged.rules_over_cap.is_empty() && merged.devices_over_cap.is_empty());
    }
    assert_eq!(&merge_older_copy(document, document), document);
    assert_eq!(&merge_older_copy(document, &empty), document);
    assert_eq!(merge_older_copy(&empty, document), empty);

    let Some(device) = document.devices.keys().next() else {
        return;
    };
    let mut clock = HlcClock::new(*device);
    clock.observe_document(document);
    for now_ms in [0, NOW_MS, u64::MAX] {
        let _ = clock.tick(now_ms);
    }
}

pub fn vault_setup_code(data: &[u8]) {
    let Ok(typed) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(code) = SetupCode::parse(typed) {
        let shown = code.display();
        let again = SetupCode::parse(&shown).expect("a shown code reads back");
        assert_eq!(again.display(), shown);
    }
}

/// Every statement parser, then, for whatever parses, the check a Mac runs
/// on bytes a malicious server chose: against the vectors' key, whose
/// statements the corpus holds, and against a key from the input itself.
pub fn vault_statements(data: &[u8]) {
    round_trip::<PutStatement>(data);
    round_trip::<CreateStatement>(data);
    round_trip::<RotateStatement>(data);
    round_trip::<DeleteStatement>(data);
    round_trip::<HandoffStatement>(data);
    round_trip::<ResetCancelStatement>(data);
    if let Ok(rotation) = SignedRotation::parse(data) {
        assert_eq!(rotation.to_bytes(), data);
    }

    let user_id = UserId::new(USER_ID).expect("user id");
    let vector_key = VaultKey::from_slice(&VECTOR_KEY).expect("32 bytes");
    let mut trusted = vec![*vector_key.write_public_key()];
    if let Some(chosen) = data
        .get(..32)
        .and_then(|bytes| WritePublicKey::from_slice(bytes).ok())
    {
        trusted.push(chosen);
    }
    for key in &trusted {
        if let Ok(rotation) = SignedRotation::parse(data) {
            if let Ok(next) = rotation.verify(&user_id, key) {
                assert_eq!(next, rotation.statement.new_write_pk);
                assert_ne!(next.key_id(), key.key_id());
            }
        }
        if let Ok(delete) = Signed::<DeleteStatement>::parse(data) {
            let _ = delete.verify(&user_id, key);
        }
        if let Ok(cancel) = Signed::<ResetCancelStatement>::parse(data) {
            let _ = cancel.verify(&user_id, key);
        }
        if let Ok(handoff) = Signed::<HandoffStatement>::parse(data) {
            let _ = handoff.verify_handoff(&user_id, key);
        }
        if let Ok(put) = Signed::<PutStatement>::parse(data) {
            let _ = put.statement.verify(&put.signature, &user_id, key);
        }
    }
    if let Ok(create) = Signed::<CreateStatement>::parse(data) {
        let _ = create.statement.verify(&create.signature, &user_id);
    }
}

fn round_trip<S: Statement + std::fmt::Debug + PartialEq>(data: &[u8]) {
    if let Ok(statement) = S::parse(data) {
        assert_eq!(statement.to_bytes(), data);
    }
    if let Ok(signed) = Signed::<S>::parse(data) {
        assert_eq!(signed.to_bytes(), data);
    }
}

/// A claimed payload, opened with a fixed code, then the same bytes as an
/// opened plaintext.
pub fn vault_handoff_payload(data: &[u8]) {
    let code = SetupCode::from_secret_bytes(&SETUP_SECRET).expect("100-bit secret");
    let user_id = UserId::new(USER_ID).expect("user id");
    if let Ok(contents) = open_handoff(&code, &user_id, data) {
        assert_eq!(contents.key.key_id(), contents.key_id);
    }
    if let Ok(contents) = parse_handoff_plaintext(data) {
        assert_eq!(contents.key.key_id(), contents.key_id);
    }
}
