-- Device-local source label used only by the menu-bar queue inspector.
-- It is never copied to abstraction mappings, upload batches, or HTTP DTOs.
ALTER TABLE raw_event_buffer ADD COLUMN local_display_label TEXT;

CREATE INDEX idx_raw_event_buffer_event_id_local_label
    ON raw_event_buffer(event_id, local_display_label);
