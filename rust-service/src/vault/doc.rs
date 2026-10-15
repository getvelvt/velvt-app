//! The `prefs` document, schema 1 (design §4.1), and how it is sealed and
//! opened.
//!
//! Plaintext exists only in Rust memory. A schema-1 document is parsed
//! strictly: unknown fields, duplicate keys, values outside their enums and
//! ranges, and more than the device and rule caps are all rejected, and a
//! document that fails is never overwritten. A higher schema is read only for
//! the two fields that apply at once (Mac names and the daily-insight switch)
//! and can never be written back.
//!
//! The only free text is a Mac's `name`. Rules are keyed by person keys, never
//! by a site, bundle id or app name.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::crypto::{
    CryptoError, DocumentContext, PersonKey, PersonKeyKind, PersonSalt, VaultKey,
    MAX_DOCUMENT_JSON_LENGTH,
};
use super::merge::Hlc;

pub const SCHEMA_V1: u16 = 1;
pub const MAX_LIVE_DEVICES: usize = 32;
pub const MAX_DEVICE_ENTRIES: usize = 256;
pub const MAX_LIVE_RULES: usize = 2_000;
pub const MAX_NAME_CHARACTERS: usize = 64;
pub const LAST_LOCAL_MINUTE: u16 = 1_439;

/// Why a document is not one this Mac may read or write.
///
/// Carries positions and field names, never document content, so it is safe
/// to log: a malformed name, for instance, is reported by where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DocumentError {
    #[error("the document is not valid JSON for its schema (line {line}, column {column})")]
    Malformed { line: usize, column: usize },
    #[error("schema {0} is not a schema this Mac reads")]
    UnsupportedSchema(u16),
    #[error("the document's schema does not match the schema it was sealed under")]
    SchemaMismatch,
    #[error("more than {MAX_LIVE_DEVICES} devices are live")]
    TooManyLiveDevices,
    #[error("more than {MAX_DEVICE_ENTRIES} device entries")]
    TooManyDeviceEntries,
    #[error("more than {MAX_LIVE_RULES} rules are live")]
    TooManyLiveRules,
    #[error("the document is too large to seal")]
    TooLarge,
    #[error("a field is out of range: {0}")]
    OutOfRange(&'static str),
}

impl From<serde_json::Error> for DocumentError {
    fn from(error: serde_json::Error) -> Self {
        Self::Malformed {
            line: error.line(),
            column: error.column(),
        }
    }
}

/// Why sealing or opening a document failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VaultDocError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Document(#[from] DocumentError),
}

/// The `prefs` document, schema 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefsDocument {
    pub schema: u16,
    pub person_salt: PersonSalt,
    #[serde(deserialize_with = "unique_map")]
    pub devices: BTreeMap<SyncDeviceId, DeviceEntry>,
    #[serde(deserialize_with = "unique_map")]
    pub rules: BTreeMap<RuleKey, RuleRegister>,
    pub settings: Settings,
}

/// One Mac's entry. `joined_via` and `joined_at_ms` are written once, by the
/// Mac itself; every other field is a register any syncing Mac may write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceEntry {
    pub joined_via: JoinedVia,
    pub joined_at_ms: u64,
    pub server_device_id: Register<ServerDeviceId>,
    pub name: Register<DeviceName>,
    pub notify_daily_insight: Register<bool>,
    pub removed: Register<bool>,
}

/// A last-writer-wins register: the value with the higher HLC wins a merge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register<T> {
    pub value: T,
    pub hlc: Hlc,
}

/// A category rule: one register over `{category, deleted}`. A delete is a
/// tombstone with a newer HLC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleRegister {
    pub category: Category,
    pub deleted: bool,
    pub hlc: Hlc,
}

/// Settings that are written into the document and, in v1, held: no Mac
/// applies them from here (§10.1, §11). Absent until a Mac first writes one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitations_enabled: Option<Register<bool>>,
    /// `null` once quiet hours are turned off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours: Option<Register<Option<QuietHours>>>,
}

/// `velvt_quiet_hours`, as migration 0021 bounds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietHours {
    pub start_local_minutes: u16,
    pub end_local_minutes: u16,
    pub rule_version: u32,
}

/// How a Mac got the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinedVia {
    Created,
    IcloudKeychain,
    Code,
}

/// The `mvp-2` categories: exactly the ones a local rule can hold
/// (`abstraction::override_label_for_category`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Category {
    FocusWork,
    PassiveConsumption,
    SocialFeed,
    Communication,
    TaskManagement,
    Reference,
    System,
    Unlogged,
}

impl Category {
    pub const ALL: [Self; 8] = [
        Self::FocusWork,
        Self::PassiveConsumption,
        Self::SocialFeed,
        Self::Communication,
        Self::TaskManagement,
        Self::Reference,
        Self::System,
        Self::Unlogged,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FocusWork => "FOCUS_WORK",
            Self::PassiveConsumption => "PASSIVE_CONSUMPTION",
            Self::SocialFeed => "SOCIAL_FEED",
            Self::Communication => "COMMUNICATION",
            Self::TaskManagement => "TASK_MANAGEMENT",
            Self::Reference => "REFERENCE",
            Self::System => "SYSTEM",
            Self::Unlogged => "UNLOGGED",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }
}

