//! The host binding (design J8, T15): which Mac a `vault_state` row was made
//! on.
//!
//! Migration Assistant and a restored backup copy the database, and with it
//! this Mac's `sync_device_id`. Two Macs sharing one would overwrite each
//! other's entry in the document. So each row keeps `HMAC-SHA256(salt, host
//! UUID)` under a random salt of its own, and a row whose binding does not
//! match the Mac it is read on takes a new `sync_device_id` and turns sync
//! off there. Only the HMAC is stored, and it never leaves the Mac.

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

use crate::vault::crypto::CryptoError;

/// The Mac's host UUID, the value `gethostuuid(2)` returns.
pub trait HostIdentity: Send + Sync {
    /// `None` when it cannot be read. The binding is then not checked: a
    /// Mac whose UUID is unreadable is not taken for a clone.
    fn host_uuid(&self) -> Option<[u8; 16]>;
}

/// The platform UUID from the I/O Registry (`IOPlatformUUID`), which is what
/// `gethostuuid(2)` returns on macOS. Read with `/usr/sbin/ioreg`, so the
/// service needs no FFI for it, and read only when sync needs it.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacHostIdentity;

impl HostIdentity for MacHostIdentity {
    fn host_uuid(&self) -> Option<[u8; 16]> {
        let output = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_platform_uuid(&String::from_utf8_lossy(&output.stdout))
    }
}

/// A fixed answer, for tests and for a Mac the binding should not judge.
#[derive(Debug, Clone, Copy)]
pub struct FixedHostIdentity(pub Option<[u8; 16]>);

impl HostIdentity for FixedHostIdentity {
    fn host_uuid(&self) -> Option<[u8; 16]> {
        self.0
    }
}

/// `"IOPlatformUUID" = "564D…"` in `ioreg`'s output.
pub fn parse_platform_uuid(ioreg: &str) -> Option<[u8; 16]> {
    ioreg.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "\"IOPlatformUUID\"" {
            return None;
        }
        let value = value.trim().trim_matches('"');
        uuid::Uuid::try_parse(value)
            .ok()
            .map(|uuid| *uuid.as_bytes())
    })
}

/// A new salt and the binding of `host` under it.
pub fn new_binding(host: &[u8; 16]) -> Result<([u8; 32], [u8; 32]), CryptoError> {
    let mut salt = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| CryptoError::Random)?;
    Ok((salt, binding(&salt, host)))
}

pub fn binding(salt: &[u8; 32], host: &[u8; 16]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, salt);
    let mut out = [0_u8; 32];
    out.copy_from_slice(hmac::sign(&key, host).as_ref());
    out
}

/// Whether `stored` is `host`'s binding under `salt`, compared in constant
/// time.
pub fn matches(salt: &[u8], stored: &[u8], host: &[u8; 16]) -> bool {
    hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, salt), host, stored).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{binding, matches, new_binding, parse_platform_uuid};

    #[test]
    fn the_platform_uuid_is_read_from_ioreg_output() {
        let output = r#"+-o J316sAP  <class IOPlatformExpertDevice, id 0x100000234>
    {
      "IOPlatformSerialNumber" = "C02XXXXXXX"
      "IOPlatformUUID" = "564D1A2B-3C4D-5E6F-7081-92A3B4C5D6E7"
      "model" = <"MacBookPro18,3">
    }"#;
        assert_eq!(
            parse_platform_uuid(output),
            Some([
                0x56, 0x4d, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e, 0x6f, 0x70, 0x81, 0x92, 0xa3, 0xb4, 0xc5,
                0xd6, 0xe7
            ])
        );
        assert_eq!(parse_platform_uuid("\"IOPlatformUUID\" = \"nope\""), None);
        assert_eq!(parse_platform_uuid(""), None);
    }

    #[test]
    fn a_binding_matches_only_its_own_host_under_its_own_salt() {
        let host = [7; 16];
        let (salt, stored) = new_binding(&host).unwrap();
        assert!(matches(&salt, &stored, &host));
        assert!(!matches(&salt, &stored, &[8; 16]));
        assert!(!matches(&[0; 32], &stored, &host));
        assert_ne!(
            new_binding(&host).unwrap().1,
            stored,
            "a fresh salt each time"
        );
        assert_eq!(binding(&salt, &host), stored);
    }
}
