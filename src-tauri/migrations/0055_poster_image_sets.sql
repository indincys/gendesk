-- 审稿通过的同一组图在 Poster 里组成一条图文（图片包），不再只散着进图片库。
-- 一组 = 同一次提交（GenDesk 批次）× 同一款式 × 同一需求（或显式 posterType）：同一套背景，天然是一套图。
-- 同组可能分几次点通过，所以一条图文是一个独立的 Poster 批次，跨多次验收收图，等这一组没有图再待审时才组成。
-- 只加可空列，旧版本应用读写不受影响：set_key 为空的批次就是原来的散图批次。
ALTER TABLE poster_batches ADD COLUMN set_key TEXT;
-- Poster 回执里这条图文的素材 ID（对账用）。
ALTER TABLE poster_batches ADD COLUMN material_id TEXT;
-- 图文内顺序按任务编号（审稿页同组的显示顺序），第 1 张是封面。
ALTER TABLE poster_images ADD COLUMN task_id INTEGER;
CREATE INDEX idx_poster_batches_set_key ON poster_batches(set_key) WHERE set_key IS NOT NULL;
