-- The application rung a window correction wrote, recorded on the window rule
-- itself.
--
-- WHY. Every correction has written two rungs since 0017: `personal_override`
-- for the window the user was looking at, and `personal_app_override` for the
-- application it belonged to. Removing the window rule has to take that second
-- rung with it or the undo changes nothing the user can see -- the engine falls
-- straight through the emptied window rung into the surviving app rung and
-- answers with the category the user just deleted.
--
-- Until this migration the removal resolved which app rung to delete by
-- subquerying `raw_event_buffer`, and that table is a 14-day TTL cache
-- (`VELVT_RAW_EVENT_TTL_HOURS`). Removing a correction older than the TTL found
-- no event, deleted nothing at app scope, and reported success: the window rule
-- went, the app rung stayed, and the surviving row was `app_only = 0`, which
-- `RULE_SOURCE` filters out of the correction history and
-- `unclassified_triage`'s NOT EXISTS matches anyway -- so the application could
-- be neither listed, nor removed, nor re-taught, short of a full Reset. That is
-- not the "one unreachable row in a rare case" 0035's header calls it: it is
-- every removal of a correction older than fourteen days.
--
-- So the pairing is recorded at write time, where the event still exists, and
-- removal becomes a key lookup that does not read `raw_event_buffer` at all.
-- `save_personal_override` (`persistence/sqlite.rs`) writes this column from the
-- most recent event of the mapping that is `app_scope_eligible = 1` -- the same
-- read `save_personal_app_override_by_stable_id` uses to choose which app rung
-- to write, so the column names the rung that correction actually wrote. It
-- coalesces on conflict: an edit made after the source events have aged out
-- resolves NULL and must not erase a pairing that was recorded when they were
-- still there.
--
-- NULL therefore means exactly one of two things, and both are correct without
-- an app rung to delete: the correction was made on a window that is not
-- generalizable to its application at all (`app_scope_eligible = 0` -- a browser
-- tab, where one site says nothing about the next, and where no app rung was
-- ever written), or the rule predates this column and no event of it survived to
-- be backfilled below.
--
-- The delete this column feeds still carries `app_only = 0` separately. A rule
-- taught about an application through triage is a rule in its own right (0035,
-- "sticky, never cleared") and must survive the removal of an unrelated window
-- rule for the same application, even where both rungs name the same identity.
--
-- Additive: one nullable column, no table rebuild, no CHECK widened, and the
-- backfill only fills rows where it is NULL. It holds a salted SHA-256 digest of
-- the application name under the domain separator 0017 defines -- the same value
-- `raw_event_buffer.app_stable_id` and `personal_app_override.app_key_hash`
-- already hold -- so it carries no recoverable raw text, no raw application
-- name, no window title and no URL. It never enters an upload DTO or a cloud
-- correction request; no type in `upload/dto.rs` has a field it could occupy.
ALTER TABLE personal_override ADD COLUMN app_key_hash TEXT
    CHECK (app_key_hash IS NULL OR length(app_key_hash) = 64);

-- The rules already on disk, repaired as far as the evidence honestly allows.
--
-- A correction whose events are still inside the TTL can have its pairing
-- recovered exactly as the writer would have recorded it, so those rules become
-- removable again on the next upgrade rather than only from here on. Older
-- corrections keep NULL: the raw application name is discarded at the
-- abstraction boundary and the two rungs are keyed in different hash domains, so
-- with no event left there is no honest way to recompute which app rung belongs
-- to which window rule -- the same reason 0035 records the paired flag at write
-- time instead of reconstructing it. Those app rungs keep classifying and Reset
-- still clears them.
--
-- `app_scope_eligible = 1` is required, so a browser-tab rule is left NULL: no
-- app rung was written for it, and pointing one at the browser's rung -- created
-- by some other correction -- would make removing one tab's rule delete it.
UPDATE personal_override
   SET app_key_hash = (
       SELECT event.app_stable_id
         FROM raw_event_buffer event
         JOIN abstraction_map map ON map.stable_id = event.stable_id
        WHERE map.key_hash = personal_override.key_hash
          AND event.app_stable_id IS NOT NULL
          AND event.app_scope_eligible = 1
          -- The application as well as the row, exactly as the writer resolves
          -- it: a browser also produces windows it read no site from, and those
          -- are eligible one row at a time while the browser is not.
          AND NOT EXISTS (
              SELECT 1 FROM raw_event_buffer ineligible
               WHERE ineligible.app_scope_eligible = 0
                 AND (ineligible.app_stable_id = event.app_stable_id
                      OR (event.app_bundle_stable_id IS NOT NULL
                          AND ineligible.app_bundle_stable_id
                              = event.app_bundle_stable_id))
          )
        ORDER BY event.occurred_at DESC
        LIMIT 1
   )
 WHERE app_key_hash IS NULL;