/// A Mac's id in the document: a version-4 UUID in its canonical lowercase,
/// hyphenated spelling. Kept across Log Out, so a Mac that signs back in keeps
/// its one entry (J7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncDeviceId(Uuid);

impl SyncDeviceId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn parse(value: &str) -> Option<Self> {
        canonical_uuid(value).and_then(Self::checked)
    }

    /// The 16 raw bytes, as the handoff payload carries them.
    pub fn from_bytes(bytes: [u8; 16]) -> Option<Self> {
        Self::checked(Uuid::from_bytes(bytes))
    }

    fn checked(uuid: Uuid) -> Option<Self> {
        (uuid.get_version_num() == 4 && uuid.get_variant() == uuid::Variant::RFC4122)
            .then_some(Self(uuid))
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for SyncDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), formatter)
    }
}

impl fmt::Debug for SyncDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SyncDeviceId({self})")
    }
}

impl Serialize for SyncDeviceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SyncDeviceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom("not a sync device id"))
    }
}

/// velvt-core's `devices.id`: a UUID string in canonical lowercase spelling.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerDeviceId(Uuid);

impl ServerDeviceId {
    pub fn parse(value: &str) -> Option<Self> {
        canonical_uuid(value).map(Self)
    }
}

impl fmt::Display for ServerDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), formatter)
    }
}

impl fmt::Debug for ServerDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ServerDeviceId({self})")
    }
}

impl Serialize for ServerDeviceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ServerDeviceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom("not a server device id"))
    }
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    let uuid = Uuid::try_parse(value).ok()?;
    (uuid.hyphenated().to_string() == value).then_some(uuid)
}

/// A Mac's name: the document's only free text. 1–64 characters, with no
/// control characters, line or paragraph separators, or bidirectional
/// formatting characters (a name is quoted in security notices, so it must
/// not be able to reorder the text around it).
///
/// `Debug` shows only the length: a name can identify a person and has no
/// place in a log.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceName(String);

impl DeviceName {
    pub fn new(value: &str) -> Option<Self> {
        let count = value.chars().count();
        let valid =
            (1..=MAX_NAME_CHARACTERS).contains(&count) && !value.chars().any(is_forbidden_in_name);
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_forbidden_in_name(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
}

impl fmt::Debug for DeviceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DeviceName({} chars)", self.0.chars().count())
    }
}

impl Serialize for DeviceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DeviceName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).ok_or_else(|| de::Error::custom("not a device name"))
    }
}

/// A rule's key: `site:`, `bundle:` or `app:` and a person key in 64
/// lowercase hex characters. Ordered as its string spelling, so the document's
/// JSON lists rules in sorted key order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RuleKey {
    pub kind: PersonKeyKind,
    pub key: PersonKey,
}

impl RuleKey {
    pub fn parse(value: &str) -> Option<Self> {
        let (prefix, hex) = value.split_once(':')?;
        Some(Self {
            kind: PersonKeyKind::from_prefix(prefix)?,
            key: PersonKey::from_hex(hex)?,
        })
    }
}

impl fmt::Display for RuleKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.kind.prefix(), self.key.to_hex())
    }
}

impl Ord for RuleKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.kind.prefix(), self.key).cmp(&(other.kind.prefix(), other.key))
    }
}

impl PartialOrd for RuleKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Serialize for RuleKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RuleKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom("not a rule key"))
    }
}

impl Serialize for PersonSalt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_base64())
    }
}

impl<'de> Deserialize<'de> for PersonSalt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Zeroizing::new(String::deserialize(deserializer)?);
        Self::from_base64(&value).map_err(|_| de::Error::custom("not a person salt"))
    }
}

/// An object whose keys are unique. serde's own map impls keep the last of
/// two equal keys, which would let two parsers read one document differently.
fn unique_map<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct UniqueMap<K, V>(PhantomData<(K, V)>);

    impl<'de, K, V> Visitor<'de> for UniqueMap<K, V>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        type Value = BTreeMap<K, V>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object with unique keys")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut map = BTreeMap::new();
            while let Some((key, value)) = access.next_entry()? {
                if map.insert(key, value).is_some() {
                    return Err(de::Error::custom("duplicate key"));
                }
            }
            Ok(map)
        }
    }

    deserializer.deserialize_map(UniqueMap(PhantomData))
}

impl PrefsDocument {
    /// The document a vault starts with (§10.4 create).
    pub fn new(person_salt: PersonSalt) -> Self {
        Self {
            schema: SCHEMA_V1,
            person_salt,
            devices: BTreeMap::new(),
            rules: BTreeMap::new(),
            settings: Settings::default(),
        }
    }

    pub fn live_device_count(&self) -> usize {
        self.devices
            .values()
            .filter(|entry| !entry.removed.value)
            .count()
    }

    pub fn live_rule_count(&self) -> usize {
        self.rules.values().filter(|rule| !rule.deleted).count()
    }

    /// Sets one rule's register, refusing a new live rule past the cap: past
    /// 2,000 the Mac stops mirroring rather than write a copy that no Mac can
    /// open (§4.1). Editing or deleting an existing rule is always allowed.
    pub fn set_rule(&mut self, key: RuleKey, rule: RuleRegister) -> Result<(), DocumentError> {
        let adds_live = !rule.deleted && self.rules.get(&key).is_none_or(|old| old.deleted);
        if adds_live && self.live_rule_count() >= MAX_LIVE_RULES {
            return Err(DocumentError::TooManyLiveRules);
        }
        self.rules.insert(key, rule);
        Ok(())
    }

