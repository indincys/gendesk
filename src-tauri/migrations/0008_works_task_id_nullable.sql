-- accepted_works.task_id 原为 NOT NULL 却带 ON DELETE SET NULL，二者矛盾：删除关联任务时
-- 触发 NOT NULL 约束失败。E22（决策 D3）需在删除归档批次（级联删任务）时保留作品快照，
-- 故此处将 task_id 改为可空（作品是独立快照，任务被删后 task_id 置空，记录仍在）。
-- SQLite 无法直接放宽列约束，按官方 12 步以表重建实现。forward-only。
CREATE TABLE accepted_works_new (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id      INTEGER REFERENCES tasks (id) ON DELETE SET NULL,
  image_path   TEXT NOT NULL,
  thumb_path   TEXT NOT NULL,
  prompt_id    INTEGER,
  prompt_text  TEXT NOT NULL,
  group_id     INTEGER,
  ref_image_id INTEGER,
  batch_id     INTEGER,
  favorite     INTEGER NOT NULL DEFAULT 0 CHECK (favorite IN (0, 1)),
  accepted_at  INTEGER NOT NULL
);
INSERT INTO accepted_works_new
  SELECT id, task_id, image_path, thumb_path, prompt_id, prompt_text,
         group_id, ref_image_id, batch_id, favorite, accepted_at
  FROM accepted_works;
DROP TABLE accepted_works;
ALTER TABLE accepted_works_new RENAME TO accepted_works;
CREATE INDEX idx_accepted_works_group ON accepted_works (group_id);
