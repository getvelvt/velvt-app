-- User-selected activity names stay beside local-only personal rules. They are
-- never selected into upload DTOs or cloud correction requests.
ALTER TABLE personal_override ADD COLUMN activity_name TEXT
    CHECK(activity_name IS NULL OR (
        length(trim(activity_name)) BETWEEN 1 AND 48
        AND instr(activity_name, char(10)) = 0
        AND instr(activity_name, char(13)) = 0
    ));
