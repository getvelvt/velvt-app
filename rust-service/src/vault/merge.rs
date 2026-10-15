//! Hybrid logical clocks and the document merge (design §10.3).
//!
//! Every mutable value is a register `{value, hlc}`, and a merge takes the
//! register with the higher HLC. The merge is commutative, associative and
//! idempotent for documents that share a `person_salt` while the result stays
//! within the live caps, so any set of Macs converges whatever order they sync
//! in. Past a cap, the server's copy keeps its entries and this Mac's
//! additions wait ([`merge`]).
//!
//! A served copy older than the newest this Mac has seen goes through
//! [`merge_older_copy`] instead, which adds nothing: after tombstone
//! collection, an old copy is the only thing that could bring a removed Mac or
//! a deleted rule back.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::doc::{
    DeviceEntry, PrefsDocument, Register, RuleKey, RuleRegister, Settings, SyncDeviceId,
    MAX_DEVICE_ENTRIES, MAX_LIVE_DEVICES, MAX_LIVE_RULES,
};

/// Deleted rules and removed device entries are dropped on write once their
/// register is older than this.
pub const TOMBSTONE_RETENTION_MS: u64 = 180 * 24 * 60 * 60 * 1_000;

/// `<13-digit unix ms>.<4-digit counter>.<sync_device_id>`, compared as a
/// string. Every part has a fixed width, so the string order is the order of
/// `(wall_ms, counter, device)`, which is what [`Ord`] compares.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hlc {
    wall_ms: u64,
    counter: u16,
    device: SyncDeviceId,
}

impl Hlc {
    /// The first and last milliseconds with 13 digits and no leading zero:
    /// 2001-09-09 to 2286-11-20.
    pub const MIN_WALL_MS: u64 = 1_000_000_000_000;
    pub const MAX_WALL_MS: u64 = 9_999_999_999_999;
    pub const MAX_COUNTER: u16 = 9_999;
    const LENGTH: usize = 13 + 1 + 4 + 1 + 36;

    pub fn new(wall_ms: u64, counter: u16, device: SyncDeviceId) -> Option<Self> {
        (Self::is_valid_wall_ms(wall_ms) && counter <= Self::MAX_COUNTER).then_some(Self {
            wall_ms,
            counter,
            device,
        })
    }

    pub fn is_valid_wall_ms(wall_ms: u64) -> bool {
        (Self::MIN_WALL_MS..=Self::MAX_WALL_MS).contains(&wall_ms)
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value.len() != Self::LENGTH {
            return None;
        }
        let bytes = value.as_bytes();
        if bytes[13] != b'.' || bytes[18] != b'.' {
            return None;
        }
        let wall_ms = decimal(&value[..13])?;
        let counter = u16::try_from(decimal(&value[14..18])?).ok()?;
        let device = SyncDeviceId::parse(&value[19..])?;
        Self::new(wall_ms, counter, device)
    }

    pub fn wall_ms(&self) -> u64 {
        self.wall_ms
    }

    pub fn counter(&self) -> u16 {
        self.counter
    }

    pub fn device(&self) -> SyncDeviceId {
        self.device
    }
}

/// Only ASCII digits: `u64::from_str` would also take a leading `+`.
fn decimal(digits: &str) -> Option<u64> {
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.wall_ms, self.counter, self.device).cmp(&(other.wall_ms, other.counter, other.device))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:013}.{:04}.{}",
            self.wall_ms, self.counter, self.device
        )
    }
}

impl fmt::Debug for Hlc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Hlc({self})")
    }
}

impl Serialize for Hlc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom("not an HLC"))
    }
}

/// Why this Mac cannot issue an HLC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HlcError {
    #[error("the clock is outside the range an HLC can represent")]
    OutOfRange,
}

/// This Mac's HLC issuer. It issues `max(now_ms, highest seen)` plus a
/// counter, so each write it makes beats everything it has seen, its own
/// writes included, and it never goes backwards.
#[derive(Clone, Debug)]
pub struct HlcClock {
    device: SyncDeviceId,
    last: Option<(u64, u16)>,
}

impl HlcClock {
    pub fn new(device: SyncDeviceId) -> Self {
        Self { device, last: None }
    }

    /// Takes account of an HLC from anywhere: a document, or this Mac's own
    /// last write after a restart.
    pub fn observe(&mut self, hlc: &Hlc) {
        let seen = (hlc.wall_ms, hlc.counter);
        if self.last.is_none_or(|last| seen > last) {
            self.last = Some(seen);
        }
    }

    /// Observes every register in a document.
    pub fn observe_document(&mut self, document: &PrefsDocument) {
        for entry in document.devices.values() {
            self.observe(&entry.server_device_id.hlc);
            self.observe(&entry.name.hlc);
            self.observe(&entry.notify_daily_insight.hlc);
            self.observe(&entry.removed.hlc);
        }
        for rule in document.rules.values() {
            self.observe(&rule.hlc);
        }
        if let Some(register) = &document.settings.invitations_enabled {
            self.observe(&register.hlc);
        }
        if let Some(register) = &document.settings.quiet_hours {
            self.observe(&register.hlc);
        }
    }

