-- 批次 B：区分「人工取消」与「风控熔断取消」（B2）。
--
-- 看板原来把「有任一 canceled 任务」判为账号熔断，人工取消一行也亮「当日熔断」。
-- 存量行按其产生路径无法区分，统一记为 risk：v0.8.0 之前 UI 没有人工取消入口，
-- 已有的 canceled 只可能来自 cancel_pending_of_account（风控熔断）。
ALTER TABLE publish_tasks ADD COLUMN cancel_kind TEXT;
UPDATE publish_tasks SET cancel_kind = 'risk' WHERE status = 'canceled';
