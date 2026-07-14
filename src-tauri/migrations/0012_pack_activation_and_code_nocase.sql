-- 批次 A：素材包生命周期闭环（A1）+ SKU 编码大小写唯一（A5）。

-- A1 存量解锁：入库默认 lifecycle 由 new 改为 active（文件齐备即可发），
-- 历史入库但从未人工「完善」的包一并转可用，否则它们永远不会被排期选中。
UPDATE asset_packs SET lifecycle = 'active', updated_at = strftime('%s','now')
 WHERE lifecycle = 'new';

-- A5 大小写唯一：Windows 文件系统大小写不敏感，`sf-1` 与 `SF-1` 会争抢
-- 资产库/{编码}/ 同一个目录。存量库若已有大小写重复的编码，本索引会创建失败、
-- 迁移中止 —— 这是刻意的：请先人工把重复编码改名（保留其一），再重启应用。
CREATE UNIQUE INDEX IF NOT EXISTS idx_skus_code_nocase ON skus (LOWER(code));
