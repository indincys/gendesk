-- 批次备注名（E10）：批次切换器行内可编辑，便于区分不同日期/用途的批次。forward-only。
ALTER TABLE batches ADD COLUMN note TEXT;
