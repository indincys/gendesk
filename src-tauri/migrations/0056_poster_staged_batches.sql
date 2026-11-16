CREATE TABLE poster_intake_groups (
    id TEXT PRIMARY KEY NOT NULL,
    closed INTEGER NOT NULL DEFAULT 0 CHECK (closed IN (0, 1))
);
ALTER TABLE batches ADD COLUMN poster_group_id TEXT REFERENCES poster_intake_groups(id);
CREATE INDEX idx_batches_poster_group ON batches(poster_group_id) WHERE poster_group_id IS NOT NULL;

-- Older clients treat a string in poster_batches.set_key as having no pending tasks
-- and would deliver the sample early. They must not open a library with staged groups.
UPDATE schema_meta SET value = MAX(value, 56) WHERE key='reader_floor';
