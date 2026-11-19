-- awaiting 继续表示未经用户确认。排队的编辑稿也有新顺序和排除语义，旧读者会按旧规则发送，
-- 因此此迁移需要抬高 reader_floor，即使结构本身只增加列。
ALTER TABLE poster_images ADD COLUMN draft_position INTEGER;
ALTER TABLE poster_images ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0 CHECK(excluded IN (0,1));
ALTER TABLE poster_batches ADD COLUMN draft_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE poster_batches ADD COLUMN draft_confirmed INTEGER NOT NULL DEFAULT 0;
UPDATE schema_meta SET value = MAX(value, 58) WHERE key = 'reader_floor';
