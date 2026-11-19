-- Preserve gallery and video records while keeping archived assets out of active workflows.
ALTER TABLE ref_library_styles ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
ALTER TABLE v2v_clips ADD COLUMN archived_at INTEGER;

-- Older readers would re-expose archived items and could send them through active workflows.
UPDATE schema_meta SET value=MAX(value,59) WHERE key='reader_floor';

ALTER TABLE api_keys ADD COLUMN last_test_at INTEGER;
ALTER TABLE api_keys ADD COLUMN last_test_ok INTEGER;

ALTER TABLE v2v_clips ADD COLUMN prior_params_json TEXT;
ALTER TABLE v2v_clips ADD COLUMN prior_video_prompt TEXT;

CREATE TRIGGER trg_v2v_capture_prior_params
AFTER UPDATE OF model_version, duration, video_resolution ON v2v_clips
WHEN OLD.model_version IS NOT NEW.model_version OR OLD.duration IS NOT NEW.duration
     OR OLD.video_resolution IS NOT NEW.video_resolution
BEGIN
  UPDATE v2v_clips SET prior_params_json=json_object('modelVersion',OLD.model_version,
    'duration',OLD.duration,'videoResolution',OLD.video_resolution) WHERE id=NEW.id;
END;

CREATE TRIGGER trg_v2v_capture_prior_prompt
AFTER UPDATE OF video_prompt ON v2v_clips
WHEN OLD.video_prompt IS NOT NULL AND OLD.video_prompt IS NOT NEW.video_prompt
BEGIN
  UPDATE v2v_clips SET prior_video_prompt=OLD.video_prompt WHERE id=NEW.id;
END;
