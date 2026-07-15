ALTER TABLE abstraction_map
    ADD COLUMN classification_tier TEXT NOT NULL DEFAULT 'fallback';
ALTER TABLE raw_event_buffer
    ADD COLUMN classification_tier TEXT NOT NULL DEFAULT 'fallback';
ALTER TABLE batch_event
    ADD COLUMN classification_tier TEXT NOT NULL DEFAULT 'fallback';
