-- The RAW APPLICATION NAME, stored verbatim, device-local, for at most 7 days.
--
-- `responsible_local_name_suggestion` (abstraction/engine.rs) returns
-- `Some(app_name.trim().to_owned())` -- the name itself, not a derivation -- and
-- only when the classifier matched neither a seed rule nor a user correction.
-- Seed-matched apps, user-corrected apps, and generic names ("app", "browser",
-- "unknown") all yield NULL. It exists so the local UI can offer a one-tap
-- rename instead of showing "Unclassified".
--
-- It is never copied to upload tables, is redacted in `Debug`, and expires with
-- the rest of `raw_event_buffer`. Disclosed by name in PRIVACY.md and in the
-- 0001 header. The original wording here ("a naming hint derived from the raw
-- application metadata") was accurate but obscuring; corrected 2026-08-21.
ALTER TABLE raw_event_buffer ADD COLUMN local_name_suggestion TEXT
    CHECK(local_name_suggestion IS NULL OR (
        length(trim(local_name_suggestion)) BETWEEN 1 AND 48
        AND instr(local_name_suggestion, char(10)) = 0
        AND instr(local_name_suggestion, char(13)) = 0
    ));
