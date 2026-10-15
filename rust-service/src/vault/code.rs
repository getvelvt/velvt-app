//! The one-time setup code and the handoff payload it opens (design §5.3,
//! §7.1).
//!
//! The code carries a 100-bit secret `s` as 20 Crockford base32 characters
//! plus one Luhn mod 32 check character, shown as `K7PQ-M2XD-9HTW-4RZN-8CJ5X`.
//! It travels from one screen to a keyboard and never through velvt-core, so
//! no PAKE is needed: the server sees only `handoff_id` and a payload sealed
//! under a key that only `s` yields.
//!
//! `s` enters HKDF as 13 bytes, big-endian, whose top four bits are zero.

use std::fmt;

use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::crypto::{
    aead_open, aead_seal, decode_hex, encode_hex, hkdf_sha256, CryptoError, KeyId, UserId,
    VaultKey, KEY_LENGTH, NONCE_LENGTH, TAG_LENGTH,
};
use super::doc::SyncDeviceId;
use super::wire::Writer;

/// Crockford's base32 alphabet: no `I`, `L`, `O` or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const BASE: u32 = 32;
const SECRET_BYTES: usize = 13;
const HANDOFF_INFO: &[u8] = b"velvt-vault-handoff-v1";
const HANDOFF_AAD_TAG: &[u8] = b"velvt-vault-handoff-v1\0";
const HANDOFF_PLAIN_VERSION: u8 = 0x01;
const HANDOFF_PLAIN_LENGTH: usize = 1 + KeyId::LENGTH + KEY_LENGTH + 16;

/// Bits of secret in a setup code.
pub const SECRET_BITS: u32 = 100;
/// Characters a code carries, check character included; separators are not
/// counted.
pub const CODE_LENGTH: usize = 21;
const PAYLOAD_LENGTH: usize = CODE_LENGTH - 1;
/// `nonce ‖ ciphertext ‖ tag` over `0x01 ‖ key_id ‖ K ‖ issuer_sync_device_id`:
/// 85 bytes, inside velvt-core's 256-byte limit.
pub const HANDOFF_PAYLOAD_LENGTH: usize = NONCE_LENGTH + HANDOFF_PLAIN_LENGTH + TAG_LENGTH;

/// Why typed text is not a setup code. Decided on this Mac, before any network
/// call (J3 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    #[error("a character is not in the code alphabet")]
    InvalidCharacter,
    #[error("the code has the wrong number of characters")]
    WrongLength,
    #[error("the check character does not match")]
    CheckFailed,
}

/// Why a handoff payload did not give a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HandoffError {
    #[error("the payload is not the handoff payload length")]
    Length,
    #[error("the payload did not open under this code and account")]
    Open,
    #[error("the payload's version is not one this Mac reads")]
    Version,
    #[error("the payload's key id is not the id of the key inside it")]
    KeyIdMismatch,
    #[error("the payload's issuer is not a sync device id")]
    Issuer,
    #[error("a key derivation failed")]
    Crypto(#[from] CryptoError),
}

/// A setup code's secret `s`. Zeroized on drop, redacted in `Debug`, and not
/// serializable; [`SetupCode::display`] is the only way out.
pub struct SetupCode {
    secret: Zeroizing<u128>,
}

