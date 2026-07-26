-- 视频流水线的「实时进度」落库。forward-only。
--
-- 0020 之后，一条已提交的 clip 在库里只有 submit_id 与 submitted_at；它此刻在即梦那边是
-- 排队、在跑、还是卡住了，只活在轮询循环推出去的 `v2v://progress` 事件里，而那个事件
-- 只被前端存进一个 React state map —— 切页、刷新、重启，全部归零。于是看板上「已提交 19」
-- 旁边没有任何东西能回答「它们到底在干嘛」，这正是用户报的第一个问题。
--
-- 三列都是**别人系统的**状态快照，不是我们的业务真相，故：
-- - `gen_status` 存即梦返回的原文，不映射成自造的中文态（它加一个新态时，翻译层只会
--   显示成「未知」，而原文至少还能被搜索、被拿去问客服）。
-- - `queue_idx` 是队列位次，即梦只在排队时给。
-- - `polled_at` 是**我们**最后一次问到答案的时刻。有它才能区分「它还在排队」与
--   「我们已经十分钟没问出话来了」——后者是故障，前者不是。
ALTER TABLE v2v_clips ADD COLUMN gen_status TEXT;
ALTER TABLE v2v_clips ADD COLUMN queue_idx INTEGER;
ALTER TABLE v2v_clips ADD COLUMN polled_at INTEGER;
