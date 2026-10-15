//! One entry point per vault fuzz target (design §16.1).
//!
//! Shared by `fuzz_targets/*.rs`, which cargo-fuzz runs under libFuzzer, and
//! by `tests/vault_fuzz_corpus.rs`, which replays every committed corpus input
//! on each `cargo test`, with no nightly toolchain. Each entry point must
//! never panic on any input; the assertions inside check that whatever parses
//! re-encodes to what was parsed.

#![allow(dead_code)]

use velvt_service::vault::code::{open_handoff, parse_handoff_plaintext, SetupCode};
use velvt_service::vault::crypto::statement::{
    CreateStatement, DeleteStatement, HandoffStatement, PutStatement, ResetCancelStatement,
    RotateStatement, Signed, SignedRotation, Statement,
};
use velvt_service::vault::crypto::{unpad_document, UserId};
use velvt_service::vault::doc::{decode, DecodedDocument, DocumentError};

/// The code and account the published vectors seal their handoff payload
/// under, so the committed payload seed opens and the fuzzer starts past the
/// AEAD.
const SETUP_SECRET: [u8; 13] = [
    0x0a, 0x5c, 0x31, 0x9e, 0x07, 0xd2, 0x44, 0xb8, 0x6f, 0x13, 0xe9, 0x20, 0x8b,
];
const USER_ID: &str = "8f14e45f-ceea-467a-9575-1f2ab6c9d0e1";

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
