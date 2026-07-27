import { type QueueStats, type V2vTick, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useCallback, useEffect, useState } from "react";

/**
 * 队列观测条 —— 「第二天醒来判断是还在排队，还是卡住报错了」。
 *
 * ## 这里为什么没有「前面还有 N 人在排队」
 *
 * 因为即梦不给。实测排队中的 `query_result` 只回 submit_id / prompt / logid /
 * gen_status；`list_task` 也只有状态；`queue_info.queue_idx` 只在**已完成**的回体里
 * 出现过（值 0、Finish）。解析代码留着以备它哪天开始回传，但界面上不能凭空造一个
 * 「第 N 位」——编出来的排队位次比没有更糟，因为人会拿它做决定。
 *
 * ## 那用什么代替
 *
 * 用**我们自己测得准**的两件事，而它们恰好就是判据本身：
 *
 * - **上次出片距今多久**。这是主信号：早上看到「20 分钟前」就知道队列在推进，
 *   看到「9 小时前」就知道该去查了。比「前面还有几个人」更直接地回答那个问题。
 * - **逐小时出片趋势**。柱子在长 = 在动；连着几根空 = 停了。
 *
 * 外加「最久那条等了多久」（绝对进度）与按实测速度算的粗略 ETA。
 */
export function V2vQueuePanel({
  tick,
  now,
  always,
}: {
  tick: V2vTick | null;
  now: number;
  /** 观测面板里要**始终**显示：人是特意点开来看的，「队列空」本身就是答案。 */
  always?: boolean;
}) {
  const [q, setQ] = useState<QueueStats | null>(null);
  /** 这份统计是什么时候取的。用它把秒数补齐到「此刻」，界面才不会一卡一卡地跳。 */
  const [at, setAt] = useState(0);

  const load = useCallback(() => {
    void unwrap(commands.v2vQueueStats())
      .then((v) => {
        setQ(v);
        setAt(Math.floor(Date.now() / 1000));
      })
      .catch(() => {});
  }, []);

  useEffect(load, [load]);
  // 心跳一到就重算：心跳本身是事件驱动的，这里不额外起轮询（铁律 4）。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是心跳时刻这个信号
  useEffect(load, [tick?.at, load]);

  if (!q) return null;
  if (q.running === 0 && !always) return null;

  // 统计取回后每过一秒，各项时长就该多一秒 —— 否则等待时长每 6 秒才跳一次，
  // 看着像卡住了，而「是不是卡住了」正是这块面板要回答的问题。
  const drift = Math.max(0, now - at);
  const stale = q.sinceLastFinish != null && q.sinceLastFinish + drift > 2 * 3600;
  const peak = Math.max(1, ...q.hourly);
  const nextPoll = q.nextPollIn == null ? null : Math.max(0, q.nextPollIn - drift);

  return (
    <div className={cn("qbar", always && "flat")}>
      <div className="fx ac gap10 wrap">
        <span className="fs12 fw6">
          {q.running === 0 ? "队列是空的" : `${q.running} 条在队列里`}
        </span>
        {q.running > 0 && (
          <span className="fs11 t3">
            最久已等 <b>{fmtDur(q.oldestWait + drift)}</b>
            {q.newestWait !== q.oldestWait && ` · 最新 ${fmtDur(q.newestWait + drift)}`}
          </span>
        )}
        <span className={cn("fs11", stale ? "qwarn" : "t3")}>
          {q.sinceLastFinish == null
            ? "这批还没有出过片"
            : `上次出片 ${fmtDur(q.sinceLastFinish + drift)}前`}
        </span>
        {q.etaSecs != null && (
          <span className="fs11 t3">按当前速度全部收完约需 {fmtDur(q.etaSecs)}</span>
        )}
        <div className="f1" />
        <span className="fs10 t3 nowrap">
          {nextPoll != null && nextPoll > 0 ? `下次查询 ${nextPoll} 秒后` : "正在查询…"}
          {q.timeoutHours == null ? " · 不设超时" : ` · 超过 ${q.timeoutHours} 小时判超时`}
        </span>
      </div>

      {/* 逐小时出片趋势：柱子在长 = 队列在动；连着几根空 = 停了。
          比一个总数有用，因为要判断的是「还在不在动」而不是「一共出了多少」。 */}
      <div className="fx ac gap8 mt8">
        <span className="fs10 t3 nowrap">近 12 小时出片</span>
        <div className="spark">
          {[...q.hourly].reverse().map((n, i) => (
            <div
              // biome-ignore lint/suspicious/noArrayIndexKey: 柱子就是按小时定位的，索引即身份
              key={i}
              className={cn("sparkb", n > 0 && "on")}
              style={{ height: `${Math.max(2, (n / peak) * 100)}%` }}
              title={`${q.hourly.length - 1 - i} 小时前：${n} 条`}
            />
          ))}
        </div>
        <span className="fs10 t3 nowrap">共 {q.hourly.reduce((a, b) => a + b, 0)} 条</span>
      </div>

      {stale && (
        <div className="fs11 mt8 qwarn" style={{ lineHeight: 1.7 }}>
          已经 {fmtDur((q.sinceLastFinish ?? 0) + drift)}{" "}
          没有新片落盘了。任务不会因此丢失（额度已扣、
          即梦那边照跑），但值得开「执行日志」看看最近几轮查询有没有报错。
        </div>
      )}
    </div>
  );
}

/** 秒 → 「3 小时 12 分」。与 Rust 侧 `runner::fmt_dur` 同一套读法。 */
function fmtDur(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  if (s < 60) return `${s} 秒`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟`;
  return `${Math.floor(s / 3600)} 小时 ${Math.floor((s % 3600) / 60)} 分`;
}
