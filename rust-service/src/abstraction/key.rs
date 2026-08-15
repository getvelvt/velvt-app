use sha2::{Digest, Sha256};

const KEY_DOMAIN: &[u8] = b"velvt:abstraction-key:v1";
/// Separate domain so an app-scoped key can never collide with a
/// (app, title) key, even for an app whose title is empty.
const APP_KEY_DOMAIN: &[u8] = b"velvt:abstraction-app-key:v1";

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
    use super::RawKey;

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
}
