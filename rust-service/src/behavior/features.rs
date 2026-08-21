//! The frozen feature contract: the observable `x_t`, and the list of things
//! this data structurally cannot identify.
//!
//! Per `03-BEHAVIORAL-ENGINE-SPEC.md` § 1. One row per closed run — a closed
//! `work_block_observation`, or an `out_of_block_run` now that the durable
//! out-of-block store exists.
//!
//! **This module deliberately computes nothing.** It is the contract that the
//! online change-point detector and the nightly segmenter will both read, and it
//! is published first precisely so the two cannot diverge. `q_t` and `s_t` in
//! particular are defined here as *the shipped gate's own functions*, not as
//! re-implementations: if the model and the gate disagree about what counts as
//! evidence, every comparison between them is meaningless.
//!
//! Nothing here is representable in the upload path, and nothing here holds a
//! label, a stable id, an application name, a window title, a URL, or intention
//! text.

// Nothing consumes these symbols yet, by design — the contract ships before
// either model does (§ 1 above). A module-level allow keeps adding a symbol to
// the contract a one-line change rather than a two-line one; the alternative is
// a per-item attribute on a file that is entirely contract.
#![allow(dead_code)]

/// The closed category vocabulary, `|C| = 8`.
///
/// Mirrors `resources/abstraction-taxonomy-mvp-1.json`, which is the authority.
/// Listed here so the feature layer's cardinality is a compile-time fact: a
/// Dirichlet prior over 8 categories is wrong the moment the taxonomy grows, and
/// it should fail loudly rather than quietly renormalise.
pub const CATEGORIES: [&str; 8] = [
    "FOCUS_WORK",
    "PASSIVE_CONSUMPTION",
    "SOCIAL_FEED",
    "COMMUNICATION",
    "TASK_MANAGEMENT",
    "REFERENCE",
    "SYSTEM",
    "UNLOGGED",
];

/// The taxonomy this contract was frozen against. A mismatch against the loaded
/// taxonomy means the contract, not the taxonomy, is out of date.
pub const CONTRACT_TAXONOMY_VERSION: &str = "mvp-1";

/// Version of the feature contract itself. Every model result must be logged
/// with it: a comparison across contract versions is a comparison of two
/// different feature spaces.
pub const FEATURE_CONTRACT_VERSION: u32 = 1;

/// The dwell clip, in seconds, applied at collection time.
///
/// This is not a choice made here — it is Swift's, at
/// `swift-client/Sources/VelvtMac/Collection/CollectionModule.swift:155`
/// (`maximumDwellDuration: TimeInterval = 30 * 60`), applied in
/// `dwellSeconds(from:through:)`. Restated as a constant so that
/// `d_t = ln(1 + min(dwell_seconds, DWELL_CLIP_SECONDS))` is saturating at the
/// same point the data already saturates, rather than at a second, different
/// point invented in Rust.
pub const DWELL_CLIP_SECONDS: u32 = 1800;

/// The trailing window, in seconds, over which `n_t` counts distinct confident
/// categories. Matches the shipped drift window so the feature and the gate see
/// the same recent past.
pub const TRAILING_WINDOW_SECONDS: i64 = 600;

/// The proximal-outcome horizon, in seconds: "a confident anchor observation was
/// seen within this many seconds of the decision."
///
/// The same horizon is applied to offers and to abstentions, which is the only
/// reason the two are comparable at all. Stored per decision in
/// `intervention_decision_log.anchor_seen_within_600s`.
pub const PROXIMAL_OUTCOME_HORIZON_SECONDS: i64 = 600;

/// One observable in the frozen `x_t` vector.
///
/// The `source` field is load-bearing documentation: it names the shipped code
/// or column the value must come from. A feature whose source is a
/// re-implementation of shipped logic is a bug in this table, not a detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observable {
    /// The symbol used in `03-BEHAVIORAL-ENGINE-SPEC.md`.
    pub symbol: &'static str,
    /// What the value is.
    pub definition: &'static str,
    /// Where it comes from. Never "derived in the model".
    pub source: &'static str,
}

