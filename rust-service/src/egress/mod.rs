//! What leaves the device, recorded before it leaves.
//!
//! `ReqwestHttpClient` (`auth/http.rs`) holds the only network client in this
//! service, and its constructor takes an [`EgressLedgerRepo`]. Every request it
//! is asked to send is appended to the hash-chained `egress_ledger` table
//! (migration 0038) first; a request whose entry cannot be written is not sent.
//! [`dry_run`] prints what would be sent without sending it, and
//! `scripts/prove_egress.sh` checks the chain with sqlite3 and perl alone.
//!
//! [`EgressLedgerRepo`]: crate::persistence::EgressLedgerRepo

pub mod dry_run;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The first field of every hashed line. A new line format gets a new tag, so
/// a verifier can never mistake one format for another.
pub const LEDGER_HASH_DOMAIN: &str = "velvt-egress-v1";

/// `prev_hash` of the first entry ever written.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Entries older than this are pruned. Constants rather than settings, like
/// `out_of_block_run`'s, so widening either takes a code change and a
/// PRIVACY.md edit. 30 days is also how long a sent upload batch stays on disk,
/// so every upload entry the ledger still holds can be matched to its batch.
pub const EGRESS_LEDGER_RETENTION_DAYS: i64 = 30;

/// The row cap, whichever of the two bounds is tighter. At the default poll and
/// flush intervals a signed-in Mac makes roughly two thousand requests a day, so
/// the age bound is normally the one that applies.
pub const EGRESS_LEDGER_MAX_ENTRIES: u64 = 100_000;

/// What a secret value is replaced with before a credential body is hashed.
pub const REDACTED: &str = "[redacted]";

/// JSON keys whose values are secrets. The password on sign-up and log-in and
/// the refresh token on a token refresh are the ones sent today; the access
/// token is listed so a body that ever carried one would be hashed the same way.
const SECRET_FIELDS: &[&str] = &["password", "refresh_token", "access_token"];

/// One outbound request, described the way the ledger records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressRecord {
    pub method: String,
    /// The full URL the request went to, as `ledger_endpoint` stores it.
    pub endpoint: String,
    pub body_bytes: u64,
    pub body_sha256: String,
    pub body_redacted: bool,
    pub bearer: bool,
}

impl EgressRecord {
    /// Describes a request whose body is `body`, sent to `url`.
    ///
    /// `json` is the value `body` was serialized from, when there is one. It is
    /// read only to find secret fields: when there are none, `body_sha256` is
    /// the hash of `body` itself, byte for byte.
    pub fn new(method: &str, url: &str, body: &[u8], json: Option<&Value>, bearer: bool) -> Self {
        let redacted = json.and_then(redact_secrets);
        let body_sha256 = match &redacted {
            Some(redacted) => sha256_hex(&serde_json::to_vec(redacted).unwrap_or_default()),
            None => sha256_hex(body),
        };
        Self {
            method: method.to_owned(),
            endpoint: ledger_endpoint(url),
            body_bytes: body.len() as u64,
            body_sha256,
            body_redacted: redacted.is_some(),
            bearer,
        }
    }
}

/// One row of `egress_ledger`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressLedgerEntry {
    pub seq: i64,
    pub recorded_at: i64,
    pub method: String,
    pub endpoint: String,
    pub body_bytes: i64,
    pub body_sha256: String,
    pub body_redacted: bool,
    pub bearer: bool,
    pub prev_hash: String,
    pub entry_hash: String,
}

impl EgressLedgerEntry {
    /// The hash this entry's fields produce, whatever `entry_hash` says.
    pub fn computed_hash(&self) -> String {
        entry_hash(
            self.seq,
            self.recorded_at,
            &self.method,
            &self.endpoint,
            self.body_bytes,
            &self.body_sha256,
            self.body_redacted,
            self.bearer,
            &self.prev_hash,
        )
    }
}

/// The newest `egress_ledger_checkpoint` row: the last entry retention removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressCheckpoint {
    pub through_seq: i64,
    pub through_hash: String,
}