    /// Sets one device entry, refusing a new live entry past either cap.
    pub fn set_device(
        &mut self,
        id: SyncDeviceId,
        entry: DeviceEntry,
    ) -> Result<(), DocumentError> {
        let existing = self.devices.get(&id);
        if existing.is_none() && self.devices.len() >= MAX_DEVICE_ENTRIES {
            return Err(DocumentError::TooManyDeviceEntries);
        }
        let adds_live = !entry.removed.value && existing.is_none_or(|old| old.removed.value);
        if adds_live && self.live_device_count() >= MAX_LIVE_DEVICES {
            return Err(DocumentError::TooManyLiveDevices);
        }
        self.devices.insert(id, entry);
        Ok(())
    }

    /// Every check a schema-1 document must pass, on open and before a write.
    pub fn validate(&self) -> Result<(), DocumentError> {
        if self.schema != SCHEMA_V1 {
            return Err(DocumentError::UnsupportedSchema(self.schema));
        }
        if self.devices.len() > MAX_DEVICE_ENTRIES {
            return Err(DocumentError::TooManyDeviceEntries);
        }
        if self.live_device_count() > MAX_LIVE_DEVICES {
            return Err(DocumentError::TooManyLiveDevices);
        }
        if self.live_rule_count() > MAX_LIVE_RULES {
            return Err(DocumentError::TooManyLiveRules);
        }
        if !self
            .devices
            .values()
            .all(|entry| Hlc::is_valid_wall_ms(entry.joined_at_ms))
        {
            return Err(DocumentError::OutOfRange("joined_at_ms"));
        }
        if let Some(Register {
            value: Some(quiet_hours),
            ..
        }) = &self.settings.quiet_hours
        {
            if quiet_hours.start_local_minutes > LAST_LOCAL_MINUTE
                || quiet_hours.end_local_minutes > LAST_LOCAL_MINUTE
            {
                return Err(DocumentError::OutOfRange("quiet_hours minutes"));
            }
            if quiet_hours.rule_version == 0 {
                return Err(DocumentError::OutOfRange("quiet_hours.rule_version"));
            }
        }
        Ok(())
    }

    /// The JSON a writer seals. Refuses a document that would fail
    /// [`Self::validate`] on any Mac, or that is too large to seal.
    ///
    /// Written into one buffer allocated at the largest size a document may
    /// have, which is never grown: a growing `Vec` would free unwiped partial
    /// copies of the plaintext, `person_salt` among them, on every reallocation.
    pub fn to_json(&self) -> Result<Zeroizing<Vec<u8>>, DocumentError> {
        self.validate()?;
        let mut writer =
            BoundedWriter(Zeroizing::new(Vec::with_capacity(MAX_DOCUMENT_JSON_LENGTH)));
        match serde_json::to_writer(&mut writer, self) {
            Ok(()) => Ok(writer.0),
            Err(error) if error.is_io() => Err(DocumentError::TooLarge),
            Err(error) => Err(error.into()),
        }
    }
}

/// A writer that refuses to hold more than [`MAX_DOCUMENT_JSON_LENGTH`]
/// bytes, so a buffer made with that capacity is never reallocated.
struct BoundedWriter(Zeroizing<Vec<u8>>);

