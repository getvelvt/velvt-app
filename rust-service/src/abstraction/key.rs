use sha2::{Digest, Sha256};

const KEY_DOMAIN: &[u8] = b"velvt:abstraction-key:v1";
/// Separate domain so an app-scoped key can never collide with a
/// (app, title) key, even for an app whose title is empty.
const APP_KEY_DOMAIN: &[u8] = b"velvt:abstraction-app-key:v1";
/// Third domain, for the same reason the second one exists: a bundle-scoped
/// key and a name-scoped key are different facts about the same application and
/// must never hash alike. Without a separate domain an app whose bundle
/// identifier happened to equal another app's name would silently inherit its
/// corrections.
///
/// Spelled to the pattern its two siblings set — `velvt:abstraction-*-key:v1` —
/// and deliberately, because the pattern is what makes "one domain per fact"
/// visible at a glance and keeps a fourth key from being added without one. The
/// v2 classification contract first drafted `velvt.app.bundle.v1`; the contract
/// was reconciled to this string rather than the reverse, since a domain string
/// is a persisted format (`personal_app_override.bundle_key_hash` and
/// `raw_event_buffer.app_bundle_stable_id` hold digests made with it) and it is
/// already quoted in `persistence/sqlite.rs`'s own collision argument.
const APP_BUNDLE_KEY_DOMAIN: &[u8] = b"velvt:abstraction-app-bundle-key:v1";

/// Fourth domain, and the only one that is not about an application: the prefix
/// under which every digest above is keyed to this install (migration 0037).
///
/// Named to the house pattern for the reason the bundle domain's comment gives:
/// a domain string is a persisted format. Every key on disk since 0037 is an HMAC
/// over this prefix, so editing it orphans every mapping and every correction
/// without a single error.
const SALTED_KEY_DOMAIN: &[u8] = b"velvt:abstraction-salted-key:v1";

/// The per-install key every persisted identity digest is computed under.
///
/// Before migration 0037 the three keys below were plain SHA-256 over a public
/// domain string and the raw inputs, computed identically on every install. A
/// reader of this file could hash (application, window title) guesses once,
/// offline, and test them against any Velvt database; and a bundle digest was the
/// same 64 characters on every Mac that ran the application, so two databases
/// could be joined on it. Each key is now HMAC-SHA-256 under 32 bytes only this
/// install holds, so a guess has to be tested per device with that device's salt
/// in hand, and no key on one Mac matches a key on another.
///
/// It is not a secret from someone who already holds the database file. The salt
/// sits in `stable_key_salt` beside the keys, and has to: a correction must still
/// match its window after a restart, so the key that computed it cannot be
/// ephemeral. That is the same trade [`super::EmbeddingSalt`] makes, and this is a
/// different value on purpose -- the embedding salt may be re-minted at the cost
/// of a cache, and this one may not be re-minted without losing every correction
/// (`AbstractionMapRepo::stable_key_salt`).
///
/// It never leaves the device: nothing in `upload::dto` has a field it could
/// occupy, it implements no serializer, and `Debug` below refuses to print it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StableKeySalt([u8; Self::LENGTH]);

impl StableKeySalt {
    /// Matches `randomblob(32)` in migration 0037, which is what generates it.
    pub const LENGTH: usize = 32;

    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    /// Keys one unsalted digest to this install.
    ///
    /// The HMAC is taken over the digest rather than over the raw inputs, and
    /// that is what makes migration 0037 possible at all: the raw application
    /// name and window title are discarded at abstraction, so the only thing a
    /// row written before 0037 still holds is its digest, and the only way to
    /// re-key it is from that digest. Nothing is lost by it. Without the salt,
    /// HMAC over a SHA-256 digest is as untestable as HMAC over the input it
    /// digests; with the salt, both cost one hash per guess.
    fn key(&self, digest: &[u8; 32]) -> String {
        encode_hex(&hmac_sha256(&self.0, &[SALTED_KEY_DOMAIN, digest]))
    }

    /// The salted form of a key stored before migration 0037.
    ///
    /// Exactly what the functions below compute for the same raw inputs, which
    /// is the property that keeps a correction taught under 1.0.11 matching the
    /// same window afterwards. `None` for anything that is not a digest as
    /// `encode_hex` writes one -- 64 lowercase hexadecimal characters -- because a
    /// value of any other shape was never a key this file produced.
    pub(crate) fn rekey_stored_digest(&self, stored: &str) -> Option<String> {
        decode_digest(stored).map(|digest| self.key(&digest))
    }
}

impl std::fmt::Debug for StableKeySalt {
    /// Redacted rather than derived, for the reason `EmbeddingSalt` gives: a
    /// derived salt is one `?` away from a tracing field.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StableKeySalt(redacted)")
    }
}

