-- A row survives retirement of the GenDesk task it was registered for.
CREATE TABLE codex_reviews (
    task_id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL,
    sample_job_id TEXT NOT NULL,
    poster_group_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    cwd TEXT NOT NULL,
    decision TEXT CHECK (decision IN ('pass', 'reject')),
    decided_at INTEGER,
    delivered_at INTEGER,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
CREATE INDEX idx_codex_reviews_group ON codex_reviews (poster_group_id, task_id);
CREATE INDEX idx_codex_reviews_pending ON codex_reviews (next_attempt_at)
    WHERE decision IS NOT NULL AND delivered_at IS NULL;
