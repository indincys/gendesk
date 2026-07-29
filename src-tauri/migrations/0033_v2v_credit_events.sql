-- 视频生成消费事件账本。forward-only、append-only。
--
-- `v2v_clips` 是当前任务状态，不是账本：任务删除、退回改写或再次提交都会覆盖
-- submit_id / credit_count。这里每次只 INSERT 事实事件，任务删掉后历史仍保留。
CREATE TABLE v2v_credit_events (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  clip_id      INTEGER NOT NULL,
  submit_id    TEXT NOT NULL,
  channel_key  TEXT NOT NULL DEFAULT '',
  charged_at   INTEGER NOT NULL,
  event_type   TEXT NOT NULL
               CHECK (event_type IN ('submit','charge','pending','pass','rej','failed','abandoned')),
  credits      INTEGER,
  is_backfill  INTEGER NOT NULL DEFAULT 0 CHECK (is_backfill IN (0,1))
);

CREATE INDEX idx_v2v_credit_events_charged
  ON v2v_credit_events(charged_at);
CREATE INDEX idx_v2v_credit_events_channel
  ON v2v_credit_events(channel_key, charged_at);
CREATE UNIQUE INDEX idx_v2v_credit_event_once
  ON v2v_credit_events(submit_id, event_type)
  WHERE event_type NOT IN ('charge','pending');
CREATE UNIQUE INDEX idx_v2v_credit_event_charge
  ON v2v_credit_events(submit_id, event_type, credits)
  WHERE event_type = 'charge';

