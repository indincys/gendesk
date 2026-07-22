-- no-transaction
-- 生成页归档 + 单 Key 并发上限放宽。forward-only。
--
-- 本迁移必须在事务外执行（sqlx 的 `-- no-transaction` 指令，须为文件首行）：
-- 重建 api_keys 需要先关掉 PRAGMA foreign_keys，而该 PRAGMA 在事务内是 no-op。
--
-- (1) archived_at：点「开始生成」后，本批用到的提示词组与参考图随批次创建同事务打上归档
--     时间戳。归档只影响「生成页的选择器默认不再列出它们」——库里仍在、仍可查、可一键恢复。
--     解决：每批新导入一批临时组 + 新上传一批参考图，选择器越积越多，只能手动逐个删。
ALTER TABLE prompt_groups ADD COLUMN archived_at INTEGER;
ALTER TABLE ref_images ADD COLUMN archived_at INTEGER;
CREATE INDEX idx_prompt_groups_archived ON prompt_groups (archived_at);
CREATE INDEX idx_ref_images_archived ON ref_images (archived_at);

-- (2) 单 Key 并发上限 10 → 100。SQLite 改不了 CHECK，须按官方 12 步重建表。
--     关键：api_keys 是 tasks / task_attempts 的**父表**（ON DELETE SET NULL）。FK 开启时
--     DROP 父表会触发隐式 DELETE，把两张子表的 api_key_id 整列置空 —— 成功率统计与验收页
--     「按 Key」分组一并报废；且 RENAME 会改写子表的 REFERENCES 子句。关掉 foreign_keys
--     后两种副作用都不发生，子表的 `REFERENCES api_keys` 原样留存，重建完自然重新对上。
PRAGMA foreign_keys = OFF;

CREATE TABLE api_keys_new (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  name              TEXT NOT NULL,
  keyring_account   TEXT NOT NULL UNIQUE,
  base_url          TEXT NOT NULL,
  model             TEXT NOT NULL,
  concurrency_limit INTEGER NOT NULL DEFAULT 2 CHECK (concurrency_limit BETWEEN 1 AND 100),
  enabled           INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at        INTEGER NOT NULL,
  rpm_limit         INTEGER,
  circuit_broken    INTEGER NOT NULL DEFAULT 0
);

INSERT INTO api_keys_new
  (id, name, keyring_account, base_url, model, concurrency_limit, enabled, created_at,
   rpm_limit, circuit_broken)
  SELECT id, name, keyring_account, base_url, model, concurrency_limit, enabled, created_at,
         rpm_limit, circuit_broken
  FROM api_keys;

DROP TABLE api_keys;
ALTER TABLE api_keys_new RENAME TO api_keys;

PRAGMA foreign_keys = ON;
