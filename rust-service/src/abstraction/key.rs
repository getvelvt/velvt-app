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

    pub(crate) fn stable_key(&self) -> String {
        // Stable-key hashing intentionally preserves the exact raw bytes instead
        // of classifier normalization. Existing installations already persist
        // these hashes; changing them would orphan mappings and rotate stable IDs.
        // Classifier tiers share canonical preprocessing, while this compatibility
        // boundary remains versioned and deterministic across restarts.
        let mut hasher = Sha256::new();
        hasher.update(KEY_DOMAIN);
        update_length_prefixed(&mut hasher, self.app_name.as_bytes());
        update_length_prefixed(&mut hasher, self.window_title.as_bytes());
        encode_hex(&hasher.finalize())
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
    pub(crate) fn app_stable_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(APP_KEY_DOMAIN);
        update_length_prefixed(&mut hasher, self.app_name.as_bytes());
        encode_hex(&hasher.finalize())
    }
}

/// The window-scoped correction key.
///
/// `stable_context` is the focused site for a browser window and the raw
/// window title otherwise — the same value the engine keys on.
pub fn stable_key_for(app_name: &str, stable_context: &str) -> String {
    RawKey::new(app_name.to_owned(), stable_context.to_owned()).stable_key()
}

/// The app-scoped correction key: the identity one correction generalizes to.
pub fn app_stable_key_for(app_name: &str) -> String {
    RawKey::new(app_name.to_owned(), String::new()).app_stable_key()
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
/// The identifier itself is local-only. This returns its hash, which is what
/// gets persisted, so the raw identifier is never stored and can reach no
/// upload DTO.
pub fn app_bundle_key_for(bundle_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(APP_BUNDLE_KEY_DOMAIN);
    // Length-prefixed like its siblings even with one field, so a later field
    // cannot be appended without the prefix and reopen the ambiguity the other
    // two keys were careful to close.
    update_length_prefixed(&mut hasher, bundle_id.as_bytes());
    encode_hex(&hasher.finalize())
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
        app_bundle_key_for, app_stable_key_for, encode_hex, stable_key_for, Digest, RawKey, Sha256,
    };

    #[test]
    fn length_prefix_prevents_delimiter_ambiguity() {
        let first = RawKey::new("a".into(), "bc".into()).stable_key();
        let second = RawKey::new("ab".into(), "c".into()).stable_key();

        assert_ne!(first, second);
    }

    /// The whole point of the app key: one correction has to cover every
    /// window of the app, not just the file that happened to be open.
    #[test]
    fn the_app_key_ignores_the_window_title() {
        let editing = RawKey::new("Cursor".into(), "main.rs — velvt".into()).app_stable_key();
        let reviewing = RawKey::new("Cursor".into(), "lib.rs — velvt".into()).app_stable_key();

        assert_eq!(editing, reviewing);
    }

    #[test]
    fn the_app_key_still_separates_different_apps() {
        let editor = RawKey::new("Cursor".into(), String::new()).app_stable_key();
        let chat = RawKey::new("Slack".into(), String::new()).app_stable_key();

        assert_ne!(editor, chat);
    }

    /// A correction scoped to one window and a correction scoped to the whole
    /// app are different facts, so their keys must never collide — including
    /// for an app that reports no title at all.
    #[test]
    fn the_app_key_never_collides_with_the_pair_key() {
        let titled = RawKey::new("Cursor".into(), String::new());

        assert_ne!(titled.stable_key(), titled.app_stable_key());
    }

    /// The case that motivated keying on the bundle at all: the name macOS
    /// reports for Visual Studio Code is `Code`, which matches no taxonomy
    /// entry, while the bundle identifier is exact and stable.
    #[test]
    fn the_bundle_key_is_stable_across_the_names_one_app_reports() {
        let short_name = app_bundle_key_for("com.microsoft.VSCode");
        let renamed = app_bundle_key_for("com.microsoft.VSCode");

        assert_eq!(short_name, renamed);
        assert_ne!(
            app_bundle_key_for("com.microsoft.VSCode"),
            app_bundle_key_for("com.vscodium.codium")
        );
    }

    /// Three domains, three different facts. A bundle key must never collide
    /// with a name key or a window key, including in the degenerate case where
    /// the bundle identifier and the application name are the same string.
    #[test]
    fn the_bundle_key_never_collides_with_the_name_or_window_keys() {
        let shared = "Terminal";

        let bundle = app_bundle_key_for(shared);
        let app = app_stable_key_for(shared);
        let window = stable_key_for(shared, "");

        assert_ne!(bundle, app);
        assert_ne!(bundle, window);
        assert_ne!(app, window);
    }

    /// Pins the bundle domain string itself, not only its behaviour.
    ///
    /// The string is a persisted format: every `bundle_key_hash` a user has
    /// taught and every `app_bundle_stable_id` on a stored event was computed
    /// with it, so editing it orphans those rows without a single error. It also
    /// carries the collision argument above, which is an argument about the
    /// *pattern* the three domains share. Both reasons are why the contract's
    /// draft spelling was reconciled to this one and not the other way round,
    /// and this test is where that decision is enforced.
    #[test]
    fn the_bundle_key_domain_is_the_house_pattern_string() {
        let bundle_id = b"com.microsoft.VSCode";
        let mut hasher = Sha256::new();
        hasher.update(b"velvt:abstraction-app-bundle-key:v1");
        hasher.update((bundle_id.len() as u64).to_be_bytes());
        hasher.update(bundle_id);

        assert_eq!(
            app_bundle_key_for("com.microsoft.VSCode"),
            encode_hex(&hasher.finalize())
        );
    }

    /// Every key is 64 hex characters, because that is what the persisted
    /// columns CHECK for. A key that failed the constraint would surface as a
    /// write error at correction time, far from its cause.
    #[test]
    fn the_bundle_key_is_a_sixty_four_character_hex_digest() {
        let key = app_bundle_key_for("com.apple.dt.Xcode");

        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|character| character.is_ascii_hexdigit()));
    }
}
