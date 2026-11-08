-- Stage identities commit with their imported rows. Never delete a checkpoint to replay a job.
CREATE TABLE intake_checkpoints (
    job_id TEXT PRIMARY KEY REFERENCES intake_jobs(job_id) ON DELETE CASCADE,
    state_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
