-- 批次 E：草稿保护（E6）需要知道「这张单是什么时候生成的」，
-- 才能判断之后有没有人工调整过（改时间/增补行/换套装）。
-- 存量单以 created_at 兜底：那之后的改动都算人工调整（宁可多问一次确认）。
ALTER TABLE task_sheets ADD COLUMN generated_at INTEGER;
UPDATE task_sheets SET generated_at = created_at WHERE generated_at IS NULL;