impl SetupCode {
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = Zeroizing::new([0_u8; SECRET_BYTES]);
        SystemRandom::new()
            .fill(&mut bytes[..])
            .map_err(|_| CryptoError::Random)?;
        bytes[0] &= 0x0f;
        Ok(Self::from_masked(&bytes))
    }

    /// `s` as 13 big-endian bytes. `None` if any of the top four bits is set,
    /// since those are not part of a 100-bit secret.
    pub fn from_secret_bytes(bytes: &[u8; SECRET_BYTES]) -> Option<Self> {
        (bytes[0] & 0xf0 == 0).then(|| Self::from_masked(bytes))
    }

    fn from_masked(bytes: &[u8; SECRET_BYTES]) -> Self {
        let mut wide = Zeroizing::new([0_u8; 16]);
        wide[16 - SECRET_BYTES..].copy_from_slice(bytes);
        Self {
            secret: Zeroizing::new(u128::from_be_bytes(*wide)),
        }
    }

    fn secret_bytes(&self) -> Zeroizing<[u8; SECRET_BYTES]> {
        let wide = Zeroizing::new(self.secret.to_be_bytes());
        let mut bytes = Zeroizing::new([0_u8; SECRET_BYTES]);
        bytes.copy_from_slice(&wide[16 - SECRET_BYTES..]);
        bytes
    }

    /// Reads what the person typed.
    ///
    /// Case is ignored; `O` reads as `0` and `I` and `L` as `1`; hyphens,
    /// dashes and white space are ignored; `U` and anything else outside the
    /// alphabet is refused. The check character then catches every single
    /// wrong character and every swap of two neighbours except `0` with `Z`.
    pub fn parse(typed: &str) -> Result<Self, CodeError> {
        let mut values = Zeroizing::new([0_u8; CODE_LENGTH]);
        let mut count = 0_usize;
        for character in typed.chars() {
            if is_separator(character) {
                continue;
            }
            let value = code_value(character).ok_or(CodeError::InvalidCharacter)?;
            if count == CODE_LENGTH {
                return Err(CodeError::WrongLength);
            }
            values[count] = value;
            count += 1;
        }
        if count != CODE_LENGTH {
            return Err(CodeError::WrongLength);
        }
        if !luhn_is_valid(&values[..]) {
            return Err(CodeError::CheckFailed);
        }
        let mut secret = Zeroizing::new(0_u128);
        for &value in &values[..PAYLOAD_LENGTH] {
            *secret = (*secret << 5) | u128::from(value);
        }
        Ok(Self { secret })
    }

    /// The code as the sheet shows it, `XXXX-XXXX-XXXX-XXXX-XXXXX`, check
    /// character last.
    pub fn display(&self) -> Zeroizing<String> {
        let mut values = Zeroizing::new([0_u8; CODE_LENGTH]);
        for (index, value) in values[..PAYLOAD_LENGTH].iter_mut().enumerate() {
            let shift = 5 * (PAYLOAD_LENGTH - 1 - index);
            *value = ((*self.secret >> shift) & 0x1f) as u8;
        }
        values[PAYLOAD_LENGTH] = luhn_check_value(&values[..PAYLOAD_LENGTH]);
        let mut shown = Zeroizing::new(String::with_capacity(CODE_LENGTH + 4));
        for (index, &value) in values.iter().enumerate() {
            if index > 0 && index.is_multiple_of(4) && index < PAYLOAD_LENGTH {
                shown.push('-');
            }
            shown.push(char::from(ALPHABET[usize::from(value)]));
        }
        shown
    }

    /// `handoff_id ‖ K_wrap = HKDF-SHA256(ikm = s, salt = ∅,
    /// info = "velvt-vault-handoff-v1", L = 48)`.
    fn handoff_keys(&self) -> Result<(HandoffId, Zeroizing<[u8; KEY_LENGTH]>), CryptoError> {
        let okm = hkdf_sha256::<{ HandoffId::LENGTH + KEY_LENGTH }>(
            &self.secret_bytes()[..],
            HANDOFF_INFO,
        )?;
        let mut id = [0_u8; HandoffId::LENGTH];
        id.copy_from_slice(&okm[..HandoffId::LENGTH]);
        let mut wrap = Zeroizing::new([0_u8; KEY_LENGTH]);
        wrap.copy_from_slice(&okm[HandoffId::LENGTH..]);
        Ok((HandoffId(id), wrap))
    }

    /// The mailbox id velvt-core stores the payload under. Meaningless without
    /// the code.
    pub fn handoff_id(&self) -> Result<HandoffId, CryptoError> {
        self.handoff_keys().map(|(id, _)| id)
    }
}

impl fmt::Debug for SetupCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SetupCode([redacted])")
    }
}

/// `handoff_id`: 16 bytes derived from the code; 32 lowercase hex characters
/// on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandoffId([u8; 16]);

impl HandoffId {
    pub const LENGTH: usize = 16;

    pub fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }

    pub fn from_hex(encoded: &str) -> Option<Self> {
        decode_hex::<{ Self::LENGTH }>(encoded).map(Self)
    }
}

impl fmt::Debug for HandoffId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "HandoffId({})", self.to_hex())
    }
}

/// What a handoff payload carries.
#[derive(Debug)]
pub struct HandoffContents {
    pub key: VaultKey,
    pub key_id: KeyId,
    pub issuer_sync_device_id: SyncDeviceId,
}