    /// The next HLC for a write at `now_ms`.
    ///
    /// The counter is four digits, so string order holds only while it stays
    /// at four. When the next counter would be 10000 the millisecond field
    /// moves on by one and the counter starts again at 0 (§10.3). Without that,
    /// a Mac that had taken a far-future time from another Mac's bad clock
    /// would, after 10,000 writes, issue `…10000…`, which sorts below
    /// `…9999…`, and its own writes would start losing.
    pub fn tick(&mut self, now_ms: u64) -> Result<Hlc, HlcError> {
        let (wall_ms, counter) = match self.last {
            Some((last_wall, last_counter)) if now_ms <= last_wall => {
                if last_counter >= Hlc::MAX_COUNTER {
                    (last_wall.checked_add(1).ok_or(HlcError::OutOfRange)?, 0)
                } else {
                    (last_wall, last_counter + 1)
                }
            }
            _ => (now_ms, 0),
        };
        let hlc = Hlc::new(wall_ms, counter, self.device).ok_or(HlcError::OutOfRange)?;
        self.last = Some((wall_ms, counter));
        Ok(hlc)
    }
}

impl<T: Clone + Ord> Register<T> {
    /// The register with the higher HLC. Two registers with the same HLC came
    /// from the same write, so they agree; if a damaged document makes them
    /// differ, the larger value wins, which keeps the merge commutative.
    pub fn merge(&self, other: &Self) -> Self {
        match self.hlc.cmp(&other.hlc) {
            Ordering::Greater => self.clone(),
            Ordering::Less => other.clone(),
            Ordering::Equal if self.value >= other.value => self.clone(),
            Ordering::Equal => other.clone(),
        }
    }
}

impl RuleRegister {
    pub fn merge(&self, other: &Self) -> Self {
        let rank = |rule: &Self| (rule.hlc, rule.category, rule.deleted);
        if rank(self) >= rank(other) {
            self.clone()
        } else {
            other.clone()
        }
    }
}

impl DeviceEntry {
    /// Field by field, so Mac A renaming Mac B while B changes its own routing
    /// keeps both changes. `joined_via` and `joined_at_ms` are written once by
    /// the Mac itself; if two copies ever disagree, the earlier join is kept.
    pub fn merge(&self, other: &Self) -> Self {
        let (joined_at_ms, joined_via) = std::cmp::min(
            (self.joined_at_ms, self.joined_via),
            (other.joined_at_ms, other.joined_via),
        );
        Self {
            joined_via,
            joined_at_ms,
            server_device_id: self.server_device_id.merge(&other.server_device_id),
            name: self.name.merge(&other.name),
            notify_daily_insight: self.notify_daily_insight.merge(&other.notify_daily_insight),
            removed: self.removed.merge(&other.removed),
        }
    }
}

impl Settings {
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            invitations_enabled: merge_optional(
                &self.invitations_enabled,
                &other.invitations_enabled,
            ),
            quiet_hours: merge_optional(&self.quiet_hours, &other.quiet_hours),
        }
    }
}

fn merge_optional<T: Clone + Ord>(
    left: &Option<Register<T>>,
    right: &Option<Register<T>>,
) -> Option<Register<T>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.merge(right)),
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (None, None) => None,
    }
}

/// A merged document, and what of this Mac's copy it could not take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    pub document: PrefsDocument,
    /// The copy belongs to a vault made after a reset. Its salt is kept, this
    /// Mac's rules are dropped (their person keys were made under the old
    /// salt), and the caller clears its rule links so the rules re-mirror
    /// (§4.3).
    pub person_salt_replaced: bool,
    /// Rules this Mac made live that the merged copy has no room for: with
    /// them it would hold more than 2,000 live rules. The merged copy keeps
    /// the server's register for each (or no entry). They stay in this Mac's
    /// own tables and are not mirrored, and the status line says so (§4.1).
    pub rules_over_cap: Vec<RuleKey>,
    /// Device entries this Mac made live, or added, that the merged copy has
    /// no room for: more than 32 live Macs or 256 entries. The merged copy
    /// keeps the server's `removed` register for each (or no entry). This
    /// Mac's own id here means it cannot join until a Mac is removed.
    pub devices_over_cap: Vec<SyncDeviceId>,
}

/// Merges this Mac's document into the server's.
///
/// `person_salt` is written once at creation and the server copy's value wins
/// (§10.3). With equal salts the merge is symmetric in its arguments, as long
/// as the result is within the caps.
///
/// Past a cap the merge gives up this Mac's additions, never the server's
/// entries, so the result is always a document a writer may write (§4.1:
/// "a writer never produces a document that fails validation"). An addition
/// is an entry that is live only because of this Mac's copy. The server's copy
/// passed validation, so giving up every addition always fits; they are given
/// up from the highest key down until the result fits, which does not depend
/// on the order this Mac made them in. Two Macs that each add the 2,000th rule
/// then end with 2,000, not 2,001 that neither could write.
pub fn merge(server: &PrefsDocument, local: &PrefsDocument) -> Merged {
    let person_salt_replaced = server.person_salt != local.person_salt;
    let rules = if person_salt_replaced {
        server.rules.clone()
    } else {
        merge_maps(&server.rules, &local.rules, RuleRegister::merge)
    };
    let mut document = PrefsDocument {
        schema: server.schema,
        person_salt: server.person_salt.clone(),
        devices: merge_maps(&server.devices, &local.devices, DeviceEntry::merge),
        rules,
        settings: server.settings.merge(&local.settings),
    };
    let rules_over_cap = fit_rules(&mut document, server);
    let devices_over_cap = fit_devices(&mut document, server);
    Merged {
        document,
        person_salt_replaced,
        rules_over_cap,
        devices_over_cap,
    }
}

