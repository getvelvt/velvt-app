-- Discovered antecedent patterns. Nothing reaches a user without `confirmed_at`
-- set from a held-out window: discovery and confirmation never share data.
--
-- Per `04-DATA-ARCHITECTURE.md` § 5. Three properties are enforced here rather
-- than in application code, for the same reason the one-intervention cap is
-- enforced by an index: a rule enforced by the database cannot be forgotten
-- during a rushed change, and a rushed change is exactly what a pitch week
-- produces.
--
--   1. A finding may not be SURFACED without held-out confirmation.
--   2. A finding may not be REGISTERED for a candidate outside the closed,
--      versioned registry -- enforced in `behavior/candidates.rs`, which is the
--      only thing that can produce a `candidate_id`, and re-checked on read.
--   3. The same candidate may not be recorded twice for the same discovery
--      window under the same registry version. `03` § 3.4's sequential-looking
--      problem is not solved by an index, but recording the same look twice and
--      calling the second one new evidence is one concrete way to make it
--      worse, and that one IS closable.
--
-- Privacy posture: `candidate_id` is a key from a closed compile-time registry
-- -- a time bin, a day type, a broad taxonomy category, a coarse elapsed bucket.
-- It can never hold an application name, a label, a stable id, a window title,
-- a URL, or intention text, because the registry that mints it has no
-- constructor that could. Nothing in this table is representable in the upload
-- path.
--
-- NOTHING SURFACES TODAY. There is no IPC message that carries a finding, no
-- copy template that renders one, and no caller that writes one outside tests.
-- Surfacing is Week 8+ and gated on real data; the trigger below is what makes
-- that a property of the schema rather than an intention.

CREATE TABLE antecedent_finding (
    finding_id                 TEXT    PRIMARY KEY NOT NULL,
    -- From the closed, versioned candidate registry. A finding for a candidate
    -- that is not registered cannot exist -- the same closed-registry discipline
    -- already used for intervention actions and explanation claims.
    candidate_id               TEXT    NOT NULL,
    candidate_registry_version INTEGER NOT NULL CHECK(candidate_registry_version >= 1),
    discovered_at              INTEGER NOT NULL,
    discovery_window_start     TEXT    NOT NULL CHECK(discovery_window_start GLOB '????-??-??'),
    discovery_window_end       TEXT    NOT NULL CHECK(discovery_window_end GLOB '????-??-??'),
    support_episodes           INTEGER NOT NULL CHECK(support_episodes >= 0),
    -- The RISK DIFFERENCE, P(Y=1|A) - P(Y=1|not A), on [-1, 1]. Not an odds
    -- ratio: RD is the only association measure the analysis computes, because
    -- it is the only one a copy surface could state, and a surface that renders
    -- a quantity the analysis did not compute is how honest products drift.
    effect_size                REAL    NOT NULL CHECK(effect_size >= -1.0 AND effect_size <= 1.0),
    q_value                    REAL    NOT NULL CHECK(q_value >= 0.0 AND q_value <= 1.0),
    -- Held-out confirmation. NULL means never confirmed, which means never shown.
    confirmed_at               INTEGER,
    confirm_support_episodes   INTEGER CHECK(confirm_support_episodes IS NULL
                                             OR confirm_support_episodes >= 0),
    confirm_effect_size        REAL    CHECK(confirm_effect_size IS NULL
                                             OR (confirm_effect_size >= -1.0
                                                 AND confirm_effect_size <= 1.0)),
    state                      TEXT    NOT NULL CHECK(state IN (
                                   'candidate', 'confirmed', 'surfaced', 'retracted', 'disputed')),
    surfaced_at                INTEGER,
    retracted_at               INTEGER,
    retraction_reason          TEXT    CHECK(retraction_reason IS NULL OR retraction_reason IN (
                                   'effect_disappeared', 'support_lost', 'user_disputed', 'registry_version_change')),
    user_disputed_at           INTEGER,

    -- HARDENING BEYOND THE SPEC, and the reason is a hole in the spec's own
    -- trigger. `BEFORE UPDATE OF state` cannot fire on an INSERT, so a row
    -- INSERTed directly with state = 'surfaced' and confirmed_at NULL would
    -- walk straight past it. Likewise a row whose `surfaced_at` is stamped
    -- without its `state` being touched. This CHECK closes both, declaratively,
    -- on every write path there is.
    CHECK(surfaced_at IS NULL OR confirmed_at IS NOT NULL),
    CHECK(state <> 'surfaced' OR confirmed_at IS NOT NULL),
    -- A confirmation is a pair of numbers or it is nothing. A `confirmed_at`
    -- with no held-out support count is an assertion with no evidence behind
    -- it, which is precisely what this table exists to prevent.
    CHECK(confirmed_at IS NULL
          OR (confirm_support_episodes IS NOT NULL AND confirm_effect_size IS NOT NULL)),
    CHECK(retracted_at IS NULL OR retraction_reason IS NOT NULL)
);

-- A finding may only be surfaced if it was confirmed on held-out data.
-- Verbatim from `04-DATA-ARCHITECTURE.md` § 5. Redundant with the CHECK above
-- and kept anyway: the CHECK states the invariant, the trigger states the rule
-- in the words the design document uses, and the error message is what someone
-- debugging a failed UPDATE will read.
CREATE TRIGGER trg_antecedent_finding_requires_confirmation
BEFORE UPDATE OF state ON antecedent_finding
WHEN NEW.state = 'surfaced' AND NEW.confirmed_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'a finding cannot be surfaced without held-out confirmation');
END;

-- The same rule on the path the spec's trigger cannot see.
CREATE TRIGGER trg_antecedent_finding_insert_requires_confirmation
BEFORE INSERT ON antecedent_finding
WHEN NEW.state = 'surfaced' AND NEW.confirmed_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'a finding cannot be surfaced without held-out confirmation');
END;

-- Confirmation must come from a window that is not the discovery window.
-- "Discovery and confirmation never share data" is the whole claim, so a
-- confirmation stamped before the discovery window had even closed -- which is
-- what re-scoring the discovery window itself looks like from the outside --
-- must be unrepresentable rather than merely discouraged.
--
-- `strftime` rather than `unixepoch` because the latter needs SQLite 3.38 and
-- this file must apply on whatever the bundled amalgamation happens to be.
CREATE TRIGGER trg_antecedent_finding_confirmation_is_held_out
BEFORE UPDATE OF confirmed_at ON antecedent_finding
WHEN NEW.confirmed_at IS NOT NULL
     AND NEW.confirmed_at < CAST(strftime('%s', NEW.discovery_window_end) AS INTEGER)
BEGIN
    SELECT RAISE(ABORT, 'held-out confirmation cannot predate the discovery window it confirms');
END;

-- One recorded look per candidate per discovery window per registry version.
-- Re-running the same analysis and inserting the result again would double the
-- apparent evidence for a finding while adding none.
CREATE UNIQUE INDEX idx_antecedent_finding_one_look_per_window
    ON antecedent_finding(candidate_id, candidate_registry_version,
                          discovery_window_start, discovery_window_end);

CREATE INDEX idx_antecedent_finding_state
    ON antecedent_finding(state, discovered_at DESC);
