-- 删除款式时从图库和工单候选中移出整组，保留图片原件与用量引用用于恢复。
ALTER TABLE ref_library_styles ADD COLUMN deleted_at INTEGER;
-- 旧应用会继续按名称与编号使用已删除的款式。
UPDATE schema_meta SET value = MAX(value, 60) WHERE key = 'reader_floor';