/// Merges a served copy whose version is below this Mac's
/// `highest_seen_version` into `current`, this Mac's document at that version
/// (its last opened copy with its own edits merged in). §6.1: "merge it into
/// ours (registers cannot go backwards); never cache it as current".
///
/// Unlike [`merge`], it never adds a device or a rule that `current` lacks.
/// Every key an older copy holds was carried into each later version, unless
/// a later write collected it as a tombstone more than 180 days old
/// ([`collect_garbage`]). The older copy has the entry as it was before it was
/// removed or deleted, so adding it back would revive a removed (perhaps
/// stolen) Mac, or a deleted rule, on every Mac. velvt-core can replay any old
/// copy signed under the current key, so it must not be able to do that (T4).
/// Registers of keys both copies hold merge as usual; a copy under another
/// `person_salt` contributes no rules; settings `current` has never had stay
/// unset. The result stays within the caps.
pub fn merge_older_copy(current: &PrefsDocument, older: &PrefsDocument) -> PrefsDocument {
    let mut document = current.clone();
    for (id, entry) in &mut document.devices {
        if let Some(old) = older.devices.get(id) {
            *entry = entry.merge(old);
        }
    }
    if current.person_salt == older.person_salt {
        for (key, rule) in &mut document.rules {
            if let Some(old) = older.rules.get(key) {
                *rule = rule.merge(old);
            }
        }
    }
    document.settings = Settings {
        invitations_enabled: merge_existing(
            &current.settings.invitations_enabled,
            &older.settings.invitations_enabled,
        ),
        quiet_hours: merge_existing(&current.settings.quiet_hours, &older.settings.quiet_hours),
    };
    fit_rules(&mut document, current);
    fit_devices(&mut document, current);
    document
}

fn merge_existing<T: Clone + Ord>(
    current: &Option<Register<T>>,
    older: &Option<Register<T>>,
) -> Option<Register<T>> {
    match (current, older) {
        (Some(current), Some(older)) => Some(current.merge(older)),
        (current, _) => current.clone(),
    }
}

/// Gives up live rules that only `merged` has live, highest key first, until
/// at most [`MAX_LIVE_RULES`] are live. Returns the keys given up.
fn fit_rules(merged: &mut PrefsDocument, baseline: &PrefsDocument) -> Vec<RuleKey> {
    let excess = merged.live_rule_count().saturating_sub(MAX_LIVE_RULES);
    if excess == 0 {
        return Vec::new();
    }
    let given_up: Vec<RuleKey> = merged
        .rules
        .iter()
        .rev()
        .filter(|(key, rule)| {
            !rule.deleted && baseline.rules.get(key).is_none_or(|kept| kept.deleted)
        })
        .map(|(key, _)| *key)
        .take(excess)
        .collect();
    for key in &given_up {
        match baseline.rules.get(key) {
            Some(kept) => merged.rules.insert(*key, kept.clone()),
            None => merged.rules.remove(key),
        };
    }
    given_up
}

/// The same for devices: first live entries past [`MAX_LIVE_DEVICES`] (the
/// baseline's `removed` register comes back, or the entry goes), then new
/// entries past [`MAX_DEVICE_ENTRIES`].
fn fit_devices(merged: &mut PrefsDocument, baseline: &PrefsDocument) -> Vec<SyncDeviceId> {
    let mut given_up = Vec::new();
    let excess_live = merged.live_device_count().saturating_sub(MAX_LIVE_DEVICES);
    if excess_live > 0 {
        let additions: Vec<SyncDeviceId> = merged
            .devices
            .iter()
            .rev()
            .filter(|(id, entry)| {
                !entry.removed.value
                    && baseline
                        .devices
                        .get(id)
                        .is_none_or(|kept| kept.removed.value)
            })
            .map(|(id, _)| *id)
            .take(excess_live)
            .collect();
        for id in additions {
            match baseline.devices.get(&id) {
                Some(kept) => {
                    if let Some(entry) = merged.devices.get_mut(&id) {
                        entry.removed = kept.removed.clone();
                    }
                }
                None => {
                    merged.devices.remove(&id);
                }
            }
            given_up.push(id);
        }
    }
    let excess_entries = merged.devices.len().saturating_sub(MAX_DEVICE_ENTRIES);
    if excess_entries > 0 {
        let additions: Vec<SyncDeviceId> = merged
            .devices
            .keys()
            .rev()
            .filter(|id| !baseline.devices.contains_key(id))
            .copied()
            .take(excess_entries)
            .collect();
        for id in additions {
            merged.devices.remove(&id);
            given_up.push(id);
        }
    }
    given_up
}

fn merge_maps<K: Ord + Clone, V: Clone>(
    left: &BTreeMap<K, V>,
    right: &BTreeMap<K, V>,
    merge_values: impl Fn(&V, &V) -> V,
) -> BTreeMap<K, V> {
    let mut merged = left.clone();
    for (key, value) in right {
        let value = match left.get(key) {
            Some(existing) => merge_values(existing, value),
            None => value.clone(),
        };
        merged.insert(key.clone(), value);
    }
    merged
}

/// Drops deleted rules and removed device entries whose register is older
/// than [`TOMBSTONE_RETENTION_MS`] at `now_ms`. Run before a write, never as
/// part of a merge.
///
/// A Mac offline for longer than that can bring a deleted rule back. That is
/// documented and harmless, since rules are held (§10.3). An old copy served
/// by velvt-core cannot, because it is merged with [`merge_older_copy`].
pub fn collect_garbage(document: &mut PrefsDocument, now_ms: u64) {
    let expired = |hlc: &Hlc| now_ms.saturating_sub(hlc.wall_ms()) > TOMBSTONE_RETENTION_MS;
    document
        .rules
        .retain(|_, rule| !(rule.deleted && expired(&rule.hlc)));
    document
        .devices
        .retain(|_, entry| !(entry.removed.value && expired(&entry.removed.hlc)));
}

