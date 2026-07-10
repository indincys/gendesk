-- 抽卡序号（E17 / 决策 D2）：同一「参考图 × 提示词」组合独立生成 k 次，
-- 每次一个独立任务，draw_index ∈ 1..k。输出文件名追加该序号以避免同组合多张通过时冲突。
-- forward-only。历史任务默认 1。
ALTER TABLE tasks ADD COLUMN draw_index INTEGER NOT NULL DEFAULT 1;