/// The complete observable feature vector. Adding a row here is a contract
/// change and requires a `FEATURE_CONTRACT_VERSION` bump.
pub const OBSERVABLES: [Observable; 12] = [
    Observable {
        symbol: "c_t",
        definition: "Category, |C| = 8, from the closed shipped taxonomy",
        source: "resources/abstraction-taxonomy-mvp-1.json",
    },
    Observable {
        symbol: "d_t",
        definition: "ln(1 + min(dwell_seconds, 1800))",
        source: "clip is Swift's, CollectionModule.swift:155",
    },
    Observable {
        symbol: "q_t",
        definition: "Confident evidence, or not",
        source: "exactly `is_confident_evidence` in work_block/mod.rs — reused, \
                  never re-implemented",
    },
    Observable {
        symbol: "a_t",
        definition: "1[c_t == anchor]",
        source: "anchor from `dominant_category` in work_block/mod.rs",
    },
    Observable {
        symbol: "s_t",
        definition: "Departure indicator: a confident non-anchor observation \
                     whose previous confident observation was the anchor",
        source: "the same anchor->non-anchor rule the shipped gate uses in \
                 `evaluate_drift`",
    },
    Observable {
        symbol: "e_t",
        definition: "Elapsed fraction of the declared block",
        source: "`elapsed_seconds` / `planned_duration_seconds`",
    },
    Observable {
        symbol: "r_t",
        definition: "Remaining seconds in the declared block",
        source: "`planned_duration_seconds - elapsed_seconds`",
    },
    Observable {
        symbol: "k_t",
        definition: "Run index within the block",
        source: "derived from the ordered observation rows",
    },
    Observable {
        symbol: "n_t",
        definition: "Distinct confident categories in the trailing 600s",
        source: "derived, over `is_confident_evidence` rows only",
    },
    Observable {
        symbol: "h_t, w_t",
        definition: "Local hour 0-23; weekday or weekend",
        source: "focus_observer_state.utc_offset_seconds (migration 0019)",
    },
    Observable {
        symbol: "f_t",
        definition: "System Focus/DND active",
        source: "`FocusStateSource::is_focus_active`",
    },
    Observable {
        symbol: "g_t",
        definition: "Gap seconds since the previous run ended",
        source: "derived from adjacent closed observation spans",
    },
];

/// One thing Velvt cannot observe, with the reason it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotIdentifiable {
    pub construct: &'static str,
    pub reason: &'static str,
}

/// Why the input-activity half of this list is structural rather than a policy
/// choice, stated once so the individual entries can stay short.
///
/// **Verified by grep over the shipped sources on 2026-08-21**, not asserted:
/// `CGEventSource`, `CGEventSourceSecondsSinceLastEventType`, `CGEventTap`,
/// `IOHIDSystem`, `IOHIDManager`, `NSEvent.addGlobal`, `ScreenCaptureKit`,
/// `EventKit`, `EKEventStore`, and `CGWindowList` return **zero** matches in
/// `swift-client/Sources/` and **zero** in `rust-service/src/`.
///
/// The single match anywhere in the tree is
/// `CGEventSourceSecondsSinceLastEventType` inside the vendored Sparkle
/// updater's own `SPUStandardUserDriver.m` (two copies of the same checkout,
/// under `.build/checkouts/` and `DerivedData/SourcePackages/checkouts/`), where
/// Sparkle uses it to avoid interrupting an idle user with an update prompt.
/// It is third-party updater code, it is not on the collection path, and its
/// result never enters an event, an observation, or this feature vector.
pub const NO_INPUT_SIGNAL_RATIONALE: &str = "There is no input-activity signal \
    anywhere in the pipeline: no keystroke cadence, no mouse dynamics, no idle \
    timer, no screen contents, no window titles, no calendar. Every engagement \
    construct that would normally lean on those is unavailable by construction, \
    not by policy.";

