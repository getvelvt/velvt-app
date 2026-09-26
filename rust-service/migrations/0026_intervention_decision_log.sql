-- Every moment the drift policy was evaluated, and what it decided --
-- including every abstention.
--
-- Deliberately NOT `work_block_intervention`: that table's PRIMARY KEY(block_id)
-- is the denominator of the pre-registered primary outcome, and writing silence
-- decisions into it would silently change a pre-registered metric. Nothing in
-- this migration writes to, reads from, or alters `work_block_intervention`.
--
-- Delivery behaviour changes by exactly zero. Every gate still suppresses
-- identically; this only records the decision the gate already made.
--
-- Privacy posture is identical to `work_block_intervention`: `anchor_category`
-- is a broad taxonomy category, never app identity, a window title, a URL, a
-- local display label, a stable mapping key, or intention text. Nothing in this
-- table is representable in the upload path. It is cleared by clear-all-data
-- through the same ON DELETE CASCADE the rest of the block evidence uses.

CREATE TABLE intervention_decision_log (
    decision_id       TEXT    PRIMARY KEY NOT NULL,
    occurred_at       INTEGER NOT NULL,
    block_id          TEXT,
    policy_version    INTEGER NOT NULL CHECK(policy_version >= 1),
    -- Gate evidence at the moment of decision, for later off-policy analysis.
    -- NULL `anchor_category` is meaningful, not missing data: it records that
    -- the gate abstained before it had computed an anchor. The log states what
    -- the gate knew, not what could be reconstructed afterwards.
    anchor_category   TEXT,
    switch_count      INTEGER NOT NULL CHECK(switch_count >= 0),
    elapsed_seconds   INTEGER NOT NULL CHECK(elapsed_seconds >= 0),
    remaining_seconds INTEGER NOT NULL CHECK(remaining_seconds >= 0),
    -- The closed verdict enum. Every value must be reachable -- there is a test.
    gate_verdict      TEXT    NOT NULL CHECK(gate_verdict IN (
                          'offered',
                          'abstained_warmup',
                          'abstained_remaining',
                          'abstained_block_cap',
                          'abstained_backoff',
                          'abstained_no_anchor',
                          'abstained_min_switches',
                          'abstained_at_anchor',
                          'withheld_demotion',
                          'suppressed_dnd')),
    -- 1.0 today: the policy is deterministic. The column exists so randomization
    -- can be enabled later without a migration or a protocol change.
    propensity        REAL    NOT NULL DEFAULT 1.0
                          CHECK(propensity > 0.0 AND propensity <= 1.0),
    -- Proximal outcome, backfilled on the same horizon regardless of verdict.
    -- This is what makes abstentions comparable to offers later. NULL means the
    -- horizon has not been resolved; a resolved-and-negative row is 0, never
    -- NULL, so "unresolved" and "did not return" can never be confused.
    anchor_seen_within_600s INTEGER CHECK(anchor_seen_within_600s IN (0, 1)),
    outcome_at        INTEGER,
    FOREIGN KEY(block_id) REFERENCES work_block(block_id) ON DELETE CASCADE
);

CREATE INDEX idx_decision_log_block_time
    ON intervention_decision_log(block_id, occurred_at);
CREATE INDEX idx_decision_log_verdict
    ON intervention_decision_log(gate_verdict, occurred_at DESC);
