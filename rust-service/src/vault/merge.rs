//! Hybrid logical clocks and the document merge (design §10.3).
//!
//! Every mutable value is a register `{value, hlc}`, and a merge takes the
//! register with the higher HLC. The merge is commutative, associative and
//! idempotent for documents that share a `person_salt`, so any set of Macs
//! converges whatever order they sync in.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::doc::{DeviceEntry, PrefsDocument, Register, RuleRegister, Settings, SyncDeviceId};

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

/// A merged document, and whether the server's copy carried a different
/// `person_salt` from this Mac's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    pub document: PrefsDocument,
    /// The copy belongs to a vault made after a reset. Its salt is kept, this
    /// Mac's rules are dropped (their person keys were made under the old
    /// salt), and the caller clears its rule links so the rules re-mirror
    /// (§4.3).
    pub person_salt_replaced: bool,
}

/// Merges this Mac's document into the server's.
///
/// `person_salt` is written once at creation and the server copy's value wins
/// (§10.3). With equal salts the merge is symmetric in its arguments.
pub fn merge(server: &PrefsDocument, local: &PrefsDocument) -> Merged {
    let person_salt_replaced = server.person_salt != local.person_salt;
    let rules = if person_salt_replaced {
        server.rules.clone()
    } else {
        merge_maps(&server.rules, &local.rules, RuleRegister::merge)
    };
    Merged {
        document: PrefsDocument {
            schema: server.schema,
            person_salt: server.person_salt.clone(),
            devices: merge_maps(&server.devices, &local.devices, DeviceEntry::merge),
            rules,
            settings: server.settings.merge(&local.settings),
        },
        person_salt_replaced,
    }
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
/// documented and harmless, since rules are held (§10.3).
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
    use super::{collect_garbage, merge, Hlc, HlcClock, HlcError, TOMBSTONE_RETENTION_MS};
    use crate::vault::crypto::PersonKeyKind;
    use crate::vault::doc::{
        Category, JoinedVia, PrefsDocument, QuietHours, Register, RuleRegister,
    };
    use crate::vault::test_support::{
        device, device_entry, hlc, name, permutations, register, rule, rule_key, salt,
        sample_document, Rng, NOW_MS,
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
