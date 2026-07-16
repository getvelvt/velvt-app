ALTER TABLE abstraction_map
    ADD COLUMN classification_status TEXT NOT NULL DEFAULT 'unclassified';
ALTER TABLE abstraction_map
    ADD COLUMN classification_confidence TEXT NOT NULL DEFAULT 'none';
ALTER TABLE abstraction_map
    ADD COLUMN classification_source TEXT NOT NULL DEFAULT 'fallback';

ALTER TABLE raw_event_buffer
    ADD COLUMN classification_status TEXT NOT NULL DEFAULT 'unclassified';
ALTER TABLE raw_event_buffer
    ADD COLUMN classification_confidence TEXT NOT NULL DEFAULT 'none';
ALTER TABLE raw_event_buffer
    ADD COLUMN classification_source TEXT NOT NULL DEFAULT 'fallback';

UPDATE abstraction_map SET
    classification_status = CASE classification_tier
        WHEN 'fallback' THEN 'unclassified' ELSE 'classified' END,
    classification_confidence = CASE classification_tier
        WHEN 'exact_match' THEN 'high'
        WHEN 'local_purpose_heuristic' THEN 'medium'
        WHEN 'embedding_similarity' THEN 'medium'
        ELSE 'none' END,
    classification_source = CASE classification_tier
        WHEN 'exact_match' THEN 'seed'
        WHEN 'local_purpose_heuristic' THEN 'heuristic'
        WHEN 'embedding_similarity' THEN 'embedding'
        ELSE 'fallback' END;

UPDATE raw_event_buffer SET
    classification_status = CASE classification_tier
        WHEN 'fallback' THEN 'unclassified' ELSE 'classified' END,
    classification_confidence = CASE classification_tier
        WHEN 'exact_match' THEN 'high'
        WHEN 'local_purpose_heuristic' THEN 'medium'
        WHEN 'embedding_similarity' THEN 'medium'
        ELSE 'none' END,
    classification_source = CASE classification_tier
        WHEN 'exact_match' THEN 'seed'
        WHEN 'local_purpose_heuristic' THEN 'heuristic'
        WHEN 'embedding_similarity' THEN 'embedding'
        ELSE 'fallback' END;

-- Earlier builds stored arbitrary app/title-derived values in these local
-- display columns. Convert them to the curated allowlist before they can be
-- rehydrated into ready-to-display IPC payloads.
UPDATE abstraction_map SET display_name = CASE
    WHEN label = 'communication:slack' THEN 'Slack'
    WHEN label = 'reference:github' THEN 'GitHub'
    WHEN label = 'document:docs' THEN 'Docs'
    WHEN label = 'document:write' AND lower(display_name) LIKE '%docs%' THEN 'Docs'
    WHEN label = 'reference:ai_assistant' THEN 'AI Assistant'
    WHEN label = 'reference:browser' THEN 'Browser'
    WHEN label IN ('document:edit', 'document:code')
         AND lower(display_name) IN ('vs code', 'visual studio code') THEN 'VS Code'
    ELSE NULL
END;

UPDATE raw_event_buffer SET local_display_label = (
    SELECT display_name FROM abstraction_map
    WHERE abstraction_map.stable_id = raw_event_buffer.stable_id
);
