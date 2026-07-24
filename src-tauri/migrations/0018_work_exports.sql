-- 作品导出台账（跨包去重）。forward-only。
--
-- 「哪些验收图已经做过图生视频」必须有真相源。包内的 ledger.jsonl 只管得住包内，
-- 包被移走/删掉/重建之后就失忆了，于是同一张图会被反复导出、反复花即梦额度。
--
-- 只新增表，不 ALTER 任何既有表：channel 留给未来别的下游（发布/纯导出），
-- 不给 accepted_works 加 `v2v_exported_at` 这种一次性布尔列——第二个下游来了就要再加一列。
--
-- work_id 不设 FK：作品可以进废纸篓/被删，台账要留痕回答「这张图当时导出过」，
-- 不能被级联抹掉；孤儿行无害（只参与「排除已导出」的 NOT EXISTS 判定）。
CREATE TABLE work_exports (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id     INTEGER NOT NULL,
  -- 下游渠道；当前只有 'i2v'（图生视频）。
  channel     TEXT    NOT NULL,
  -- 包目录名（相对导出根），例：260724-GD4-G-Dragon。
  pack_id     TEXT    NOT NULL,
  -- 包内条目主键，例：W1032。
  item_id     TEXT    NOT NULL,
  exported_at INTEGER NOT NULL
);

-- 「这张图在这个渠道导出过吗」——导出预检与「排除已导出」筛选都走这条。
CREATE INDEX idx_work_exports_work_channel ON work_exports (work_id, channel);
-- 同一包内 item_id 唯一：重复导出同一包不产生重影台账。
CREATE UNIQUE INDEX idx_work_exports_pack_item ON work_exports (pack_id, item_id);