/// The published not-identifiable list.
///
/// This is the mechanism that stops a latent variable being quietly promoted
/// into a product claim. Velvt **cannot** observe any of these and must never
/// claim to. Copy that asserts one of them is a bug, and the closed-template
/// copy mechanism is what enforces it.
pub const NOT_IDENTIFIABLE: [NotIdentifiable; 11] = [
    NotIdentifiable {
        construct: "cognitive load",
        reason: "no input-activity signal exists to estimate it from",
    },
    NotIdentifiable {
        construct: "effort",
        reason: "dwell on a category is time, not exertion",
    },
    NotIdentifiable {
        construct: "motivation",
        reason: "unobserved; nothing in x_t covaries with it identifiably",
    },
    NotIdentifiable {
        construct: "energy",
        reason: "would require an input or physiological signal; neither exists",
    },
    NotIdentifiable {
        construct: "mood",
        reason: "no affective signal is collected anywhere in the pipeline",
    },
    NotIdentifiable {
        construct: "stress",
        reason: "same: no input dynamics, no physiological input",
    },
    NotIdentifiable {
        construct: "whether the work was good",
        reason: "no content is observed — only broad category and duration",
    },
    NotIdentifiable {
        construct: "whether a switch was necessary",
        reason: "a necessary switch and an idle one are the same two rows",
    },
    NotIdentifiable {
        construct: "task difficulty",
        reason: "no task is observed, only an application category",
    },
    NotIdentifiable {
        construct: "whether the user was blocked",
        reason: "indistinguishable from any other departure from the anchor",
    },
    NotIdentifiable {
        construct: "intent of any kind",
        reason: "the one intention Velvt holds is text the user typed, is \
                 device-local, and expires in 24 hours; it is not a feature",
    },
];

/// What *is* weakly inferable, with stated uncertainty. This list is exhaustive:
/// anything not on it is not inferable, and this is the whole of it.
pub const WEAKLY_INFERABLE: [&str; 3] = ["fragmentation", "anchor adherence", "episode structure"];

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract's cardinality must match the shipped taxonomy. A Dirichlet
    /// prior over the wrong number of categories is wrong silently, which is the
    /// worst way for it to be wrong.
    #[test]
    fn contract_categories_match_the_shipped_taxonomy() {
        let raw = include_str!("../../resources/abstraction-taxonomy-mvp-1.json");
        let parsed: serde_json::Value = serde_json::from_str(raw).unwrap();
        let shipped: Vec<String> = parsed["categories"]
            .as_array()
            .expect("the taxonomy lists its categories")
            .iter()
            .map(|value| value.as_str().expect("a category is a string").to_owned())
            .collect();
        assert_eq!(
            shipped, CATEGORIES,
            "the frozen feature contract and the shipped taxonomy disagree about \
             the category vocabulary"
        );
        assert_eq!(
            parsed["category_taxonomy_version"].as_str(),
            Some(CONTRACT_TAXONOMY_VERSION)
        );
    }

    /// Symbols must be unique, or two features silently share a name in every
    /// logged result.
    #[test]
    fn observable_symbols_are_unique() {
        let mut symbols: Vec<&str> = OBSERVABLES.iter().map(|o| o.symbol).collect();
        symbols.sort_unstable();
        let count = symbols.len();
        symbols.dedup();
        assert_eq!(
            count,
            symbols.len(),
            "duplicate symbol in the feature contract"
        );
    }

    /// The dwell clip is Swift's number, not this module's. If the collection
    /// agent's clip moves and this constant does not, `d_t` saturates at a point
    /// the data does not — and the whole feature is quietly wrong.
    ///
    /// Deliberately tolerant of formatting (`30 * 60` or `1800`) and of the
    /// parameter moving, so ordinary Swift-side churn does not fail this; only
    /// an actual change to the number does.
    #[test]
    fn dwell_clip_matches_the_collection_agent() {
        let swift = include_str!(
            "../../../swift-client/Sources/VelvtMac/Collection/CollectionModule.swift"
        );
        let declaration = swift
            .lines()
            .find(|line| line.contains("maximumDwellDuration") && line.contains('='))
            .expect("the collection agent still declares a maximum dwell duration");
        assert!(
            declaration.contains("30 * 60") || declaration.contains("1800"),
            "the collection agent's dwell clip moved to `{}`; DWELL_CLIP_SECONDS \
             is now a different number from the one the data is clipped at",
            declaration.trim()
        );
        assert_eq!(DWELL_CLIP_SECONDS, 30 * 60);
    }

    /// The not-identifiable list is the credibility of the whole layer. An empty
    /// or truncated list would pass every other test in this file.
    #[test]
    fn the_not_identifiable_list_is_populated_and_reasoned() {
        assert_eq!(NOT_IDENTIFIABLE.len(), 11);
        for entry in NOT_IDENTIFIABLE {
            assert!(!entry.construct.is_empty());
            assert!(
                entry.reason.len() > 20,
                "'{}' is asserted without a reason",
                entry.construct
            );
        }
        assert_eq!(WEAKLY_INFERABLE.len(), 3);
    }
}
