use sha2::{Digest, Sha256};

const KEY_DOMAIN: &[u8] = b"velvt:abstraction-key:v1";

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
        // SHA-256 hashes a versioned domain plus length-prefixed raw UTF-8 fields.
        // Length prefixes prevent delimiter collisions; exact bytes and the fixed
        // domain keep keys deterministic across service and binary restarts.
        let mut hasher = Sha256::new();
        hasher.update(KEY_DOMAIN);
        update_length_prefixed(&mut hasher, self.app_name.as_bytes());
        update_length_prefixed(&mut hasher, self.window_title.as_bytes());
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
}
