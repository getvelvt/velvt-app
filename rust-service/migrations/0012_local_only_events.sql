-- Events captured before authentication provide local first value but must
-- never be retroactively synchronized without an explicit consent flow.
ALTER TABLE raw_event_buffer
    ADD COLUMN upload_eligible INTEGER NOT NULL DEFAULT 1
    CHECK (upload_eligible IN (0, 1));

CREATE INDEX idx_raw_event_buffer_upload_eligible_occurred_at
    ON raw_event_buffer(upload_eligible, occurred_at);
