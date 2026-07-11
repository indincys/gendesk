-- 批次归档时刻（E22 / 决策 D3）：用于「归档满 N 天自动删除」的到期判定。
-- 历史已归档批次 archived_at 为 NULL——回填为 created_at 作为保守近似，避免其永不到期。
-- forward-only。
ALTER TABLE batches ADD COLUMN archived_at INTEGER;
UPDATE batches SET archived_at = created_at WHERE status = 'archived' AND archived_at IS NULL;
