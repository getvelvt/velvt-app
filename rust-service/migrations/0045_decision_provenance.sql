-- Which rung of the classification ladder produced each observation the drift
-- gate saw, and how many of a decision's observations a synced rule decided.
--
-- WHY. The personal-vault design (docs/architecture/personal-vault-and-devices.md,
-- section 11.1, lock 3) syncs category rules between a person's Macs and holds
-- them: no synced rule classifies anything until a pre-registered policy
-- version turns that on, between waves. These two columns are added while
-- that is still impossible, so the logged data itself shows that no decision
-- was made on a synced rule, and so the build that turns it on finds them
-- already in place rather than adding them mid-wave.
--
-- `work_block_observation.classification_source` is the
-- `ClassificationSource` of the classification that opened the row, as the
-- abstraction engine reported it: a seed entry, a heuristic, Tier 2, a rule
-- you taught, the application's declared document types or App Store
-- category, or the fallback. A later report with the same evidence opens no
-- row; the open row absorbs it, and takes its source when that outranks the
-- row's: `vault_rule`, then `user_rule`, then every other rung, which never
-- replaces the opener. So a row names a rule whenever a rule classified any
-- report the row holds, including a dwell the rule kept from being a switch
-- away. `vault_rule` is accepted here and produced by nothing: the rung that
-- would produce it is not compiled in. NULL means the row was opened before
-- this migration and has absorbed no rule's report since.
--
-- `intervention_decision_log.synced_rule_evidence` will count the confident
-- observations in a decision's window that a synced rule classified, read
-- from the rows above. It is 0 on every row, including every row written
-- before this migration.
--
-- A record, not an input: the gate reads the observation's category, status
-- and confidence exactly as it did before, and never this column, so no
-- decision changes because of either (tests/golden_replay.rs). Same privacy
-- posture as the rest of each row: one closed token and one count, no app,
-- site, rule or label. Local only; no upload path and no export reads them.

ALTER TABLE work_block_observation ADD COLUMN classification_source TEXT
    CHECK(classification_source IS NULL OR classification_source IN (
        'seed', 'heuristic', 'embedding', 'user_rule', 'declared_document_types',
        'declared_app_category', 'fallback', 'vault_rule'));

ALTER TABLE intervention_decision_log ADD COLUMN synced_rule_evidence INTEGER NOT NULL DEFAULT 0
    CHECK(synced_rule_evidence >= 0);
