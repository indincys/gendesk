-- 0030：每日额度快照台账。forward-only。
--
-- ## 它要回答的问题
--
-- 「即梦每天登录送 80 积分，能不能靠 CLI 自动领？」——实测 `dreamina -h` 的命令面里
-- 根本没有签到/领取这类命令（只有 login/relogin/logout/session/user_credit/list_task/
-- query_result + 生成类），`user_credit` 也只回 `{total_credit, user_id, user_name,
-- vip_level}`。所以「能不能自动领」不是一个能靠读文档回答的问题，只剩一个可证伪的假设：
-- **即梦可能在检测到有效登录态时由服务端自动发放**。
--
-- 这张表就是那个实验：每天记一次余额，加上同期本机的扣费，算出「凭空多出来多少」。
-- 连着几天稳定 ≈ +80 → 假设成立，那么「保持登录 + 每天调一次 CLI」本身就是全部实现，
-- 一行领取代码都不用写。≈ 0 → 假设被推翻，得走网页领取，届时再单独评估。
--
-- ## 为什么 delta 要把本机扣费加回去
--
-- 余额差 = 进账 − 花掉。而「花掉」这一半我们自己有账（`v2v_clips.credit_count`），
-- 减掉它剩下的才是进账。不这么算的话，一天跑了 200 额度的机器上，
-- 到账的 80 会被淹没在 −120 里，实验直接失去分辨力。
--
-- ## day 按本地时区切
--
-- 「每天」是人的每天，不是 UTC 的每天。北京时间早八点属于当天，而 `and_utc()`
-- 会把它当成 UTC 八点（CLAUDE.md 已记这条坑）。故 day 存 `YYYY-MM-DD` 本地日期字符串
-- 而不是时间戳除以 86400。
--
-- 一天一行、一天一次 CLI 调用，量级可以忽略；它同时也是排产要用的每日预算基线。
CREATE TABLE v2v_credit_daily (
    -- 本地日期 `YYYY-MM-DD`。一天一行，重复写同一天是 no-op（见 repo 的 INSERT OR IGNORE）。
    day               TEXT PRIMARY KEY,
    -- 真正问到余额的那一刻。快照不是恰好零点取的，算 delta 时要用它配对扣费窗口。
    at                INTEGER NOT NULL,
    balance           INTEGER NOT NULL,
    -- 上一条快照到这一条之间，本机流水线花掉的额度。
    spent_since_prev  INTEGER NOT NULL DEFAULT 0,
    -- 凭空进账 = (本次余额 − 上次余额) + 这期间花掉的。首条没有上一条可比，故为 NULL
    -- —— 留空而不是填 0：0 是一个结论（「没进账」），而首日我们没有结论。
    delta             INTEGER
);
