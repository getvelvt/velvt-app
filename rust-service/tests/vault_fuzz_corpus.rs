//! Replays every committed vault fuzz input through the fuzz targets' own
//! entry points (`fuzz/harness.rs`), so a regression a fuzzer once found, or
//! a seed stops parsing, fails `cargo test` without a nightly toolchain. The
//! fuzzing itself runs in CI's Fuzz workflow (`make fuzz-vault`).

#[path = "../fuzz/harness.rs"]
mod harness;

use std::path::PathBuf;

use velvt_service::vault::code::{open_handoff, SetupCode};
use velvt_service::vault::crypto::statement::{
    DeleteStatement, PutStatement, Signed, SignedRotation, Statement,
};
use velvt_service::vault::crypto::{UserId, VaultKey};
use velvt_service::vault::doc::{decode, DecodedDocument};

type Target = (&'static str, fn(&[u8]));

const TARGETS: [Target; 4] = [
    ("vault_document", harness::vault_document),
    ("vault_setup_code", harness::vault_setup_code),
    ("vault_statements", harness::vault_statements),
    ("vault_handoff_payload", harness::vault_handoff_payload),
];

fn corpus(target: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/corpus")
        .join(target)
}

fn seed(target: &str, name: &str) -> Vec<u8> {
    std::fs::read(corpus(target).join(name))
        .unwrap_or_else(|error| panic!("{target}/{name}: {error}"))
}

#[test]
fn every_committed_fuzz_input_replays_without_a_panic() {
    for (target, run) in TARGETS {
        let mut inputs = 0;
        for entry in std::fs::read_dir(corpus(target)).unwrap() {
            let path = entry.unwrap().path();
            run(&std::fs::read(&path).unwrap());
            inputs += 1;
        }
        assert!(inputs > 0, "{target} has no committed corpus");
    }
}

/// The seeds start the fuzzer past each parser, not at its first check.
#[test]
fn the_seeds_parse() {
    assert!(matches!(
        decode(&seed("vault_document", "prefs-sample.json")),
        Ok(DecodedDocument::Current(_))
    ));
    assert!(matches!(
        decode(&seed("vault_document", "prefs-schema-2.json")),
        Ok(DecodedDocument::NewerSchema(_))
    ));
    let typed = String::from_utf8(seed("vault_setup_code", "code-lowercase-spaced")).unwrap();
    assert!(SetupCode::parse(&typed).is_ok());
    assert!(PutStatement::parse(&seed("vault_statements", "put-statement")).is_ok());
    // Signed by the vectors' key, so a mutation reaches the signature check.
    let user_id = UserId::new("8f14e45f-ceea-467a-9575-1f2ab6c9d0e1").unwrap();
    let vector_key = VaultKey::from_slice(&(0..32).collect::<Vec<u8>>()).unwrap();
    let trusted = vector_key.write_public_key();
    SignedRotation::parse(&seed("vault_statements", "rotate-stored"))
        .unwrap()
        .verify(&user_id, trusted)
        .unwrap();
    Signed::<DeleteStatement>::parse(&seed("vault_statements", "delete-signed"))
        .unwrap()
        .verify(&user_id, trusted)
        .unwrap();

    let code =
        SetupCode::parse(std::str::from_utf8(&seed("vault_setup_code", "code")).unwrap()).unwrap();
    assert!(open_handoff(&code, &user_id, &seed("vault_handoff_payload", "payload")).is_ok());
}

/// A target added to the fuzz crate is also run by `make fuzz-vault`, has a
/// committed corpus, and is replayed here.
#[test]
fn every_fuzz_target_is_run_and_replayed() {
    use std::collections::BTreeSet;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("fuzz/Cargo.toml")).unwrap();
    let built: BTreeSet<String> = manifest
        .lines()
        .filter_map(|line| line.strip_prefix("name = \"")?.strip_suffix('"'))
        .filter(|name| *name != "velvt-service-fuzz")
        .map(str::to_owned)
        .collect();
    let makefile = std::fs::read_to_string(root.join("../Makefile")).unwrap();
    let run: BTreeSet<String> = makefile
        .lines()
        .find_map(|line| line.strip_prefix("VAULT_FUZZ_TARGETS :="))
        .expect("Makefile names VAULT_FUZZ_TARGETS")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let replayed: BTreeSet<String> = TARGETS.iter().map(|(name, _)| (*name).to_owned()).collect();
    let corpora: BTreeSet<String> = std::fs::read_dir(root.join("fuzz/corpus"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();

    assert_eq!(built.len(), 4);
    assert_eq!(run, built, "make fuzz-vault runs every fuzz target");
    assert_eq!(replayed, built, "this file replays every fuzz target");
    assert_eq!(corpora, built, "every fuzz target has a committed corpus");
}