impl std::io::Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_DOCUMENT_JSON_LENGTH - self.0.len() {
            return Err(std::io::Error::other("the document is too large to seal"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What opening a copy gives this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodedDocument {
    /// Schema 1: read, merged and written as usual.
    Current(PrefsDocument),
    /// A higher schema: this Mac is read-only until it updates
    /// (`read_only_newer_schema`). Deliberately a different type with no way
    /// to encode it, so it cannot be written back.
    NewerSchema(NewerSchemaView),
}

/// The two applied fields of a document from a newer Velvt. Nothing else in
/// it is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewerSchemaView {
    pub schema: u16,
    pub devices: BTreeMap<SyncDeviceId, NewerSchemaDevice>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewerSchemaDevice {
    pub name: Option<Register<DeviceName>>,
    pub notify_daily_insight: Option<Register<bool>>,
}

/// Decodes document JSON: strictly for schema 1, leniently and partially for a
/// higher one.
pub fn decode(json: &[u8]) -> Result<DecodedDocument, DocumentError> {
    #[derive(Deserialize)]
    struct SchemaOnly {
        schema: u16,
    }

    if json.len() > MAX_DOCUMENT_JSON_LENGTH {
        return Err(DocumentError::TooLarge);
    }
    let SchemaOnly { schema } = serde_json::from_slice(json)?;
    match schema {
        SCHEMA_V1 => {
            let document: PrefsDocument = serde_json::from_slice(json)?;
            document.validate()?;
            Ok(DecodedDocument::Current(document))
        }
        newer if newer > SCHEMA_V1 => Ok(DecodedDocument::NewerSchema(decode_newer(newer, json)?)),
        older => Err(DocumentError::UnsupportedSchema(older)),
    }
}

/// A register read without `deny_unknown_fields`, since a newer schema may add
/// fields to registers too.
#[derive(Deserialize)]
struct LenientRegister<T> {
    value: T,
    hlc: Hlc,
}

/// The one part of a newer-schema document this Mac reads. Every other
/// field, `person_salt` included, is skipped by the parser and never held in
/// memory, since nothing here could wipe it.
#[derive(Deserialize)]
struct NewerSchemaFields {
    #[serde(default)]
    devices: serde_json::Value,
}

fn decode_newer(schema: u16, json: &[u8]) -> Result<NewerSchemaView, DocumentError> {
    let NewerSchemaFields { devices: entries } = serde_json::from_slice(json)?;
    let mut devices = BTreeMap::new();
    for (id, entry) in entries.as_object().into_iter().flatten() {
        let Some(id) = SyncDeviceId::parse(id) else {
            continue;
        };
        let field = |name: &str| entry.get(name).cloned();
        let device = NewerSchemaDevice {
            name: field("name").and_then(lenient_register::<DeviceName>),
            notify_daily_insight: field("notify_daily_insight").and_then(lenient_register::<bool>),
        };
        devices.insert(id, device);
    }
    Ok(NewerSchemaView { schema, devices })
}

fn lenient_register<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Option<Register<T>> {
    serde_json::from_value::<LenientRegister<T>>(value)
        .ok()
        .map(|register| Register {
            value: register.value,
            hlc: register.hlc,
        })
}

/// Seals a document for `PUT` at the context's version. The context's schema
/// must be the document's.
pub fn seal(
    key: &VaultKey,
    context: &DocumentContext<'_>,
    document: &PrefsDocument,
) -> Result<Vec<u8>, VaultDocError> {
    if context.schema != document.schema {
        return Err(DocumentError::SchemaMismatch.into());
    }
    let json = document.to_json()?;
    Ok(key.seal_document(context, &json)?)
}

/// Opens and validates a served copy. The schema inside must equal the schema
/// the copy was sealed under, which the AAD binds.
pub fn open(
    key: &VaultKey,
    context: &DocumentContext<'_>,
    sealed: &[u8],
) -> Result<DecodedDocument, VaultDocError> {
    let json = key.open_document(context, sealed)?;
    let decoded = decode(&json)?;
    let schema = match &decoded {
        DecodedDocument::Current(document) => document.schema,
        DecodedDocument::NewerSchema(view) => view.schema,
    };
    if schema != context.schema {
        return Err(DocumentError::SchemaMismatch.into());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{
        decode, open, seal, Category, DecodedDocument, DeviceName, DocumentError, PrefsDocument,
        RuleKey, VaultDocError, MAX_DEVICE_ENTRIES, MAX_LIVE_DEVICES, MAX_LIVE_RULES,
    };
    use crate::abstraction::{override_label_for_category, Taxonomy};
    use crate::vault::crypto::{
        CryptoError, DocumentContext, PersonKeyKind, MAX_DOCUMENT_JSON_LENGTH,
    };
    use crate::vault::merge::Hlc;
    use crate::vault::test_support::{
        device, device_entry, hlc, name, numbered_rule, prefs, rule, rule_key, sample_document,
        test_key, user, uuid_device, NOW_MS,
    };

    fn as_value(document: &PrefsDocument) -> Value {
        serde_json::from_slice(&document.to_json().unwrap()).unwrap()
    }

    fn decode_value(value: &Value) -> Result<DecodedDocument, DocumentError> {
        decode(&serde_json::to_vec(value).unwrap())
    }

    fn malformed(result: Result<DecodedDocument, DocumentError>) -> bool {
        matches!(result, Err(DocumentError::Malformed { .. }))
    }

    #[test]
    fn a_document_reads_back_exactly() {
        let document = sample_document();
        let json = document.to_json().unwrap();
        assert_eq!(
            decode(&json).unwrap(),
            DecodedDocument::Current(document.clone())
        );
        assert_eq!(
            document.to_json().unwrap(),
            json,
            "encoding is deterministic"
        );
    }

    /// The bytes a second implementation must reproduce: struct fields in
    /// declaration order, the `devices` and `rules` maps in sorted key order,
    /// no white space. (`serde_json::Value` sorts every object, so the shape
    /// test below cannot see field order.)
    #[test]
    fn the_json_lists_fields_in_declaration_order() {
        let document = sample_document();
        let json = String::from_utf8(document.to_json().unwrap().to_vec()).unwrap();
        let salt = document.person_salt.to_base64();
        assert!(
            json.starts_with(&format!(
                r#"{{"schema":1,"person_salt":"{}","devices":{{"{}":{{"joined_via":"created","joined_at_ms":{NOW_MS},"server_device_id":{{"value":"#,
                salt.as_str(),
                device(1)
            )),
            "{json}"
        );
        assert!(json.contains(r#""rules":{"app:"#));
        assert!(json.contains(r#"{"category":"COMMUNICATION","deleted":true,"hlc":"#));
        assert!(json.contains(r#""settings":{"invitations_enabled":{"value":false,"hlc":"#));
        assert!(json.contains(
            r#""quiet_hours":{"value":{"start_local_minutes":1320,"end_local_minutes":420,"rule_version":1},"hlc":"#
        ));
        let without_names = json.replace("Studio Mac", "").replace("MacBook Pro", "");
        assert!(!without_names.contains(char::is_whitespace), "{json}");
    }

    /// The JSON is written into one buffer made at the largest document size,
    /// so no reallocation leaves an unwiped partial copy behind.
    #[test]
    fn the_json_is_written_into_one_buffer_that_never_grows() {
        let json = sample_document().to_json().unwrap();
        assert!(json.capacity() >= MAX_DOCUMENT_JSON_LENGTH);
        assert!(json.len() < json.capacity());
    }

    /// The shape §4.1 publishes.
    #[test]
    fn the_json_has_the_published_shape() {
        let value = as_value(&sample_document());
        let keys =
            |value: &Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
        assert_eq!(
            keys(&value),
            ["devices", "person_salt", "rules", "schema", "settings"]
        );
        let entry = &value["devices"][device(1).to_string()];
        assert_eq!(
            keys(entry),
            [
                "joined_at_ms",
                "joined_via",
                "name",
                "notify_daily_insight",
                "removed",
                "server_device_id"
            ]
        );
        assert_eq!(
            entry["name"],
            json!({"value": "Studio Mac", "hlc": entry["name"]["hlc"]})
        );
        let hlc = entry["name"]["hlc"].as_str().unwrap();
        assert_eq!(hlc, format!("{NOW_MS}.0000.{}", device(1)));
        let rule_keys = keys(&value["rules"]);
        assert!(rule_keys[0].starts_with("app:"));
        assert!(rule_keys[1].starts_with("bundle:"));
        assert!(rule_keys[2].starts_with("site:"));
        assert_eq!(
            value["rules"][&rule_keys[2]],
            json!({"category": "SOCIAL_FEED", "deleted": false, "hlc": value["rules"][&rule_keys[2]]["hlc"]})
        );
        assert_eq!(
            value["settings"]["quiet_hours"]["value"],
            json!({"start_local_minutes": 1320, "end_local_minutes": 420, "rule_version": 1})
        );
        assert_eq!(value["person_salt"].as_str().unwrap().len(), 44);
    }

    /// The document's only free text is a Mac's name (§4.1, T19). Every other
    /// string in it is an id, an HLC, a rule key, the salt, or an enum value.
    #[test]
    fn the_only_free_text_in_a_document_is_a_mac_name() {
        fn walk(value: &Value, path: &str, strings: &mut Vec<(String, String)>) {
            match value {
                Value::String(text) => strings.push((path.to_owned(), text.clone())),
                Value::Array(items) => {
                    for item in items {
                        walk(item, &format!("{path}[]"), strings);
                    }
                }
                Value::Object(map) => {
                    for (key, item) in map {
                        strings.push((format!("{path}.<key>"), key.clone()));
                        walk(item, &format!("{path}.{key}"), strings);
                    }
                }
                _ => {}
            }
        }
        const FIELD_NAMES: &[&str] = &[
            "schema",
            "person_salt",
            "devices",
            "rules",
            "settings",
            "joined_via",
            "joined_at_ms",
            "server_device_id",
            "name",
            "notify_daily_insight",
            "removed",
            "category",
            "deleted",
            "invitations_enabled",
            "quiet_hours",
            "start_local_minutes",
            "end_local_minutes",
            "rule_version",
            "value",
            "hlc",
        ];
        let document = sample_document();
        let names: Vec<String> = document
            .devices
            .values()
            .map(|entry| entry.name.value.as_str().to_owned())
            .collect();
        let mut strings = Vec::new();
        walk(&as_value(&document), "", &mut strings);

        for (path, text) in strings {
            let structured = Hlc::parse(&text).is_some()
                || super::SyncDeviceId::parse(&text).is_some()
                || super::ServerDeviceId::parse(&text).is_some()
                || RuleKey::parse(&text).is_some()
                || Category::parse(&text).is_some()
                || ["created", "icloud_keychain", "code"].contains(&text.as_str())
                || (path == ".person_salt" && text.len() == 44)
                || (path.ends_with(".<key>") && FIELD_NAMES.contains(&text.as_str()));
            let is_name = path.ends_with(".name.value") && names.contains(&text);
            assert!(structured || is_name, "free text at {path}");
        }
    }

    #[test]
    fn every_category_is_one_a_local_rule_can_hold_and_the_taxonomy_declares() {
        let taxonomy = Taxonomy::from_builtin().unwrap();
        for category in Category::ALL {
            assert!(override_label_for_category(category.as_str()).is_some());
            assert!(taxonomy.contains_category(category.as_str()));
            assert_eq!(Category::parse(category.as_str()), Some(category));
            assert_eq!(
                serde_json::to_value(category).unwrap(),
                Value::String(category.as_str().to_owned())
            );
        }
    }

    #[test]
    fn a_schema_one_document_rejects_anything_unknown_or_duplicated() {
        let base = as_value(&sample_document());
        let device_key = device(1).to_string();
        let rule_key = base["rules"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        type Edit = Box<dyn Fn(&mut Value)>;
        let edits: Vec<(&str, Edit)> = vec![
            ("top-level field", Box::new(|v| v["extra"] = json!(1))),
            (
                "entry field",
                Box::new(move |v| v["devices"][&device_key]["activity_name"] = json!("Xcode")),
            ),
            (
                "register field",
                Box::new(|v| {
                    let key = device(1).to_string();
                    v["devices"][&key]["name"]["writer"] = json!("x");
                }),
            ),
            (
                "rule field",
                Box::new(move |v| v["rules"][&rule_key]["host"] = json!("example.com")),
            ),
            (
                "settings field",
                Box::new(|v| v["settings"]["notify_category_reminder"] = json!(true)),
            ),
            (
                "quiet hours field",
                Box::new(|v| v["settings"]["quiet_hours"]["value"]["days"] = json!(1)),
            ),
        ];
        for (label, edit) in edits {
            let mut value = base.clone();
            edit(&mut value);
            assert!(malformed(decode_value(&value)), "{label}");
        }

        let json = String::from_utf8(sample_document().to_json().unwrap().to_vec()).unwrap();
        let first_device = format!("\"{}\":", device(1));
        let duplicated = json.replacen(
            &first_device,
            &format!("{first_device}{},{first_device}", device_json(&base)),
            1,
        );
        assert!(malformed(decode(duplicated.as_bytes())), "duplicate device");
        let first_rule = json.find("\"app:").unwrap();
        let end = json[first_rule..].find('}').unwrap() + first_rule + 1;
        let rule_text = &json[first_rule..end];
        let duplicated = json.replacen(rule_text, &format!("{rule_text},{rule_text}"), 1);
        assert!(malformed(decode(duplicated.as_bytes())), "duplicate rule");
        let duplicated = json.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1);
        assert!(malformed(decode(duplicated.as_bytes())), "duplicate schema");
    }

    fn device_json(base: &Value) -> String {
        serde_json::to_string(&base["devices"][device(1).to_string()]).unwrap()
    }

    #[test]
    fn a_schema_one_document_rejects_values_outside_their_ranges() {
        let base = as_value(&sample_document());
        let entry = device(1).to_string();
        let rule = base["rules"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let pointer_edits: Vec<(String, Value)> = vec![
            ("/rules/R/category".into(), json!("PRODUCTIVE")),
            ("/rules/R/category".into(), json!("focus_work")),
            ("/rules/R/deleted".into(), json!(0)),
            ("/rules/R/hlc".into(), json!("1760486400000.0000")),
            ("/devices/D/joined_via".into(), json!("magic")),
            ("/devices/D/joined_at_ms".into(), json!(-1)),
            ("/devices/D/joined_at_ms".into(), json!(1.5)),
            ("/devices/D/name/value".into(), json!("")),
            ("/devices/D/name/value".into(), json!("x".repeat(65))),
            ("/devices/D/name/value".into(), json!("Studio\nMac")),
            ("/devices/D/name/value".into(), json!("Studio\u{202E}caM")),
            ("/devices/D/name/value".into(), json!(42)),
            (
                "/devices/D/server_device_id/value".into(),
                json!("not-a-uuid"),
            ),
            ("/devices/D/notify_daily_insight/value".into(), json!("yes")),
            ("/devices/D/removed/hlc".into(), json!(null)),
            (
                "/settings/quiet_hours/value/start_local_minutes".into(),
                json!(1440),
            ),
            (
                "/settings/quiet_hours/value/end_local_minutes".into(),
                json!(-1),
            ),
            ("/settings/quiet_hours/value/rule_version".into(), json!(0)),
            ("/person_salt".into(), json!("c2hvcnQ=")),
            ("/schema".into(), json!(1.0)),
        ];
        for (pointer, replacement) in pointer_edits {
            let pointer = pointer
                .replace("/R/", &format!("/{rule}/"))
                .replace("/D/", &format!("/{entry}/"));
            let mut value = base.clone();
            *value.pointer_mut(&pointer).unwrap() = replacement.clone();
            assert!(
                decode_value(&value).is_err(),
                "{pointer} = {replacement} was accepted"
            );
        }

        let mut value = base.clone();
        let entry_value = value["devices"][&entry].clone();
        value["devices"]
            .as_object_mut()
            .unwrap()
            .insert(device(0xab).to_string().to_uppercase(), entry_value);
        assert!(malformed(decode_value(&value)), "uppercase sync device id");

        for bad_key in [
            format!("site:{}", "A".repeat(64)),
            format!("site:{}", "a".repeat(63)),
            format!("window:{}", "a".repeat(64)),
            "site:".to_owned(),
        ] {
            let mut value = base.clone();
            let rule_value = value["rules"][&rule].clone();
            value["rules"]
                .as_object_mut()
                .unwrap()
                .insert(bad_key.clone(), rule_value);
            assert!(malformed(decode_value(&value)), "{bad_key}");
        }
    }

    #[test]
    fn a_name_may_be_any_sixty_four_characters_without_control_or_direction_marks() {
        assert!(DeviceName::new(&"é".repeat(64)).is_some());
        assert!(DeviceName::new("Kevin’s MacBook Pro").is_some());
        assert!(DeviceName::new(&"é".repeat(65)).is_none());
        for bad in [
            "\u{0}",
            "a\tb",
            "a\u{7f}",
            "a\u{2028}",
            "\u{2066}x",
            "\u{200F}",
        ] {
            assert!(DeviceName::new(bad).is_none(), "{bad:?}");
        }
        let shown = format!("{:?}", name("Kevin’s MacBook Pro"));
        assert!(!shown.contains("Kevin"), "{shown}");
    }

    #[test]
    fn schema_zero_is_refused_and_a_missing_field_is_malformed() {
        assert_eq!(
            decode(br#"{"schema":0}"#),
            Err(DocumentError::UnsupportedSchema(0))
        );
        assert!(malformed(decode(br#"{"schema":1}"#)));
        assert!(malformed(decode(b"[]")));
        assert!(malformed(decode(b"")));
        assert!(malformed(decode(br#"{"schema":70000}"#)));
    }

    #[test]
    fn the_caps_hold_on_read_and_a_writer_cannot_exceed_them() {
        let at = hlc(NOW_MS, 0, 1);
        let mut document = sample_document();
        document.devices.clear();
        for seed in 0..MAX_LIVE_DEVICES as u8 {
            document
                .set_device(device(seed), device_entry(seed, "Mac", at))
                .unwrap();
        }
        assert_eq!(
            document.set_device(device(200), device_entry(200, "Mac", at)),
            Err(DocumentError::TooManyLiveDevices)
        );
        let mut removed = device_entry(200, "Mac", at);
        removed.removed.value = true;
        document.set_device(device(200), removed).unwrap();
        document
            .set_device(device(0), device_entry(0, "Renamed", at))
            .unwrap();
        assert!(document.to_json().is_ok());

        let mut over = document.clone();
        over.devices
            .insert(device(201), device_entry(201, "Mac", at));
        assert_eq!(
            over.to_json().err(),
            Some(DocumentError::TooManyLiveDevices)
        );
        let json = serde_json::to_vec(&over).unwrap();
        assert_eq!(decode(&json), Err(DocumentError::TooManyLiveDevices));

        let mut entries = sample_document();
        entries.devices.clear();
        for index in 0..MAX_DEVICE_ENTRIES {
            let mut entry = device_entry(1, "Mac", at);
            entry.removed.value = true;
            entries.devices.insert(uuid_device(index), entry);
        }
        assert!(entries.to_json().is_ok());
        let mut entry = device_entry(1, "Mac", at);
        entry.removed.value = true;
        assert_eq!(
            entries.set_device(uuid_device(MAX_DEVICE_ENTRIES), entry.clone()),
            Err(DocumentError::TooManyDeviceEntries)
        );
        entries
            .devices
            .insert(uuid_device(MAX_DEVICE_ENTRIES), entry);
        assert_eq!(
            entries.to_json().err(),
            Some(DocumentError::TooManyDeviceEntries)
        );

        let mut rules = sample_document();
        rules.rules.clear();
        for index in 0..MAX_LIVE_RULES {
            rules
                .set_rule(numbered_rule(index), rule(Category::FocusWork, false, at))
                .unwrap();
        }
        let extra = numbered_rule(MAX_LIVE_RULES);
        assert_eq!(
            rules.set_rule(extra, rule(Category::FocusWork, false, at)),
            Err(DocumentError::TooManyLiveRules)
        );
        rules
            .set_rule(extra, rule(Category::FocusWork, true, at))
            .unwrap();
        rules
            .set_rule(numbered_rule(0), rule(Category::SocialFeed, false, at))
            .unwrap();
        rules
            .set_rule(numbered_rule(1), rule(Category::SocialFeed, true, at))
            .unwrap();
        rules
            .set_rule(extra, rule(Category::FocusWork, false, at))
            .unwrap();
        assert_eq!(rules.live_rule_count(), MAX_LIVE_RULES);
        assert!(rules.to_json().is_ok());

        rules.rules.insert(
            numbered_rule(MAX_LIVE_RULES + 1),
            rule(Category::FocusWork, false, at),
        );
        assert_eq!(rules.to_json().err(), Some(DocumentError::TooManyLiveRules));
        assert_eq!(
            decode(&serde_json::to_vec(&rules).unwrap()),
            Err(DocumentError::TooManyLiveRules)
        );
    }

    /// The longest name in bytes: 64 characters of three bytes each.
    const WIDE_NAME: &str = "€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€";

    fn full_document(
        live_devices: usize,
        entries: usize,
        live_rules: usize,
        tombstones: usize,
    ) -> PrefsDocument {
        let at = hlc(NOW_MS, 9_999, 255);
        let mut document = sample_document();
        document.devices.clear();
        document.rules.clear();
        for index in 0..entries {
            let mut entry = device_entry(1, WIDE_NAME, at);
            entry.removed.value = index >= live_devices;
            document.devices.insert(uuid_device(index), entry);
        }
        for index in 0..live_rules + tombstones {
            let deleted = index >= live_rules;
            document.rules.insert(
                numbered_rule(index),
                rule(Category::PassiveConsumption, deleted, at),
            );
        }
        document
    }

    /// Every live rule and live Mac the caps allow still seals, inside 512 KiB.
    /// A rule costs about 187 bytes of JSON, not the 110 §4.1 estimates, so 2,000
    /// of them take about 366 KiB.
    #[test]
    fn the_live_caps_fit_the_largest_sealed_size() {
        let document = full_document(MAX_LIVE_DEVICES, MAX_LIVE_DEVICES, MAX_LIVE_RULES, 0);
        let json = document.to_json().unwrap();
        assert!(json.len() <= 400 * 1024, "{} bytes", json.len());

        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = DocumentContext {
            user_id: &user_id,
            name: &name,
            key_id: key.key_id(),
            schema: 1,
            version: 1,
        };
        let sealed = seal(&key, &context, &document).unwrap();
        assert_eq!(sealed.len(), 512 * 1024);
    }

    /// The caps alone do not bound the size: 2,000 live rules pass 512 KiB with
    /// 1,000 tombstones beside them, or with 256 device entries whose names use
    /// three-byte characters. Such a document is refused by the writer, and by
    /// every reader, rather than written.
    #[test]
    fn a_document_past_the_size_limit_is_refused_not_written() {
        for document in [
            full_document(MAX_LIVE_DEVICES, MAX_DEVICE_ENTRIES, MAX_LIVE_RULES, 0),
            full_document(MAX_LIVE_DEVICES, MAX_LIVE_DEVICES, MAX_LIVE_RULES, 1_000),
        ] {
            document.validate().unwrap();
            assert_eq!(document.to_json().err(), Some(DocumentError::TooLarge));
            let json = serde_json::to_vec(&document).unwrap();
            assert!(json.len() > MAX_DOCUMENT_JSON_LENGTH);
            assert_eq!(decode(&json), Err(DocumentError::TooLarge));
        }
    }

    #[test]
    fn a_newer_schema_is_read_only_for_names_and_the_daily_insight_switch() {
        let mut value = as_value(&sample_document());
        value["schema"] = json!(2);
        value["new_top_level"] = json!({"anything": [1, 2, 3]});
        let first = device(1).to_string();
        let second = device(2).to_string();
        value["devices"][&first]["notify_category_reminder"] = json!({"value": true, "hlc": "x"});
        value["devices"][&first]["name"]["writer"] = json!("member");
        value["devices"][&second]["name"] = json!({"value": "", "hlc": "bad"});
        value["devices"]["not-an-id"] = json!({"name": {"value": "Ghost"}});
        value["rules"] = json!("a newer layout");

        let DecodedDocument::NewerSchema(view) = decode_value(&value).unwrap() else {
            panic!("a newer schema decoded as current");
        };
        assert_eq!(view.schema, 2);
        assert_eq!(view.devices.len(), 2);
        let studio = &view.devices[&device(1)];
        assert_eq!(studio.name.as_ref().unwrap().value.as_str(), "Studio Mac");
        assert!(studio.notify_daily_insight.as_ref().unwrap().value);
        let laptop = &view.devices[&device(2)];
        assert!(laptop.name.is_none(), "an invalid name is not applied");
        assert!(laptop.notify_daily_insight.is_some());

        let bare = decode(br#"{"schema":9}"#).unwrap();
        assert!(matches!(bare, DecodedDocument::NewerSchema(view) if view.devices.is_empty()));
    }

    #[test]
    fn a_document_seals_and_opens_under_its_context() {
        let key = test_key();
        let (user_id, name) = (user(), prefs());
        let context = DocumentContext {
            user_id: &user_id,
            name: &name,
            key_id: key.key_id(),
            schema: 1,
            version: 3,
        };
        let document = sample_document();
        let sealed = seal(&key, &context, &document).unwrap();
        assert_eq!(
            open(&key, &context, &sealed).unwrap(),
            DecodedDocument::Current(document.clone())
        );

        let other_version = DocumentContext {
            version: 4,
            ..context
        };
        assert_eq!(
            open(&key, &other_version, &sealed),
            Err(VaultDocError::Crypto(CryptoError::Open))
        );
        let schema_two = DocumentContext {
            schema: 2,
            ..context
        };
        assert_eq!(
            seal(&key, &schema_two, &document),
            Err(VaultDocError::Document(DocumentError::SchemaMismatch))
        );

        let mut newer = as_value(&document);
        newer["schema"] = json!(2);
        let newer_json = serde_json::to_vec(&newer).unwrap();
        let sealed_newer = key.seal_document(&schema_two, &newer_json).unwrap();
        assert!(matches!(
            open(&key, &schema_two, &sealed_newer),
            Ok(DecodedDocument::NewerSchema(_))
        ));
        let sealed_mislabelled = key.seal_document(&context, &newer_json).unwrap();
        assert_eq!(
            open(&key, &context, &sealed_mislabelled),
            Err(VaultDocError::Document(DocumentError::SchemaMismatch))
        );

        let mut invalid = document.clone();
        invalid
            .settings
            .quiet_hours
            .as_mut()
            .unwrap()
            .value
            .as_mut()
            .unwrap()
            .rule_version = 0;
        assert_eq!(
            seal(&key, &context, &invalid),
            Err(VaultDocError::Document(DocumentError::OutOfRange(
                "quiet_hours.rule_version"
            )))
        );
    }

    #[test]
    fn a_new_document_is_empty_schema_one() {
        let document = PrefsDocument::new(crate::vault::test_support::salt(3));
        assert_eq!(document.schema, 1);
        assert!(document.devices.is_empty() && document.rules.is_empty());
        let json = document.to_json().unwrap();
        assert_eq!(decode(&json).unwrap(), DecodedDocument::Current(document));
        let key = rule_key(PersonKeyKind::Site, 1);
        assert_eq!(RuleKey::parse(&key.to_string()), Some(key));
    }
}
