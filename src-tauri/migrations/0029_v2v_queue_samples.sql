-- 0029：即梦排队位次的**时序采样**。forward-only。
--
-- 0021 给了 `v2v_clips.queue_idx`，但它只存「最新问到的那一个数」（`mark_polled` 走
-- COALESCE）。于是界面答得出「现在排第 4485 位」，答不出任何一个真正要用来排产的问题：
--
--   · 这条队在动吗，动得多快？
--   · 昨天下午提交的那批，从第几位排到第几位用了多久？
--   · 今晚几点提交，明早能出片？
--
-- 而这三个问题问的都是**位次对时间的导数**，一个标量存不下。
--
-- ## 为什么不是写进执行日志
--
-- 试过那条路的代价在 `dreamina.rs` 的 `run()` 注释里已经写着：执行日志是 500 条环形
-- 缓冲、重启即清空，把常规轮询结果也记进去，等于用「一切正常」把真正的报错挤出窗口。
-- 而「每条 clip 最多留 5 行」这个折中没有好的取法 —— 取前 5 条只知道起点，取后 5 条
-- 丢了起点，均匀采样又要在只能追加的缓冲里回头删。
-- 时序就该进表；日志只记转折（首次拿到位次 / 长时间不动 / 进入 Finish）。
--
-- ## queue_length 是全局队列长度，queue_idx 是这一单在其中的位次
--
-- 实测回体：`{queue_idx: 4485, priority: 1, queue_status: "Queueing", queue_length: 574522}`。
-- 也就是说**任何一条在跑的条目都在测同一条队** —— 于是跨 clip 汇总出来的
-- 「每小时消化多少位」就是非 VIP 通道此刻的真实速度，而不是某一单的运气。
--
-- ## 主键取 (clip_id, at)
--
-- 采样由轮询驱动（非 VIP 600 秒一轮），同一秒不会有第二个样本；用它做主键顺带保证
-- 重放/补扫不会写出重复点。ON DELETE CASCADE：clip 删了，它的轨迹没有单独留存的意义。
CREATE TABLE v2v_queue_samples (
    clip_id      INTEGER NOT NULL REFERENCES v2v_clips(id) ON DELETE CASCADE,
    at           INTEGER NOT NULL,
    queue_idx    INTEGER NOT NULL,
    queue_length INTEGER,
    PRIMARY KEY (clip_id, at)
);

-- 全局速度是按**墙钟窗口**跨 clip 聚合的，故按时间检索是主查询路径。
CREATE INDEX idx_v2v_queue_samples_at ON v2v_queue_samples(at);