#[cfg(test)]
mod tests {
    use super::{
        collect_garbage, merge, merge_older_copy, Hlc, HlcClock, HlcError, TOMBSTONE_RETENTION_MS,
    };
    use crate::vault::crypto::PersonKeyKind;
    use crate::vault::doc::{
        Category, JoinedVia, PrefsDocument, QuietHours, Register, RuleRegister, MAX_DEVICE_ENTRIES,
        MAX_LIVE_DEVICES, MAX_LIVE_RULES,
    };
    use crate::vault::test_support::{
        device, device_entry, hlc, name, numbered_rule, permutations, register, rule, rule_key,
        salt, sample_document, uuid_device, Rng, NOW_MS,
    };

    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

    // --- HLC ----------------------------------------------------------------------

    #[test]
    fn an_hlc_reads_back_from_its_string() {
        let value = hlc(NOW_MS, 42, 1);
        let text = value.to_string();
        assert_eq!(text, format!("{NOW_MS}.0042.{}", device(1)));
        assert_eq!(text.len(), 55);
        assert_eq!(Hlc::parse(&text), Some(value));
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::Value::String(text.clone())
        );
    }

    #[test]
    fn an_hlc_has_one_spelling() {
        let device = device(0xab).to_string();
        for bad in [
            format!("0{}.0000.{device}", &NOW_MS.to_string()[1..]),
            format!("{}.0000.{device}", &NOW_MS.to_string()[1..]),
            format!("{NOW_MS}0.0000.{device}"),
            format!("+{}.0000.{device}", &NOW_MS.to_string()[1..]),
            format!("{NOW_MS}.10000.{device}"),
            format!("{NOW_MS}.000.{device}"),
            format!("{NOW_MS}.+001.{device}"),
            format!("{NOW_MS}:0000:{device}"),
            format!("{NOW_MS}.0000.{}", device.to_uppercase()),
            format!("{NOW_MS}.0000.{}", "0".repeat(36)),
            format!("{NOW_MS}.0000"),
            String::new(),
        ] {
            assert_eq!(Hlc::parse(&bad), None, "{bad}");
        }
    }

    /// The design compares HLCs as strings; `Ord` must agree with that.
    #[test]
    fn hlc_order_is_string_order() {
        let mut rng = Rng::new(11);
        for _ in 0..5_000 {
            let pick = |rng: &mut Rng| {
                let offset = match rng.below(3) {
                    0 => rng.below(3),
                    _ => rng.below(Hlc::MAX_WALL_MS - Hlc::MIN_WALL_MS + 1),
                };
                hlc(
                    Hlc::MIN_WALL_MS + offset,
                    rng.below(10_000) as u16,
                    rng.below(4) as u8,
                )
            };
            let (left, right) = (pick(&mut rng), pick(&mut rng));
            assert_eq!(
                left.cmp(&right),
                left.to_string().cmp(&right.to_string()),
                "{left} vs {right}"
            );
        }
    }

    #[test]
    fn a_mac_s_own_write_beats_everything_it_has_seen() {
        let mut clock = HlcClock::new(device(9));
        let seen = hlc(NOW_MS + 60_000, 17, 0xff);
        clock.observe(&seen);

        let mine = clock.tick(NOW_MS).unwrap();

        assert!(mine > seen);
        assert!(mine.to_string() > seen.to_string());
        assert_eq!((mine.wall_ms(), mine.counter()), (NOW_MS + 60_000, 18));
        assert_eq!(mine.device(), device(9));
    }

    #[test]
    fn the_clock_never_goes_backwards() {
        let mut clock = HlcClock::new(device(9));
        let mut rng = Rng::new(12);
        let mut last = clock.tick(NOW_MS).unwrap();
        for _ in 0..20_000 {
            let now = NOW_MS - 5_000 + rng.below(10_000);
            let next = clock.tick(now).unwrap();
            assert!(
                next > last && next.to_string() > last.to_string(),
                "{last} then {next}"
            );
            last = next;
        }
        let restarted = {
            let mut clock = HlcClock::new(device(9));
            clock.observe(&last);
            clock.tick(NOW_MS - 1).unwrap()
        };
        assert!(restarted > last);
    }

    /// §10.3: when the next counter would be 10000, the millisecond moves on and
    /// the counter starts again, so the string order still holds.
    #[test]
    fn counter_overflow_keeps_string_order() {
        let mut clock = HlcClock::new(device(9));
        let far_future = hlc(NOW_MS + 365 * DAY_MS, Hlc::MAX_COUNTER, 0xff);
        clock.observe(&far_future);

        let next = clock.tick(NOW_MS).unwrap();

        assert_eq!(
            (next.wall_ms(), next.counter()),
            (far_future.wall_ms() + 1, 0)
        );
        assert!(next.to_string() > far_future.to_string());
        assert!(next > far_future);

        let mut previous = next;
        for _ in 0..25_000 {
            let next = clock.tick(NOW_MS).unwrap();
            assert!(
                next.to_string() > previous.to_string(),
                "{previous} then {next}"
            );
            assert!(next.counter() <= Hlc::MAX_COUNTER);
            previous = next;
        }
    }

    #[test]
    fn a_clock_outside_thirteen_digits_issues_nothing() {
        assert_eq!(
            HlcClock::new(device(9)).tick(Hlc::MAX_WALL_MS + 1),
            Err(HlcError::OutOfRange)
        );
        assert_eq!(
            HlcClock::new(device(9)).tick(Hlc::MIN_WALL_MS - 1),
            Err(HlcError::OutOfRange)
        );
        let mut clock = HlcClock::new(device(9));
        clock.observe(&hlc(Hlc::MAX_WALL_MS, Hlc::MAX_COUNTER, 1));
        assert_eq!(clock.tick(NOW_MS), Err(HlcError::OutOfRange));
    }

    // --- Registers and documents ---------------------------------------------------

    #[test]
    fn a_register_merge_is_commutative_associative_and_idempotent() {
        let mut rng = Rng::new(13);
        let pick = |rng: &mut Rng| {
            register(
                rng.below(3) as u8,
                hlc(
                    NOW_MS + rng.below(3),
                    rng.below(2) as u16,
                    rng.below(2) as u8,
                ),
            )
        };
        for _ in 0..2_000 {
            let (a, b, c) = (pick(&mut rng), pick(&mut rng), pick(&mut rng));
            assert_eq!(a.merge(&b), b.merge(&a));
            assert_eq!(a.merge(&b).merge(&c), a.merge(&b.merge(&c)));
            assert_eq!(a.merge(&a), a);
        }
    }

    /// One Mac's concurrent edits to a shared starting document, made with its
    /// own clock, which may run behind or ahead of the others.
    fn edit(base: &PrefsDocument, mac: u8, rng: &mut Rng) -> PrefsDocument {
        let mut document = base.clone();
        let mut clock = HlcClock::new(device(mac));
        clock.observe_document(base);
        let skew = rng.below(4_000);
        for _ in 0..1 + rng.below(4) {
            let now = NOW_MS - 2_000 + skew + rng.below(100);
            let at = clock.tick(now).unwrap();
            let target = device(1 + rng.below(2) as u8);
            match rng.below(8) {
                0 => {
                    let names = ["Studio Mac", "Desk", "Laptop", "Mac mini"];
                    let entry = document.devices.get_mut(&target).unwrap();
                    entry.name = register(name(names[rng.below(4) as usize]), at);
                }
                1 => {
                    let entry = document.devices.get_mut(&target).unwrap();
                    entry.notify_daily_insight = register(rng.below(2) == 0, at);
                }
                2 | 3 => {
                    let kind = PersonKeyKind::ALL[rng.below(3) as usize];
                    let key = rule_key(kind, 0xa0 + rng.below(3) as u8);
                    let category = Category::ALL[rng.below(8) as usize];
                    document
                        .set_rule(key, rule(category, rng.below(3) == 0, at))
                        .unwrap();
                }
                4 => {
                    document.settings.invitations_enabled = Some(register(rng.below(2) == 0, at));
                }
                5 => {
                    let value = (rng.below(3) > 0).then(|| QuietHours {
                        start_local_minutes: rng.below(1440) as u16,
                        end_local_minutes: rng.below(1440) as u16,
                        rule_version: 1,
                    });
                    document.settings.quiet_hours = Some(register(value, at));
                }
                6 => {
                    let mut entry = device_entry(mac, "New Mac", at);
                    entry.joined_via = JoinedVia::IcloudKeychain;
                    document.set_device(device(mac), entry).unwrap();
                }
                _ => {
                    let entry = document.devices.get_mut(&target).unwrap();
                    entry.removed = register(rng.below(2) == 0, at);
                }
            }
        }
        document
    }

    fn fold(base: &PrefsDocument, edits: &[PrefsDocument]) -> PrefsDocument {
        edits
            .iter()
            .fold(base.clone(), |server, local| merge(&server, local).document)
    }

    /// §10.3 and §16.1: every order in which three or four Macs' concurrent edits
    /// reach the server converges on one document.
    #[test]
    fn every_order_of_three_or_four_concurrent_edits_converges() {
        for seed in 0..150 {
            let mut rng = Rng::new(seed);
            let base = sample_document();
            for macs in [3_u8, 4] {
                let edits: Vec<PrefsDocument> = (0..macs)
                    .map(|mac| edit(&base, 20 + mac, &mut rng))
                    .collect();
                let expected = fold(&base, &edits);
                expected.validate().unwrap();
                for order in permutations(&edits) {
                    assert_eq!(fold(&base, &order), expected, "seed {seed}, {macs} Macs");
                }
                let (a, b, c) = (&edits[0], &edits[1], &edits[2]);
                let pairwise = merge(&merge(a, b).document, c).document;
                let grouped = merge(a, &merge(b, c).document).document;
                assert_eq!(pairwise, grouped, "seed {seed}: associativity");
                assert_eq!(merge(a, b).document, merge(b, a).document, "seed {seed}");
                assert_eq!(merge(a, a).document, *a, "seed {seed}: idempotence");
                assert_eq!(merge(&expected, a).document, expected, "seed {seed}");
                // Every copy that went into `expected` is older than it, and
                // nothing in one is missing from it or newer than it.
                for older in edits.iter().chain([&base]) {
                    assert_eq!(merge_older_copy(&expected, older), expected, "seed {seed}");
                }
            }
        }
    }

    #[test]
    fn a_later_delete_beats_an_earlier_edit_and_a_later_edit_beats_an_earlier_delete() {
        let base = sample_document();
        let key = rule_key(PersonKeyKind::Site, 0xa1);
        let mut deleted = base.clone();
        deleted
            .set_rule(
                key,
                rule(Category::SocialFeed, true, hlc(NOW_MS + 20, 0, 3)),
            )
            .unwrap();
        let mut edited = base.clone();
        edited
            .set_rule(
                key,
                rule(Category::Reference, false, hlc(NOW_MS + 10, 0, 4)),
            )
            .unwrap();

        for merged in [merge(&deleted, &edited), merge(&edited, &deleted)] {
            assert!(merged.document.rules[&key].deleted);
        }

        edited
            .set_rule(
                key,
                rule(Category::Reference, false, hlc(NOW_MS + 30, 0, 4)),
            )
            .unwrap();
        for merged in [merge(&deleted, &edited), merge(&edited, &deleted)] {
            let rule = &merged.document.rules[&key];
            assert!(!rule.deleted);
            assert_eq!(rule.category, Category::Reference);
        }
    }

    #[test]
    fn equal_hlcs_with_different_values_still_merge_the_same_either_way() {
        let at = hlc(NOW_MS, 0, 3);
        let left = RuleRegister {
            category: Category::FocusWork,
            deleted: false,
            hlc: at,
        };
        let right = RuleRegister {
            category: Category::SocialFeed,
            deleted: true,
            hlc: at,
        };
        assert_eq!(left.merge(&right), right.merge(&left));
    }

    #[test]
    fn concurrent_edits_to_different_fields_of_one_mac_both_survive() {
        let base = sample_document();
        let laptop = device(2);
        let mut renamed_by_studio = base.clone();
        renamed_by_studio.devices.get_mut(&laptop).unwrap().name =
            register(name("Work Laptop"), hlc(NOW_MS + 100, 0, 1));
        let mut routed_by_laptop = base.clone();
        routed_by_laptop
            .devices
            .get_mut(&laptop)
            .unwrap()
            .notify_daily_insight = register(false, hlc(NOW_MS + 90, 0, 2));

        for merged in [
            merge(&renamed_by_studio, &routed_by_laptop),
            merge(&routed_by_laptop, &renamed_by_studio),
        ] {
            let entry = &merged.document.devices[&laptop];
            assert_eq!(entry.name.value.as_str(), "Work Laptop");
            assert!(!entry.notify_daily_insight.value);
        }
    }

    #[test]
    fn signing_back_in_reuses_the_one_entry() {
        let base = sample_document();
        let laptop = device(2);
        let mut turned_off = base.clone();
        turned_off.devices.get_mut(&laptop).unwrap().removed =
            register(true, hlc(NOW_MS + 10, 0, 2));
        let mut back = turned_off.clone();
        back.devices.get_mut(&laptop).unwrap().removed = register(false, hlc(NOW_MS + 20, 0, 2));

        let merged = merge(&turned_off, &back).document;

        assert_eq!(merged.devices.len(), 2);
        assert!(!merged.devices[&laptop].removed.value);
        assert_eq!(merged.live_device_count(), 2);
    }

    #[test]
    fn the_earlier_join_is_kept_if_two_copies_disagree() {
        let base = sample_document();
        let mut later = base.clone();
        let entry = later.devices.get_mut(&device(2)).unwrap();
        entry.joined_at_ms += 1;
        entry.joined_via = JoinedVia::Created;
        let earlier = &base.devices[&device(2)];
        for merged in [merge(&base, &later), merge(&later, &base)] {
            let entry = &merged.document.devices[&device(2)];
            assert_eq!(
                (entry.joined_at_ms, entry.joined_via),
                (earlier.joined_at_ms, earlier.joined_via)
            );
        }
    }

    /// The salt is written once; the server copy's wins, and rules made under
    /// another salt are not carried into it.
    #[test]
    fn the_server_copy_s_person_salt_wins_and_this_mac_s_rules_under_another_salt_are_dropped() {
        let server = sample_document();
        let same_salt = merge(&server, &server);
        assert!(!same_salt.person_salt_replaced);
        assert_eq!(same_salt.document.person_salt, server.person_salt);

        let mut local = sample_document();
        local.person_salt = salt(8);
        local
            .set_rule(
                rule_key(PersonKeyKind::Site, 0x55),
                rule(Category::FocusWork, false, hlc(NOW_MS + 50, 0, 3)),
            )
            .unwrap();
        local.devices.insert(
            device(3),
            device_entry(3, "Mac mini", hlc(NOW_MS + 50, 0, 3)),
        );
        local.settings.invitations_enabled = Some(register(true, hlc(NOW_MS + 50, 0, 3)));

        let merged = merge(&server, &local);

        assert!(merged.person_salt_replaced);
        assert_eq!(merged.document.person_salt, server.person_salt);
        assert_eq!(merged.document.rules, server.rules);
        assert!(merged.document.devices.contains_key(&device(3)));
        assert_eq!(
            merged.document.settings.invitations_enabled,
            Some(Register {
                value: true,
                hlc: hlc(NOW_MS + 50, 0, 3)
            })
        );
    }

    #[test]
    fn tombstones_and_removed_entries_are_dropped_after_one_hundred_and_eighty_days() {
        assert_eq!(TOMBSTONE_RETENTION_MS, 180 * DAY_MS);
        let old = hlc(NOW_MS, 0, 1);
        let mut document = sample_document();
        let deleted_old = rule_key(PersonKeyKind::Site, 0x01);
        let live_old = rule_key(PersonKeyKind::Site, 0x02);
        let deleted_recent = rule_key(PersonKeyKind::Site, 0x03);
        document.rules.clear();
        document
            .rules
            .insert(deleted_old, rule(Category::System, true, old));
        document
            .rules
            .insert(live_old, rule(Category::System, false, old));
        document.rules.insert(
            deleted_recent,
            rule(Category::System, true, hlc(NOW_MS + 10 * DAY_MS, 0, 1)),
        );
        document.devices.get_mut(&device(2)).unwrap().removed = register(true, old);
        let removed_recent = device(4);
        let mut entry = device_entry(4, "Old Mac", old);
        entry.removed = register(true, hlc(NOW_MS + 10 * DAY_MS, 0, 4));
        document.devices.insert(removed_recent, entry);

        let mut at_limit = document.clone();
        collect_garbage(&mut at_limit, NOW_MS + 180 * DAY_MS);
        assert_eq!(at_limit, document, "exactly 180 days is kept");

        collect_garbage(&mut document, NOW_MS + 180 * DAY_MS + 1);

        assert!(!document.rules.contains_key(&deleted_old));
        assert!(document.rules.contains_key(&live_old));
        assert!(document.rules.contains_key(&deleted_recent));
        assert!(!document.devices.contains_key(&device(2)));
        assert!(document.devices.contains_key(&device(1)));
        assert!(document.devices.contains_key(&removed_recent));
    }

    /// T4 after tombstone collection. A copy from more than 180 days ago, still
    /// validly signed under the current key, holds a Mac and a rule as they
    /// were before they were removed and deleted; the tombstones that beat
    /// them have since been collected. Merged as a peer it would revive both
    /// on every Mac. Merged as the older copy it is, it adds nothing.
    #[test]
    fn an_older_copy_never_brings_back_what_tombstone_collection_dropped() {
        let stolen = device(2);
        let deleted = rule_key(PersonKeyKind::Site, 0xa1);
        let replayed = sample_document();
        assert!(!replayed.devices[&stolen].removed.value);
        assert!(!replayed.rules[&deleted].deleted);

        let mut current = replayed.clone();
        let removed_at = hlc(NOW_MS + DAY_MS, 0, 1);
        current.devices.get_mut(&stolen).unwrap().removed = register(true, removed_at);
        current
            .set_rule(deleted, rule(Category::SocialFeed, true, removed_at))
            .unwrap();
        collect_garbage(&mut current, NOW_MS + 182 * DAY_MS);
        assert!(!current.devices.contains_key(&stolen));
        assert!(!current.rules.contains_key(&deleted));

        let as_peer = merge(&replayed, &current).document;
        assert!(
            !as_peer.devices[&stolen].removed.value && !as_peer.rules[&deleted].deleted,
            "the symmetric merge revives both, which is why it is not used here"
        );

        let merged = merge_older_copy(&current, &replayed);
        assert_eq!(merged, current);
        assert_eq!(merged.live_device_count(), 1);
    }

    #[test]
    fn an_older_copy_still_merges_the_registers_both_copies_hold() {
        let current = sample_document();
        let mut older = sample_document();
        // A damaged older copy whose register is newer: it wins, as in any
        // merge, but brings no entry with it.
        older.devices.get_mut(&device(1)).unwrap().name =
            register(name("Desk"), hlc(NOW_MS + 50, 0, 1));
        older
            .devices
            .insert(device(3), device_entry(3, "Ghost", hlc(NOW_MS, 0, 3)));
        let mut no_settings = current.clone();
        no_settings.settings.invitations_enabled = None;
        older.settings.invitations_enabled = Some(register(true, hlc(NOW_MS + 50, 0, 1)));

        let merged = merge_older_copy(&current, &older);
        assert_eq!(merged.devices[&device(1)].name.value.as_str(), "Desk");
        assert!(!merged.devices.contains_key(&device(3)));
        assert_eq!(
            merged.settings.invitations_enabled,
            Some(register(true, hlc(NOW_MS + 50, 0, 1)))
        );
        assert_eq!(
            merge_older_copy(&no_settings, &older)
                .settings
                .invitations_enabled,
            None,
            "a setting this Mac has never had is not added"
        );

        let mut other_salt = older.clone();
        other_salt.person_salt = salt(8);
        other_salt
            .rules
            .values_mut()
            .for_each(|rule| rule.hlc = hlc(NOW_MS + 99, 0, 9));
        assert_eq!(
            merge_older_copy(&current, &other_salt).rules,
            current.rules,
            "rules under another salt are not merged"
        );
        assert_eq!(
            merge_older_copy(&current, &other_salt).person_salt,
            current.person_salt
        );
    }

    fn with_live_rules(range: std::ops::Range<usize>, at: Hlc) -> PrefsDocument {
        let mut document = sample_document();
        document.rules.clear();
        for index in range {
            document
                .rules
                .insert(numbered_rule(index), rule(Category::FocusWork, false, at));
        }
        document
    }

    /// §4.1: a writer never produces a document that fails validation. Two
    /// Macs that each add the 2,000th rule must not end with 2,001, which no
    /// Mac could write; the server's copy keeps its rule and this Mac's waits.
    #[test]
    fn two_macs_adding_the_two_thousandth_rule_end_with_two_thousand() {
        let at = hlc(NOW_MS, 0, 1);
        let full = with_live_rules(0..MAX_LIVE_RULES - 1, at);
        let mut server = full.clone();
        server
            .set_rule(
                numbered_rule(5_000),
                rule(Category::Reference, false, hlc(NOW_MS + 1, 0, 1)),
            )
            .unwrap();
        let mut local = full.clone();
        let mine = numbered_rule(6_000);
        local
            .set_rule(
                mine,
                rule(Category::Reference, false, hlc(NOW_MS + 2, 0, 2)),
            )
            .unwrap();

        let merged = merge(&server, &local);

        assert_eq!(merged.rules_over_cap, [mine]);
        assert_eq!(merged.document.live_rule_count(), MAX_LIVE_RULES);
        assert!(!merged.document.rules.contains_key(&mine));
        assert!(merged.document.to_json().is_ok());
        assert!(merged.devices_over_cap.is_empty());
    }

    /// The probe from the review: 1,500 live rules on each of two Macs, on
    /// different keys. The server's 1,500 stay; 500 of this Mac's fit, and the
    /// 1,000 with the highest keys wait.
    #[test]
    fn merging_past_the_rule_cap_gives_up_this_mac_s_additions_highest_key_first() {
        let at = hlc(NOW_MS, 0, 1);
        let server = with_live_rules(0..1_500, at);
        let local = with_live_rules(1_500..3_000, hlc(NOW_MS, 0, 2));

        let merged = merge(&server, &local);

        assert_eq!(merged.document.live_rule_count(), MAX_LIVE_RULES);
        assert_eq!(merged.rules_over_cap.len(), 1_000);
        let mut local_keys: Vec<_> = local.rules.keys().copied().collect();
        local_keys.sort();
        let (kept, given_up) = local_keys.split_at(500);
        assert!(kept
            .iter()
            .all(|key| merged.document.rules.contains_key(key)));
        assert!(given_up
            .iter()
            .all(|key| !merged.document.rules.contains_key(key)));
        assert!(server
            .rules
            .keys()
            .all(|key| merged.document.rules.contains_key(key)));
        assert!(merged.document.to_json().is_ok());
        // Within the caps nothing is given up, and the merge stays symmetric.
        let small = merge(&with_live_rules(0..10, at), &with_live_rules(5..15, at));
        assert!(small.rules_over_cap.is_empty());
    }

    /// A rule the server has deleted and this Mac revived, while another Mac
    /// filled the last place, keeps the server's tombstone.
    #[test]
    fn a_rule_revived_past_the_cap_keeps_the_server_s_tombstone() {
        let at = hlc(NOW_MS, 0, 1);
        let mut before = with_live_rules(0..MAX_LIVE_RULES - 1, at);
        let tombstone = rule(Category::System, true, hlc(NOW_MS + 1, 0, 1));
        let revived = numbered_rule(9_000);
        before.rules.insert(revived, tombstone.clone());
        let mut local = before.clone();
        local
            .set_rule(
                revived,
                rule(Category::System, false, hlc(NOW_MS + 2, 0, 2)),
            )
            .unwrap();
        let mut server = before;
        server
            .set_rule(
                numbered_rule(9_001),
                rule(Category::System, false, hlc(NOW_MS + 3, 0, 3)),
            )
            .unwrap();

        let merged = merge(&server, &local);

        assert_eq!(merged.rules_over_cap, [revived]);
        assert_eq!(merged.document.rules[&revived], tombstone);
        assert!(merged.document.to_json().is_ok());
    }

    /// Two Macs joining at 31 live Macs do not make 33: the one whose copy is
    /// merged second waits until a Mac is removed.
    #[test]
    fn merging_past_the_live_device_cap_gives_up_this_mac_s_join() {
        let at = hlc(NOW_MS, 0, 1);
        let mut full = sample_document();
        full.devices.clear();
        for index in 0..MAX_LIVE_DEVICES - 1 {
            full.devices
                .insert(uuid_device(index), device_entry(1, "Mac", at));
        }
        let mut server = full.clone();
        server
            .set_device(device(0xa0), device_entry(0xa0, "First", at))
            .unwrap();
        let mut local = full.clone();
        local
            .set_device(device(0xb0), device_entry(0xb0, "Second", at))
            .unwrap();

        let merged = merge(&server, &local);

        assert_eq!(merged.devices_over_cap, [device(0xb0)]);
        assert_eq!(merged.document.live_device_count(), MAX_LIVE_DEVICES);
        assert_eq!(merged.document.devices, server.devices);
        assert!(merged.document.to_json().is_ok());

        // A removed Mac that signs back in while another Mac takes the last
        // place keeps its entry, still removed.
        let returning = uuid_device(0);
        let mut removed = server.clone();
        removed.devices.get_mut(&returning).unwrap().removed =
            register(true, hlc(NOW_MS + 1, 0, 1));
        let mut back = removed.clone();
        back.devices.get_mut(&returning).unwrap().removed = register(false, hlc(NOW_MS + 2, 0, 9));
        let mut later_server = removed;
        later_server
            .set_device(device(0xc0), device_entry(0xc0, "Third", at))
            .unwrap();
        let merged = merge(&later_server, &back);
        assert_eq!(merged.devices_over_cap, [returning]);
        assert!(merged.document.devices[&returning].removed.value);
        assert!(merged.document.to_json().is_ok());
    }

    #[test]
    fn merging_past_the_entry_cap_gives_up_this_mac_s_new_entries() {
        let at = hlc(NOW_MS, 0, 1);
        let mut server = sample_document();
        server.devices.clear();
        for index in 0..MAX_DEVICE_ENTRIES {
            let mut entry = device_entry(1, "Old Mac", at);
            entry.removed.value = index >= 4;
            server.devices.insert(uuid_device(index), entry);
        }
        // This Mac collected one old entry, so its own copy has room.
        let mut local = server.clone();
        local.devices.remove(&uuid_device(MAX_DEVICE_ENTRIES - 1));
        local
            .set_device(device(0xd0), device_entry(0xd0, "New", at))
            .unwrap();

        let merged = merge(&server, &local);

        assert_eq!(merged.devices_over_cap, [device(0xd0)]);
        assert_eq!(merged.document.devices.len(), MAX_DEVICE_ENTRIES);
        assert!(merged.document.to_json().is_ok());
    }

    #[test]
    fn observing_a_document_sees_every_register() {
        let document = sample_document();
        let latest = document
            .devices
            .values()
            .map(|entry| entry.removed.hlc)
            .max()
            .unwrap();
        let mut clock = HlcClock::new(device(9));
        clock.observe_document(&document);
        assert!(clock.tick(NOW_MS - 1).unwrap() > latest);
    }
}
