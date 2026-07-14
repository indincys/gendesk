-- 批次 F：同步链路健康监测（F9）。
--
-- 「导出了但一直没回执」有两种可能：执行器没跑，或者同步软件根本没把包送过去。
-- 记下首次/最近一次回写时刻，看板就能把这两种情况分开说。
ALTER TABLE task_sheets ADD COLUMN first_receipt_at INTEGER;
ALTER TABLE task_sheets ADD COLUMN last_receipt_at INTEGER;
