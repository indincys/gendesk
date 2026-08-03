-- 单 Key 图片生成并发上限 100 → 250。forward-only。
--
-- 不能重建 api_keys：它是 tasks / task_attempts 的父表，两张子表都是
-- ON DELETE SET NULL。即使迁移脚本尝试 PRAGMA foreign_keys=OFF，sqlx 的连接设置仍可能
-- 让 DROP 父表触发级联动作；结构与 foreign_key_check 都会显示正常，但历史 api_key_id
-- 已经被静默清空。这里改走 SQLite 原生 ADD COLUMN + RENAME COLUMN，全程不删除父表。

ALTER TABLE api_keys ADD COLUMN concurrency_limit_250 INTEGER NOT NULL DEFAULT 2
  CHECK (concurrency_limit_250 BETWEEN 1 AND 250);

-- 已经顶在旧上限 100 的 Key 随本次明确提额升到 250；用户自定义的较低值原样保留。
UPDATE api_keys
SET concurrency_limit_250 = CASE
  WHEN concurrency_limit = 100 THEN 250
  ELSE concurrency_limit
END;

-- 保留旧列，避免重建父表。后端继续读取名为 concurrency_limit 的新列；旧列只是一份
-- 兼容快照，新插入行走它的 DEFAULT 2，不参与任何业务读写。
ALTER TABLE api_keys RENAME COLUMN concurrency_limit TO concurrency_limit_legacy;
ALTER TABLE api_keys RENAME COLUMN concurrency_limit_250 TO concurrency_limit;