-- 存量只能尽力回填：已经删除或曾被就地覆盖的旧提交没有任何可恢复凭证。
INSERT OR IGNORE INTO v2v_credit_events
  (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
SELECT
  id,
  CASE
    WHEN submit_id IS NOT NULL AND TRIM(submit_id) <> '' THEN submit_id
    ELSE 'legacy:' || id || ':' || COALESCE(first_submitted_at, submitted_at, finished_at, created_at)
  END,
  COALESCE(NULLIF(TRIM(model_version), ''), ''),
  COALESCE(first_submitted_at, submitted_at, finished_at, created_at),
  'submit',
  COALESCE(credit_count, submit_credit),
  1
FROM v2v_clips
WHERE COALESCE(credit_count, submit_credit) IS NOT NULL;

INSERT OR IGNORE INTO v2v_credit_events
  (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
SELECT
  id,
  CASE
    WHEN submit_id IS NOT NULL AND TRIM(submit_id) <> '' THEN submit_id
    ELSE 'legacy:' || id || ':' || COALESCE(first_submitted_at, submitted_at, finished_at, created_at)
  END,
  COALESCE(NULLIF(TRIM(model_version), ''), ''),
  COALESCE(reviewed_at, finished_at, updated_at),
  CASE stage WHEN 'pass' THEN 'pass' WHEN 'rej' THEN 'rej' ELSE 'failed' END,
  NULL,
  1
FROM v2v_clips
WHERE COALESCE(credit_count, submit_credit) IS NOT NULL
  AND stage IN ('pass','rej','fail');

-- 新 submit_id 即一次新的消费尝试。实际额度有时在提交回体缺席，稍后以 charge 事件补齐。
CREATE TRIGGER trg_v2v_credit_event_submit
AFTER UPDATE OF submit_id ON v2v_clips
WHEN NEW.submit_id IS NOT NULL
 AND TRIM(NEW.submit_id) <> ''
 AND (OLD.submit_id IS NULL OR OLD.submit_id <> NEW.submit_id)
BEGIN
  INSERT OR IGNORE INTO v2v_credit_events
    (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
  VALUES (
    NEW.id,
    NEW.submit_id,
    COALESCE(NULLIF(TRIM(NEW.model_version), ''), ''),
    COALESCE(NEW.first_submitted_at, NEW.submitted_at, NEW.updated_at),
    'submit',
    COALESCE(NEW.credit_count, NEW.submit_credit),
    0
  );
END;

-- 轮询拿到真账单后追加 charge 事件；重复回体由唯一键幂等去重。
CREATE TRIGGER trg_v2v_credit_event_amount
AFTER UPDATE OF credit_count, submit_credit ON v2v_clips
WHEN NEW.submit_id IS NOT NULL
 AND TRIM(NEW.submit_id) <> ''
 AND COALESCE(NEW.credit_count, NEW.submit_credit) IS NOT NULL
BEGIN
  INSERT OR IGNORE INTO v2v_credit_events
    (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
  VALUES (
    NEW.id,
    NEW.submit_id,
    COALESCE(NULLIF(TRIM(NEW.model_version), ''), ''),
    COALESCE(NEW.first_submitted_at, NEW.submitted_at, NEW.updated_at),
    'charge',
    COALESCE(NEW.credit_count, NEW.submit_credit),
    0
  );
END;

-- 阶段结论只追加事件；同一提交失败后继续等待会追加 pending，最终再以最新结论为准。
CREATE TRIGGER trg_v2v_credit_event_outcome
AFTER UPDATE OF stage ON v2v_clips
WHEN NEW.submit_id IS NOT NULL
 AND TRIM(NEW.submit_id) <> ''
 AND OLD.stage <> NEW.stage
 AND NEW.stage IN ('run','rev','pass','rej','fail')
BEGIN
  INSERT OR IGNORE INTO v2v_credit_events
    (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
  VALUES (
    NEW.id,
    NEW.submit_id,
    COALESCE(NULLIF(TRIM(NEW.model_version), ''), ''),
    COALESCE(NEW.reviewed_at, NEW.finished_at, NEW.updated_at),
    CASE NEW.stage
      WHEN 'pass' THEN 'pass'
      WHEN 'rej' THEN 'rej'
      WHEN 'fail' THEN 'failed'
      ELSE 'pending'
    END,
    NULL,
    0
  );
END;

-- 清掉/替换仍未结算的提交单，追加 abandoned。账本自身从不 UPDATE/DELETE。
CREATE TRIGGER trg_v2v_credit_event_abandon
AFTER UPDATE OF submit_id ON v2v_clips
WHEN OLD.submit_id IS NOT NULL
 AND TRIM(OLD.submit_id) <> ''
 AND (NEW.submit_id IS NULL OR NEW.submit_id <> OLD.submit_id)
 AND COALESCE((
   SELECT event_type FROM v2v_credit_events
    WHERE submit_id=OLD.submit_id
      AND event_type IN ('pending','pass','rej','failed','abandoned')
    ORDER BY id DESC LIMIT 1
 ), 'pending') = 'pending'
BEGIN
  INSERT OR IGNORE INTO v2v_credit_events
    (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
  VALUES (
    OLD.id,
    OLD.submit_id,
    COALESCE(NULLIF(TRIM(OLD.model_version), ''), ''),
    NEW.updated_at,
    'abandoned',
    NULL,
    0
  );
END;

CREATE TRIGGER trg_v2v_credit_event_delete
BEFORE DELETE ON v2v_clips
WHEN OLD.submit_id IS NOT NULL
 AND TRIM(OLD.submit_id) <> ''
 AND COALESCE((
   SELECT event_type FROM v2v_credit_events
    WHERE submit_id=OLD.submit_id
      AND event_type IN ('pending','pass','rej','failed','abandoned')
    ORDER BY id DESC LIMIT 1
 ), 'pending') = 'pending'
BEGIN
  INSERT OR IGNORE INTO v2v_credit_events
    (clip_id, submit_id, channel_key, charged_at, event_type, credits, is_backfill)
  VALUES (
    OLD.id,
    OLD.submit_id,
    COALESCE(NULLIF(TRIM(OLD.model_version), ''), ''),
    OLD.updated_at,
    'abandoned',
    NULL,
    0
  );
END;