/// Local-only raw fields made available to abstraction plugins.
pub struct RawKey {
    app_name: String,
    window_title: String,
}

impl RawKey {
    pub(crate) fn new(app_name: String, window_title: String) -> Self {
        Self {
            app_name,
            window_title,
        }
    }

    /// Returns the raw application name for local plugin matching only.
    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    /// Returns the raw window title for local plugin matching only.
    pub fn window_title(&self) -> &str {
        &self.window_title
    }

    pub(crate) fn stable_key(&self, salt: &StableKeySalt) -> String {
        salt.key(&window_digest(&self.app_name, &self.window_title))
    }

    /// Identity of the application alone, ignoring the window title.
    ///
    /// `stable_key` binds a correction to one exact (app, title) pair, so
    /// correcting "Cursor — main.rs" teaches Velvt nothing about
    /// "Cursor — lib.rs": the title changes, the hash changes, and the next
    /// file is unclassified again. This key is what lets one correction cover
    /// every window of an app.
    ///
    /// The title is deliberately excluded rather than normalized away — the
    /// point is to be title-independent, not title-tolerant.
    pub(crate) fn app_stable_key(&self, salt: &StableKeySalt) -> String {
        salt.key(&app_digest(&self.app_name))
    }
}

/// The window-scoped correction key.
///
/// `stable_context` is the focused site for a browser window and the raw
/// window title otherwise — the same value the engine keys on.
pub fn stable_key_for(salt: &StableKeySalt, app_name: &str, stable_context: &str) -> String {
    RawKey::new(app_name.to_owned(), stable_context.to_owned()).stable_key(salt)
}

/// The app-scoped correction key: the identity one correction generalizes to.
pub fn app_stable_key_for(salt: &StableKeySalt, app_name: &str) -> String {
    RawKey::new(app_name.to_owned(), String::new()).app_stable_key(salt)
}

/// The bundle-scoped correction key: the identity that survives a rename.
///
/// `app_stable_key_for` hashes the name macOS reports, and that name is a poor
/// identity. It is localized, so an accented or translated name never matches a
/// taxonomy entry written in English; it changes between releases; and it is
/// often not the name anyone uses — `NSRunningApplication.localizedName` for
/// Visual Studio Code is literally `Code`. A bundle identifier
/// (`com.microsoft.VSCode`) is none of those things: the developer chose it
/// once, and changing it makes a different application.
///
/// Computed only when the client actually reported a bundle identifier. Absence
/// is not an error, and it is not substituted for: an event with no bundle id
/// keys exactly the way it did before this function existed.
///
/// The identifier itself is local-only. This returns its keyed hash, which is
/// what gets persisted, so the raw identifier is never stored and can reach no
/// upload DTO. Keyed since migration 0037: the set of macOS bundle identifiers is
/// small and public, so an unsalted digest of one named the application to
/// anyone with a list of them, on every Mac alike.
pub fn app_bundle_key_for(salt: &StableKeySalt, bundle_id: &str) -> String {
    salt.key(&bundle_digest(bundle_id))
}

// The three unsalted digests, one per domain. Each is still computed exactly as
// it was before migration 0037 -- the raw bytes, length-prefixed, no classifier
// normalization -- because 0037 re-keys stored rows from these digests: changing
// one would orphan every mapping and correction taught under it.

fn window_digest(app_name: &str, window_title: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(KEY_DOMAIN);
    update_length_prefixed(&mut hasher, app_name.as_bytes());
    update_length_prefixed(&mut hasher, window_title.as_bytes());
    hasher.finalize().into()
}

fn app_digest(app_name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(APP_KEY_DOMAIN);
    update_length_prefixed(&mut hasher, app_name.as_bytes());
    hasher.finalize().into()
}

fn bundle_digest(bundle_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(APP_BUNDLE_KEY_DOMAIN);
    // Length-prefixed like its siblings even with one field, so a later field
    // cannot be appended without the prefix and reopen the ambiguity the other
    // two keys were careful to close.
    update_length_prefixed(&mut hasher, bundle_id.as_bytes());
    hasher.finalize().into()
}

