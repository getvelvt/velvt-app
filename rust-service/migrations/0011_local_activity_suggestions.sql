-- A short-lived, device-local naming hint derived from the raw application
-- metadata already processed at ingestion. It is never copied to upload tables.
ALTER TABLE raw_event_buffer ADD COLUMN local_name_suggestion TEXT
    CHECK(local_name_suggestion IS NULL OR (
        length(trim(local_name_suggestion)) BETWEEN 1 AND 48
        AND instr(local_name_suggestion, char(10)) = 0
        AND instr(local_name_suggestion, char(13)) = 0
    ));
