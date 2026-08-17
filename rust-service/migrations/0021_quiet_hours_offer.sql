-- Quiet-hours offer memory and Velvt's own quiet-hours setting
-- (roadmap invariant 5; D2). Both are singleton, device-local rows.
--
-- The offer is produced by a deterministic, versioned pattern rule. A
-- decline is remembered here so the offer is not re-asked for the rule's
-- versioned interval; accepting configures `velvt_quiet_hours`, which only
-- ever reduces delivery. Velvt never reads or writes the macOS Focus
-- configuration itself. No Focus mode name, schedule, or configuration is
-- representable in either table.

CREATE TABLE quiet_hours_offer_state (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    rule_version INTEGER NOT NULL CHECK(rule_version >= 1),
    triggered_at INTEGER,
    offered_at INTEGER,
    response TEXT CHECK(response IN ('accepted', 'declined')),
    responded_at INTEGER
);

CREATE TABLE velvt_quiet_hours (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    start_local_minutes INTEGER NOT NULL CHECK(start_local_minutes BETWEEN 0 AND 1439),
    end_local_minutes INTEGER NOT NULL CHECK(end_local_minutes BETWEEN 0 AND 1439),
    rule_version INTEGER NOT NULL CHECK(rule_version >= 1),
    configured_at INTEGER NOT NULL
);