/// HMAC-SHA-256 (RFC 2104) over `sha2`, which is already a dependency, rather
/// than a new crate for sixteen lines. Pinned below against RFC 4231's published
/// test vectors, including the one whose key is longer than a block.
fn hmac_sha256(key: &[u8], message: &[&[u8]]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0_u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block_key.map(|byte| byte ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block_key.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The inverse of `encode_hex`, and only of it: lowercase, exactly 32 bytes.
fn decode_digest(encoded: &str) -> Option<[u8; 32]> {
    fn nibble(character: u8) -> Option<u8> {
        match character {
            b'0'..=b'9' => Some(character - b'0'),
            b'a'..=b'f' => Some(character - b'a' + 10),
            _ => None,
        }
    }
    if encoded.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (byte, pair) in digest.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
        *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(digest)
}

fn update_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        app_bundle_key_for, app_stable_key_for, decode_digest, encode_hex, hmac_sha256,
        stable_key_for, Digest, RawKey, Sha256, StableKeySalt,
    };

    const SALT: StableKeySalt = StableKeySalt::from_bytes([0x5a; StableKeySalt::LENGTH]);
    const OTHER_SALT: StableKeySalt = StableKeySalt::from_bytes([0xa5; StableKeySalt::LENGTH]);

    /// The digest every key was before migration 0037, spelled out here rather
    /// than borrowed from the module, so a change to the module's digest cannot
    /// also change the expectation it is checked against.
    fn legacy_digest(domain: &[u8], fields: &[&str]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(domain);
        for field in fields {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        encode_hex(&hasher.finalize())
    }

    #[test]
    fn length_prefix_prevents_delimiter_ambiguity() {
        let first = RawKey::new("a".into(), "bc".into()).stable_key(&SALT);
        let second = RawKey::new("ab".into(), "c".into()).stable_key(&SALT);

        assert_ne!(first, second);
    }

    /// The whole point of the app key: one correction has to cover every
    /// window of the app, not just the file that happened to be open.
    #[test]
    fn the_app_key_ignores_the_window_title() {
        let editing = RawKey::new("Cursor".into(), "main.rs — velvt".into()).app_stable_key(&SALT);
        let reviewing = RawKey::new("Cursor".into(), "lib.rs — velvt".into()).app_stable_key(&SALT);

        assert_eq!(editing, reviewing);
    }

    #[test]
    fn the_app_key_still_separates_different_apps() {
        let editor = RawKey::new("Cursor".into(), String::new()).app_stable_key(&SALT);
        let chat = RawKey::new("Slack".into(), String::new()).app_stable_key(&SALT);

        assert_ne!(editor, chat);
    }

    /// A correction scoped to one window and a correction scoped to the whole
    /// app are different facts, so their keys must never collide — including
    /// for an app that reports no title at all.
    #[test]
    fn the_app_key_never_collides_with_the_pair_key() {
        let titled = RawKey::new("Cursor".into(), String::new());

        assert_ne!(titled.stable_key(&SALT), titled.app_stable_key(&SALT));
    }

    /// The case that motivated keying on the bundle at all: the name macOS
    /// reports for Visual Studio Code is `Code`, which matches no taxonomy
    /// entry, while the bundle identifier is exact and stable.
    #[test]
    fn the_bundle_key_is_stable_across_the_names_one_app_reports() {
        let short_name = app_bundle_key_for(&SALT, "com.microsoft.VSCode");
        let renamed = app_bundle_key_for(&SALT, "com.microsoft.VSCode");

        assert_eq!(short_name, renamed);
        assert_ne!(
            app_bundle_key_for(&SALT, "com.microsoft.VSCode"),
            app_bundle_key_for(&SALT, "com.vscodium.codium")
        );
    }

    /// Three domains, three different facts. A bundle key must never collide
    /// with a name key or a window key, including in the degenerate case where
    /// the bundle identifier and the application name are the same string.
    /// Keying all three under one salt must not undo that: the domains are
    /// inside the digest the HMAC is taken over.
    #[test]
    fn the_bundle_key_never_collides_with_the_name_or_window_keys() {
        let shared = "Terminal";

        let bundle = app_bundle_key_for(&SALT, shared);
        let app = app_stable_key_for(&SALT, shared);
        let window = stable_key_for(&SALT, shared, "");

        assert_ne!(bundle, app);
        assert_ne!(bundle, window);
        assert_ne!(app, window);
    }

    /// Pins the bundle domain string itself, not only its behaviour.
    ///
    /// The string is a persisted format twice over: every bundle digest written
    /// before migration 0037 was computed with it, and 0037 re-keys those rows
    /// from that digest, so editing it now orphans every `bundle_key_hash` a
    /// user taught and every `app_bundle_stable_id` on a stored event without a
    /// single error. It also carries the collision argument above, which is an
    /// argument about the *pattern* the domains share.
    #[test]
    fn the_bundle_key_domain_is_the_house_pattern_string() {
        let legacy = legacy_digest(
            b"velvt:abstraction-app-bundle-key:v1",
            &["com.microsoft.VSCode"],
        );

        assert_eq!(
            app_bundle_key_for(&SALT, "com.microsoft.VSCode"),
            SALT.rekey_stored_digest(&legacy).unwrap()
        );
    }

    /// Every key is 64 hex characters, because that is what the persisted
    /// columns CHECK for and what `normalized_app_stable_id` accepts over IPC.
    /// A key that failed either would surface far from its cause.
    #[test]
    fn every_key_is_a_sixty_four_character_lowercase_hex_digest() {
        for key in [
            app_bundle_key_for(&SALT, "com.apple.dt.Xcode"),
            app_stable_key_for(&SALT, "Xcode"),
            stable_key_for(&SALT, "Xcode", "AppDelegate.swift"),
        ] {
            assert_eq!(key.len(), 64);
            assert!(key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        }
    }

    /// The finding migration 0037 answers: before it, a key was a function of
    /// the raw inputs and public strings alone, so anyone with this file could
    /// compute it. After it, the unsalted digest must appear nowhere in the
    /// output -- a key that still equalled it would be the old oracle under a
    /// new name.
    #[test]
    fn no_key_equals_the_unsalted_digest_it_was_computed_from() {
        let cases = [
            (
                stable_key_for(&SALT, "Mail", "Re: offer letter"),
                legacy_digest(b"velvt:abstraction-key:v1", &["Mail", "Re: offer letter"]),
            ),
            (
                app_stable_key_for(&SALT, "Mail"),
                legacy_digest(b"velvt:abstraction-app-key:v1", &["Mail"]),
            ),
            (
                app_bundle_key_for(&SALT, "com.apple.mail"),
                legacy_digest(b"velvt:abstraction-app-bundle-key:v1", &["com.apple.mail"]),
            ),
        ];
        for (salted, legacy) in cases {
            assert_ne!(salted, legacy);
        }
    }

    /// Two installs must not share a key for the same window: that is what
    /// stops two databases being joined on their keys, and what makes a
    /// precomputed table of guesses useless against the second one.
    #[test]
    fn two_installs_key_the_same_window_differently() {
        assert_ne!(
            stable_key_for(&SALT, "Mail", "Re: offer letter"),
            stable_key_for(&OTHER_SALT, "Mail", "Re: offer letter")
        );
        assert_ne!(
            app_bundle_key_for(&SALT, "com.apple.mail"),
            app_bundle_key_for(&OTHER_SALT, "com.apple.mail")
        );
    }

    /// The property migration 0037 depends on, for every domain: re-keying the
    /// digest a 1.0.11 install stored gives exactly the key this build computes
    /// for the same raw inputs. If it did not, every correction taught before
    /// the upgrade would silently stop matching after it.
    #[test]
    fn re_keying_a_stored_digest_matches_the_key_computed_from_raw_inputs() {
        assert_eq!(
            SALT.rekey_stored_digest(&legacy_digest(
                b"velvt:abstraction-key:v1",
                &["Code", "private project"]
            )),
            Some(stable_key_for(&SALT, "Code", "private project"))
        );
        assert_eq!(
            SALT.rekey_stored_digest(&legacy_digest(b"velvt:abstraction-app-key:v1", &["Code"])),
            Some(app_stable_key_for(&SALT, "Code"))
        );
        assert_eq!(
            SALT.rekey_stored_digest(&legacy_digest(
                b"velvt:abstraction-app-bundle-key:v1",
                &["com.microsoft.VSCode"]
            )),
            Some(app_bundle_key_for(&SALT, "com.microsoft.VSCode"))
        );
    }

    /// Only a digest as `encode_hex` writes one is re-keyed. Uppercase, short,
    /// long and non-hex values were never a key this module produced, and the
    /// migration drops them -- the row, or the optional value -- rather than
    /// inventing a key for them.
    #[test]
    fn only_a_well_formed_digest_is_re_keyed() {
        let well_formed = "0f".repeat(32);
        assert!(SALT.rekey_stored_digest(&well_formed).is_some());
        for malformed in [
            String::new(),
            "0F".repeat(32),
            "0f".repeat(31),
            "0f".repeat(33),
            "zz".repeat(32),
        ] {
            assert_eq!(SALT.rekey_stored_digest(&malformed), None, "{malformed:?}");
        }
        assert_eq!(
            decode_digest(&well_formed).map(|digest| encode_hex(&digest)),
            Some(well_formed)
        );
    }

    #[test]
    fn the_salt_does_not_print() {
        assert_eq!(format!("{SALT:?}"), "StableKeySalt(redacted)");
    }

    /// RFC 4231, test cases 1, 2 and 6: a 20-byte key, a key shorter than the
    /// output, and a 131-byte key that has to be hashed down to a block first.
    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        assert_eq!(
            encode_hex(&hmac_sha256(&[0x0b; 20], &[b"Hi There"])),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            encode_hex(&hmac_sha256(
                b"Jefe",
                &[b"what do ya want ", b"for nothing?"]
            )),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            encode_hex(&hmac_sha256(
                &[0xaa; 131],
                &[b"Test Using Larger Than Block-Size Key - Hash Key First"]
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }
}