/// Seals `K` for the Mac that will type `code` (§5.3):
/// `nonce ‖ ChaCha20-Poly1305(K_wrap, nonce, aad, 0x01 ‖ key_id ‖ K ‖ issuer)`
/// with `aad = "velvt-vault-handoff-v1\0" ‖ lp(user_id) ‖ handoff_id`.
pub fn seal_handoff(
    code: &SetupCode,
    user_id: &UserId,
    key: &VaultKey,
    issuer: SyncDeviceId,
) -> Result<Vec<u8>, CryptoError> {
    let mut nonce = [0_u8; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| CryptoError::Random)?;
    seal_handoff_with_nonce(code, user_id, key, issuer, nonce)
}

pub(crate) fn seal_handoff_with_nonce(
    code: &SetupCode,
    user_id: &UserId,
    key: &VaultKey,
    issuer: SyncDeviceId,
    nonce: [u8; NONCE_LENGTH],
) -> Result<Vec<u8>, CryptoError> {
    let (handoff_id, wrap) = code.handoff_keys()?;
    let mut plain = Zeroizing::new(Vec::with_capacity(HANDOFF_PLAIN_LENGTH));
    plain.push(HANDOFF_PLAIN_VERSION);
    plain.extend_from_slice(key.key_id().as_bytes());
    plain.extend_from_slice(key.secret_bytes());
    plain.extend_from_slice(issuer.as_bytes());
    aead_seal(&wrap, nonce, &handoff_aad(user_id, handoff_id), &plain)
}

/// Opens a claimed payload with the typed code and checks it: version 1, and a
/// key id equal to the id of the key inside. Every failure leaves nothing
/// stored ("That code didn't work.").
pub fn open_handoff(
    code: &SetupCode,
    user_id: &UserId,
    payload: &[u8],
) -> Result<HandoffContents, HandoffError> {
    if payload.len() != HANDOFF_PAYLOAD_LENGTH {
        return Err(HandoffError::Length);
    }
    let (handoff_id, wrap) = code.handoff_keys()?;
    let plain = aead_open(&wrap, &handoff_aad(user_id, handoff_id), payload)
        .map_err(|_| HandoffError::Open)?;
    parse_handoff_plaintext(&plain)
}

/// The opened payload's checks, on their own so they can be fuzzed without
/// the code.
pub fn parse_handoff_plaintext(plain: &[u8]) -> Result<HandoffContents, HandoffError> {
    if plain.len() != HANDOFF_PLAIN_LENGTH {
        return Err(HandoffError::Length);
    }
    if plain[0] != HANDOFF_PLAIN_VERSION {
        return Err(HandoffError::Version);
    }
    let (named, rest) = plain[1..].split_at(KeyId::LENGTH);
    let (secret, issuer) = rest.split_at(KEY_LENGTH);
    let key = VaultKey::from_slice(secret)?;
    if !bool::from(key.key_id().as_bytes()[..].ct_eq(named)) {
        return Err(HandoffError::KeyIdMismatch);
    }
    let issuer: [u8; 16] = issuer.try_into().map_err(|_| HandoffError::Length)?;
    let issuer_sync_device_id = SyncDeviceId::from_bytes(issuer).ok_or(HandoffError::Issuer)?;
    Ok(HandoffContents {
        key_id: key.key_id(),
        key,
        issuer_sync_device_id,
    })
}

fn handoff_aad(user_id: &UserId, handoff_id: HandoffId) -> Vec<u8> {
    Writer::with_tag(HANDOFF_AAD_TAG)
        .lp(user_id.as_str().as_bytes())
        .bytes(handoff_id.as_bytes())
        .finish()
}

fn is_separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '-' | '\u{2010}'..='\u{2015}' | '\u{2212}')
}

fn code_value(character: char) -> Option<u8> {
    let normalized = match character.to_ascii_uppercase() {
        'O' => '0',
        'I' | 'L' => '1',
        other => other,
    };
    if !normalized.is_ascii() {
        return None;
    }
    let byte = normalized as u8;
    ALPHABET
        .iter()
        .position(|&candidate| candidate == byte)
        .and_then(|position| u8::try_from(position).ok())
}

/// Luhn mod N (N = 32): the check value that makes the whole code sum to zero.
fn luhn_check_value(payload: &[u8]) -> u8 {
    let sum = luhn_sum(payload, 2);
    ((BASE - sum % BASE) % BASE) as u8
}