/// The line an entry's hash is taken over. `scripts/prove_egress.sh` builds the
/// same line in SQL; the two are pinned together by a shared test vector.
#[allow(clippy::too_many_arguments)]
pub fn chain_line(
    seq: i64,
    recorded_at: i64,
    method: &str,
    endpoint: &str,
    body_bytes: i64,
    body_sha256: &str,
    body_redacted: bool,
    bearer: bool,
    prev_hash: &str,
) -> String {
    format!(
        "{LEDGER_HASH_DOMAIN}|{seq}|{recorded_at}|{method}|{endpoint}|{body_bytes}|{body_sha256}|{}|{}|{prev_hash}",
        u8::from(body_redacted),
        u8::from(bearer),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn entry_hash(
    seq: i64,
    recorded_at: i64,
    method: &str,
    endpoint: &str,
    body_bytes: i64,
    body_sha256: &str,
    body_redacted: bool,
    bearer: bool,
    prev_hash: &str,
) -> String {
    sha256_hex(
        chain_line(
            seq,
            recorded_at,
            method,
            endpoint,
            body_bytes,
            body_sha256,
            body_redacted,
            bearer,
            prev_hash,
        )
        .as_bytes(),
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A URL as the ledger stores it: `|` and control characters percent-encoded,
/// because the endpoint is the one free-text field in the hashed line and the
/// verifier reads the ledger a line at a time. No URL the service builds today
/// contains either.
pub fn ledger_endpoint(url: &str) -> String {
    let mut encoded = String::with_capacity(url.len());
    for character in url.chars() {
        if character == '|' || character.is_control() {
            let mut buffer = [0_u8; 4];
            for byte in character.encode_utf8(&mut buffer).bytes() {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        } else {
            encoded.push(character);
        }
    }
    encoded
}

/// A copy of `value` with every secret field's value replaced by [`REDACTED`],
/// or `None` when it holds no secret field.
fn redact_secrets(value: &Value) -> Option<Value> {
    fn walk(value: &mut Value) -> bool {
        match value {
            Value::Object(fields) => {
                let mut found = false;
                for (key, child) in fields.iter_mut() {
                    if SECRET_FIELDS.contains(&key.as_str()) {
                        *child = Value::String(REDACTED.to_owned());
                        found = true;
                    } else {
                        found |= walk(child);
                    }
                }
                found
            }
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |found, item| walk(item) | found),
            _ => false,
        }
    }
    let mut copy = value.clone();
    walk(&mut copy).then_some(copy)
}

/// What a verified chain covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSummary {
    pub entries: usize,
    /// The seq the first surviving entry links back to: 0 when nothing was ever
    /// pruned, otherwise the checkpoint's `through_seq`.
    pub anchor_seq: i64,
    pub head_seq: i64,
    pub head_hash: String,
}

/// The first place a chain stops being one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainBreak {
    #[error("entry {seq} follows {expected_seq} (an entry is missing or out of order)")]
    Gap { seq: i64, expected_seq: i64 },
    #[error("entry {seq} does not link to the entry before it")]
    Link { seq: i64 },
    #[error("entry {seq} does not match its own hash (it was edited)")]
    Hash { seq: i64 },
}

/// Checks `entries`, oldest first, against `anchor`: each follows the last by
/// one, links to its hash, and hashes to what it says it does.
pub fn verify_chain(
    anchor: Option<&EgressCheckpoint>,
    entries: &[EgressLedgerEntry],
) -> Result<ChainSummary, ChainBreak> {
    let anchor_seq = anchor.map_or(0, |checkpoint| checkpoint.through_seq);
    let mut expected_seq = anchor_seq + 1;
    let mut expected_prev = anchor.map_or_else(
        || GENESIS_HASH.to_owned(),
        |checkpoint| checkpoint.through_hash.clone(),
    );
    for entry in entries {
        if entry.seq != expected_seq {
            return Err(ChainBreak::Gap {
                seq: entry.seq,
                expected_seq,
            });
        }
        if entry.prev_hash != expected_prev {
            return Err(ChainBreak::Link { seq: entry.seq });
        }
        if entry.computed_hash() != entry.entry_hash {
            return Err(ChainBreak::Hash { seq: entry.seq });
        }
        expected_prev = entry.entry_hash.clone();
        expected_seq += 1;
    }
    Ok(ChainSummary {
        entries: entries.len(),
        anchor_seq,
        head_seq: expected_seq - 1,
        head_hash: expected_prev,
    })
}

/// A request the service can make, for the dry run's list and for the test
/// that holds this list equal to the paths the source actually builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
    pub when: &'static str,
    pub body: &'static str,
}

/// Every endpoint rust-service sends to, taken from the source. The upload
/// batch is first; it is the only one that carries activity.
pub const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "POST",
        path: "/v1/events/batches",
        when: "every 60 s while there are events to send (50 events closes a batch early), and on retry",
        body: "the event batch printed above: per event an id, a timestamp, a duration, a broad category, a category-scoped abstraction type, and a classification tier",
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/events/{event_id}/classification",
        when: "when you correct a classification while signed in",
        body: "{\"category\": <the broad category you chose>}",
    },
    Endpoint {
        method: "POST",
        path: "/v1/auth/signup",
        when: "when you create an account",
        body: "{\"email\", \"password\"} (the ledger hashes it with the password redacted)",
    },
    Endpoint {
        method: "POST",
        path: "/v1/auth/login",
        when: "when you sign in",
        body: "{\"email\", \"password\"} (the ledger hashes it with the password redacted)",
    },
    Endpoint {
        method: "POST",
        path: "/v1/devices",
        when: "when you sign in on a Mac with no registered device: the first sign-in, and the first after Delete Account",
        body: "{\"client_version\"}",
    },
    Endpoint {
        method: "POST",
        path: "/v1/auth/devices/reissue",
        when: "when you sign in on a Mac that is already registered, and when the server says this device's token was revoked",
        body: "{\"device_id\"}",
    },
    Endpoint {
        method: "POST",
        path: "/v1/auth/refresh",
        when: "when the access token is about to expire",
        body: "{\"refresh_token\"} (the ledger hashes it with the token redacted)",
    },
    Endpoint {
        method: "GET",
        path: "/v1/auth/session",
        when: "when the app restores a saved session",
        body: "none",
    },
    Endpoint {
        method: "POST",
        path: "/v1/auth/logout",
        when: "when you sign out",
        body: "none",
    },
    Endpoint {
        method: "DELETE",
        path: "/v1/account",
        when: "when you use Delete Account",
        body: "none",
    },
    Endpoint {
        method: "GET",
        path: "/v1/insights/poll",
        when: "continuously while signed in: a long poll held open up to 70 s, then asked again (path set by VELVT_INSIGHT_POLL_PATH)",
        body: "none",
    },
    Endpoint {
        method: "GET",
        path: "/v1/history/daily?days=N",
        when: "every 10 minutes while signed in (N = 7), and when the app asks for history that is not cached",
        body: "none",
    },
    Endpoint {
        method: "GET",
        path: "/v1/insights/daily?date=YYYY-MM-DD",
        when: "every 10 minutes while signed in, and when the app asks for a day's insight that is not cached",
        body: "none",
    },
    Endpoint {
        method: "GET",
        path: "/v1/ready",
        when: "when the menu asks whether the server is reachable, at most once a minute",
        body: "none",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The vector `scripts/tests/prove_egress_test.sh` checks the shell
    /// verifier against. Changing the line format changes this hash, and the
    /// shell test fails until the verifier agrees.
    #[test]
    fn chain_line_matches_the_shared_test_vector() {
        let line = chain_line(
            1,
            1_800_000_000,
            "POST",
            "https://api.example.test/v1/events/batches",
            16,
            &sha256_hex(b"{\"batch_id\":\"b\"}"),
            false,
            true,
            GENESIS_HASH,
        );
        assert_eq!(
            line,
            "velvt-egress-v1|1|1800000000|POST|https://api.example.test/v1/events/batches|16|\
             df7a6dc1b45f44886ecf96df1b9d345eedb8c25a07ece23786eb107129f14b3d|0|1|\
             0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            sha256_hex(line.as_bytes()),
            "d6fd72f7470223ada172c5b57c615365750251795539350b1a1a8d70adad0a5b"
        );
    }

    #[test]
    fn a_body_without_secrets_is_hashed_byte_for_byte() {
        let body = json!({ "category": "FOCUS_WORK" });
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = EgressRecord::new("PATCH", "https://h/v1/x", &bytes, Some(&body), true);
        assert_eq!(record.body_sha256, sha256_hex(&bytes));
        assert_eq!(record.body_bytes, bytes.len() as u64);
        assert!(!record.body_redacted);
        assert!(record.bearer);
    }

    #[test]
    fn a_password_is_never_what_the_ledger_hashes() {
        let body = json!({ "email": "a@example.test", "password": "hunter2" });
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = EgressRecord::new(
            "POST",
            "https://h/v1/auth/login",
            &bytes,
            Some(&body),
            false,
        );
        let redacted =
            serde_json::to_vec(&json!({ "email": "a@example.test", "password": REDACTED }))
                .unwrap();
        assert!(record.body_redacted);
        assert_eq!(record.body_sha256, sha256_hex(&redacted));
        assert_ne!(record.body_sha256, sha256_hex(&bytes));
        assert_eq!(
            record.body_bytes,
            bytes.len() as u64,
            "the length is what was sent"
        );
    }

    #[test]
    fn a_refresh_token_is_redacted_too() {
        let body = json!({ "refresh_token": "secret" });
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = EgressRecord::new(
            "POST",
            "https://h/v1/auth/refresh",
            &bytes,
            Some(&body),
            false,
        );
        assert!(record.body_redacted);
    }

    #[test]
    fn an_empty_body_hashes_to_the_empty_digest() {
        let record = EgressRecord::new("GET", "https://h/v1/ready", &[], None, false);
        assert_eq!(
            record.body_sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(record.body_bytes, 0);
    }

    #[test]
    fn the_ledger_endpoint_cannot_split_a_hashed_line() {
        assert_eq!(ledger_endpoint("https://h/a|b\nc"), "https://h/a%7Cb%0Ac");
        assert_eq!(ledger_endpoint("https://h/v1/ready"), "https://h/v1/ready");
    }

    fn chain(count: i64) -> Vec<EgressLedgerEntry> {
        let mut previous = GENESIS_HASH.to_owned();
        (1..=count)
            .map(|seq| {
                let mut entry = EgressLedgerEntry {
                    seq,
                    recorded_at: 1_800_000_000 + seq,
                    method: "GET".into(),
                    endpoint: "https://h/v1/ready".into(),
                    body_bytes: 0,
                    body_sha256: sha256_hex(b""),
                    body_redacted: false,
                    bearer: false,
                    prev_hash: previous.clone(),
                    entry_hash: String::new(),
                };
                entry.entry_hash = entry.computed_hash();
                previous = entry.entry_hash.clone();
                entry
            })
            .collect()
    }

    #[test]
    fn an_intact_chain_verifies_and_reports_its_head() {
        let entries = chain(3);
        let summary = verify_chain(None, &entries).unwrap();
        assert_eq!(summary.entries, 3);
        assert_eq!(summary.head_seq, 3);
        assert_eq!(summary.head_hash, entries[2].entry_hash);
    }

    #[test]
    fn an_edited_entry_is_named() {
        let mut entries = chain(3);
        entries[1].body_bytes = 99;
        assert_eq!(
            verify_chain(None, &entries),
            Err(ChainBreak::Hash { seq: 2 })
        );
    }

    #[test]
    fn a_removed_entry_is_named() {
        let mut entries = chain(3);
        entries.remove(1);
        assert_eq!(
            verify_chain(None, &entries),
            Err(ChainBreak::Gap {
                seq: 3,
                expected_seq: 2
            })
        );
    }

    #[test]
    fn a_rehashed_entry_still_breaks_the_link_after_it() {
        let mut entries = chain(3);
        entries[1].body_bytes = 99;
        entries[1].entry_hash = entries[1].computed_hash();
        assert_eq!(
            verify_chain(None, &entries),
            Err(ChainBreak::Link { seq: 3 })
        );
    }

    #[test]
    fn a_pruned_chain_verifies_from_its_checkpoint() {
        let entries = chain(5);
        let checkpoint = EgressCheckpoint {
            through_seq: 2,
            through_hash: entries[1].entry_hash.clone(),
        };
        let summary = verify_chain(Some(&checkpoint), &entries[2..]).unwrap();
        assert_eq!(summary.anchor_seq, 2);
        assert_eq!(summary.head_seq, 5);
        assert_eq!(
            verify_chain(None, &entries[2..]),
            Err(ChainBreak::Gap {
                seq: 3,
                expected_seq: 1
            })
        );
    }
}
