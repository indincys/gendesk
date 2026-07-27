-- 提交回执留痕 + 首次提交时刻。forward-only。
--
-- 起点是一次真实事故（2026-07-27）：19 条提交出去，1 条真进了即梦队列并出片，
-- 另外 18 条在 `list_task` 里查得到、状态 `querying`、**但既没有 `queue_info` 也没有
-- `credit_count`**，挂了十几个小时。同参数、同通道复现一条新单，25 秒内就拿到
-- 队列位次（`queue_idx: 4485 / queue_length: 574522`）与 `credit_count: 8`。
-- 也就是说：那 18 条被即梦接单、却从未入队，也从未计费。
--
-- 事故本身在即梦侧，但 GenDesk 有三处放大了它，这份迁移补的是其中两处的数据基础：
--
-- 1. `submit_credit` / `submit_status` —— 提交回执原先**整个被丢掉**，`submit()` 只从
--    里面挑走 `submit_id`。于是「提交当时即梦怎么答的」事后无从查证，连「这条到底
--    计没计费」都只能靠轮询时再问一次。而实测健康的提交回执在 `querying` 阶段就带
--    `credit_count`（44 / 8 两个通道都验过）—— 它缺席本身就是最早可得的异常信号。
--
-- 2. `first_submitted_at` —— 「继续等待」(`resume_timed_out`) 会把 `submitted_at`
--    重置成当下（必须重置，否则下一轮立刻又判超时，按钮点了等于没点），代价是
--    **原始提交时刻被永久覆盖**。事故当天看板显示「最久已等 10 小时 54 分」，而那
--    只是从按下「继续等待」算起的；真实等待时间比它长，且再也查不出来了。
--    分成两列后：`submitted_at` 继续服务退避与超时判定（可被重置），
--    `first_submitted_at` 只在**换了新 submit_id** 时才更新，回答「这条到底等了多久」。
--
-- 存量行的 `first_submitted_at` 用 `submitted_at` 回填：对没被「继续等待」重置过的
-- 行它就是准确值，对被重置过的行它是一个已知偏小的下界 —— 都好过 NULL。
ALTER TABLE v2v_clips ADD COLUMN submit_credit INTEGER;
ALTER TABLE v2v_clips ADD COLUMN submit_status TEXT;
ALTER TABLE v2v_clips ADD COLUMN first_submitted_at INTEGER;

UPDATE v2v_clips SET first_submitted_at = submitted_at WHERE submitted_at IS NOT NULL;