fn luhn_is_valid(code: &[u8]) -> bool {
    luhn_sum(code, 1).is_multiple_of(BASE)
}

/// From the right: each value times a factor that alternates starting at
/// `first_factor`, the product's base-32 digits added.
fn luhn_sum(values: &[u8], first_factor: u32) -> u32 {
    let mut factor = first_factor;
    let mut sum = 0_u32;
    for &value in values.iter().rev() {
        let addend = factor * u32::from(value);
        sum += addend / BASE + addend % BASE;
        factor = 3 - factor;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::{
        luhn_check_value, open_handoff, parse_handoff_plaintext, seal_handoff,
        seal_handoff_with_nonce, CodeError, HandoffError, SetupCode, ALPHABET, CODE_LENGTH,
        HANDOFF_PAYLOAD_LENGTH, PAYLOAD_LENGTH,
    };
    use crate::vault::crypto::KeyId;
    use crate::vault::test_support::{device, other_key, other_user, test_key, user, Rng};

    fn code_from(rng: &mut Rng) -> SetupCode {
        let mut bytes = rng.bytes::<13>();
        bytes[0] &= 0x0f;
        SetupCode::from_secret_bytes(&bytes).unwrap()
    }

    /// The 21 alphabet characters of a displayed code, separators removed.
    fn characters(code: &SetupCode) -> Vec<char> {
        code.display().chars().filter(|&c| c != '-').collect()
    }

    fn same(left: &SetupCode, right: &SetupCode) -> bool {
        left.display() == right.display()
    }

    #[test]
    fn a_code_is_shown_in_five_groups_and_reads_back() {
        let mut rng = Rng::new(1);
        for _ in 0..500 {
            let code = code_from(&mut rng);
            let shown = code.display();
            let groups: Vec<usize> = shown.split('-').map(str::len).collect();
            assert_eq!(groups, [4, 4, 4, 4, 5], "{}", shown.as_str());
            assert!(shown
                .bytes()
                .all(|byte| byte == b'-' || ALPHABET.contains(&byte)));
            assert!(same(&SetupCode::parse(&shown).unwrap(), &code));
        }
        let generated = SetupCode::generate().unwrap();
        assert!(same(
            &SetupCode::parse(&generated.display()).unwrap(),
            &generated
        ));
    }

    #[test]
    fn the_secret_is_one_hundred_bits() {
        assert!(SetupCode::from_secret_bytes(&[0x0f; 13]).is_some());
        assert!(SetupCode::from_secret_bytes(&[0x10; 13]).is_none());
        let all_ones = SetupCode::from_secret_bytes(&{
            let mut bytes = [0xff; 13];
            bytes[0] = 0x0f;
            bytes
        })
        .unwrap();
        assert!(all_ones.display().starts_with("ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ"));
        assert!(SetupCode::from_secret_bytes(&[0; 13])
            .unwrap()
            .display()
            .starts_with("0000-0000-0000-0000-0000"));
    }

    #[test]
    fn typing_is_forgiving_about_case_separators_and_look_alike_letters() {
        let code = code_from(&mut Rng::new(2));
        let shown = code.display().to_string();
        let variants = [
            shown.to_lowercase(),
            shown.replace('-', ""),
            shown.replace('-', " "),
            shown.replace('-', "\u{2013}"),
            format!("  {}\n", shown.replace('-', " - ")),
            shown.replace('0', "O").replace('1', "l"),
            shown.replace('0', "o").replace('1', "I"),
        ];
        for variant in variants {
            let parsed =
                SetupCode::parse(&variant).unwrap_or_else(|error| panic!("{variant:?}: {error}"));
            assert!(same(&parsed, &code), "{variant:?}");
        }
    }

    #[test]
    fn text_that_is_not_a_code_is_refused_before_any_network_call() {
        let shown = code_from(&mut Rng::new(3)).display().to_string();
        let first = shown.chars().next().unwrap();
        let cases = [
            (shown.replacen(first, "U", 1), CodeError::InvalidCharacter),
            (shown.replacen(first, "u", 1), CodeError::InvalidCharacter),
            (shown.replacen(first, "*", 1), CodeError::InvalidCharacter),
            (
                shown.replacen(first, "\u{0410}", 1),
                CodeError::InvalidCharacter,
            ),
            (shown[..shown.len() - 1].to_owned(), CodeError::WrongLength),
            (format!("{shown}0"), CodeError::WrongLength),
            (String::new(), CodeError::WrongLength),
            ("-- --".to_owned(), CodeError::WrongLength),
            ("0".repeat(10_000), CodeError::WrongLength),
        ];
        for (typed, expected) in cases {
            assert_eq!(SetupCode::parse(&typed).err(), Some(expected), "{typed:?}");
        }
    }

    /// Every single wrong character, in every position of a sample of codes.
    #[test]
    fn the_check_character_catches_every_single_substitution() {
        let mut rng = Rng::new(4);
        for _ in 0..60 {
            let original = characters(&code_from(&mut rng));
            for position in 0..CODE_LENGTH {
                for &replacement in ALPHABET {
                    let replacement = char::from(replacement);
                    if replacement == original[position] {
                        continue;
                    }
                    let mut typed = original.clone();
                    typed[position] = replacement;
                    let typed: String = typed.into_iter().collect();
                    assert_eq!(
                        SetupCode::parse(&typed).err(),
                        Some(CodeError::CheckFailed),
                        "{typed}"
                    );
                }
            }
        }
    }

    /// Luhn mod 32 misses exactly one kind of neighbour swap, `0` with `Z`:
    /// a swap goes unseen when doubling (digits summed in base 32) moves both
    /// values by the same amount modulo 32, and only 0 (by 0) and 31 (by 32)
    /// share one. Checked over every pair of characters at every position.
    #[test]
    fn the_check_character_catches_every_neighbour_swap_but_zero_with_z() {
        let base = characters(&code_from(&mut Rng::new(5)));
        let mut undetected = 0;
        let mut swaps = 0;
        for position in 0..PAYLOAD_LENGTH - 1 {
            for &left in ALPHABET {
                for &right in ALPHABET {
                    if left == right {
                        continue;
                    }
                    let mut values: Vec<u8> = base[..PAYLOAD_LENGTH]
                        .iter()
                        .map(|&c| ALPHABET.iter().position(|&a| char::from(a) == c).unwrap() as u8)
                        .collect();
                    values[position] = ALPHABET.iter().position(|&a| a == left).unwrap() as u8;
                    values[position + 1] = ALPHABET.iter().position(|&a| a == right).unwrap() as u8;
                    values.push(luhn_check_value(&values));
                    values.swap(position, position + 1);
                    let typed: String = values
                        .iter()
                        .map(|&value| char::from(ALPHABET[usize::from(value)]))
                        .collect();
                    swaps += 1;
                    let zero_and_z = matches!((left, right), (b'0', b'Z') | (b'Z', b'0'));
                    match SetupCode::parse(&typed) {
                        Ok(_) => {
                            assert!(zero_and_z, "{typed} swap undetected");
                            undetected += 1;
                        }
                        Err(error) => {
                            assert!(!zero_and_z, "{typed} 0/Z swap detected?");
                            assert_eq!(error, CodeError::CheckFailed);
                        }
                    }
                }
            }
        }
        assert_eq!(undetected, 2 * (PAYLOAD_LENGTH - 1));
        assert_eq!(swaps, 32 * 31 * (PAYLOAD_LENGTH - 1));
    }

    /// Swaps that move the check character itself, over a sample of codes.
    #[test]
    fn a_swap_with_the_check_character_is_caught_unless_it_is_zero_with_z() {
        let mut rng = Rng::new(6);
        for _ in 0..2_000 {
            let mut typed = characters(&code_from(&mut rng));
            let (left, right) = (typed[CODE_LENGTH - 2], typed[CODE_LENGTH - 1]);
            if left == right {
                continue;
            }
            typed.swap(CODE_LENGTH - 2, CODE_LENGTH - 1);
            let typed: String = typed.into_iter().collect();
            let zero_and_z = matches!((left, right), ('0', 'Z') | ('Z', '0'));
            assert_eq!(SetupCode::parse(&typed).is_ok(), zero_and_z, "{typed}");
        }
    }

    #[test]
    fn a_handoff_opens_with_the_typed_code_for_the_same_account() {
        let (key, code, issuer) = (test_key(), code_from(&mut Rng::new(7)), device(3));
        let payload = seal_handoff(&code, &user(), &key, issuer).unwrap();
        assert_eq!(payload.len(), HANDOFF_PAYLOAD_LENGTH);
        assert!(payload.len() <= 256);

        let typed = SetupCode::parse(&code.display().to_lowercase()).unwrap();
        let opened = open_handoff(&typed, &user(), &payload).unwrap();

        assert_eq!(opened.key, key);
        assert_eq!(opened.key_id, key.key_id());
        assert_eq!(opened.issuer_sync_device_id, issuer);
        assert_eq!(typed.handoff_id().unwrap(), code.handoff_id().unwrap());
    }

    #[test]
    fn a_handoff_does_not_open_with_another_code_account_or_byte() {
        let mut rng = Rng::new(8);
        let (key, code) = (test_key(), code_from(&mut rng));
        let payload = seal_handoff(&code, &user(), &key, device(3)).unwrap();

        assert_eq!(
            open_handoff(&code_from(&mut rng), &user(), &payload).err(),
            Some(HandoffError::Open)
        );
        assert_eq!(
            open_handoff(&code, &other_user(), &payload).err(),
            Some(HandoffError::Open)
        );
        for index in 0..payload.len() {
            let mut changed = payload.clone();
            changed[index] ^= 0x01;
            assert_eq!(
                open_handoff(&code, &user(), &changed).err(),
                Some(HandoffError::Open),
                "byte {index}"
            );
        }
        assert_eq!(
            open_handoff(&code, &user(), &payload[1..]).err(),
            Some(HandoffError::Length)
        );
    }

    /// What a server that substituted its own payload gets: nothing, because it
    /// cannot seal under a key only the code yields.
    #[test]
    fn a_substituted_handoff_payload_is_refused() {
        let mut rng = Rng::new(9);
        let code = code_from(&mut rng);
        let attacker_code = code_from(&mut rng);
        let substituted = seal_handoff(&attacker_code, &user(), &other_key(), device(4)).unwrap();
        assert_eq!(
            open_handoff(&code, &user(), &substituted).err(),
            Some(HandoffError::Open)
        );
    }

    #[test]
    fn an_opened_payload_must_be_version_one_and_name_its_own_key() {
        let key = test_key();
        let mut plain = vec![0x01];
        plain.extend_from_slice(key.key_id().as_bytes());
        plain.extend_from_slice(key.secret_bytes());
        plain.extend_from_slice(device(5).as_bytes());
        assert!(parse_handoff_plaintext(&plain).is_ok());

        let mut version = plain.clone();
        version[0] = 0x02;
        assert_eq!(
            parse_handoff_plaintext(&version).err(),
            Some(HandoffError::Version)
        );

        let mut named = plain.clone();
        named[1..1 + KeyId::LENGTH].copy_from_slice(other_key().key_id().as_bytes());
        assert_eq!(
            parse_handoff_plaintext(&named).err(),
            Some(HandoffError::KeyIdMismatch)
        );

        let mut issuer = plain.clone();
        let last = issuer.len() - 16;
        issuer[last..].copy_from_slice(&[0; 16]);
        assert_eq!(
            parse_handoff_plaintext(&issuer).err(),
            Some(HandoffError::Issuer)
        );

        assert_eq!(
            parse_handoff_plaintext(&plain[..plain.len() - 1]).err(),
            Some(HandoffError::Length)
        );
    }

    #[test]
    fn the_payload_is_fixed_by_the_code_account_key_issuer_and_nonce() {
        let code = SetupCode::from_secret_bytes(&[3; 13]).unwrap();
        let first =
            seal_handoff_with_nonce(&code, &user(), &test_key(), device(3), [1; 12]).unwrap();
        let second =
            seal_handoff_with_nonce(&code, &user(), &test_key(), device(3), [1; 12]).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..12], &[1; 12]);
    }

    #[test]
    fn a_setup_code_never_shows_itself_in_debug() {
        let code = code_from(&mut Rng::new(10));
        let shown = format!("{code:?}");
        assert_eq!(shown, "SetupCode([redacted])");
        let handoff = open_handoff(
            &code,
            &user(),
            &seal_handoff(&code, &user(), &test_key(), device(3)).unwrap(),
        )
        .unwrap();
        let shown = format!("{handoff:?}");
        assert!(!shown.contains(test_key().to_base64().as_str()), "{shown}");
    }
}
