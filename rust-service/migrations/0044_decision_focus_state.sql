-- Whether the service knew if a Focus was on when the drift gate decided
-- (protocol 36).
--
-- WHY. The gate holds an offer while a Focus is on (`suppressed_dnd`), but it
-- learns about Focus only from the app, and Apple gives Focus status only to
-- an app signed with the Communication Notifications entitlement. A build
-- without it still reports Focus as off, so a decision made during a Focus
-- the service could not see was stored exactly like one made with no Focus:
-- the offer was posted, macOS silenced it, and nothing recorded that the
-- service never knew.
--
-- `focus_state` is what the service knew at the instant of the decision:
--   on       the app can read Focus status, and the evidence record says a
--            Focus was on
--   off      the app can read Focus status, and the record says none was on
--   unknown  the app cannot read Focus status (no entitlement, a
--            restriction, denied or never answered), has not said whether it
--            can since the service started, or nothing has been recorded yet
-- NULL means the row was written before this migration, never "off".
--
-- A record, not an input: the gate reads `focus_state_evidence` exactly as it
-- did before, so no decision changes because of this column. Same privacy
-- posture as the rest of the row: one closed value, no Focus mode name,
-- configuration or schedule. Local only; no upload path reads it.

ALTER TABLE intervention_decision_log ADD COLUMN focus_state TEXT
    CHECK(focus_state IS NULL OR focus_state IN ('on', 'off', 'unknown'));
