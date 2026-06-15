-- Add not_found flag to insight_cache to support negative caching of 404 responses.
-- A not_found entry (not_found = 1) prevents repeated API calls when no approved
-- insight exists for a date. The flag uses a shorter TTL than positive entries.
ALTER TABLE insight_cache ADD COLUMN not_found INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_insight_cache_not_found
    ON insight_cache(date, not_found);
