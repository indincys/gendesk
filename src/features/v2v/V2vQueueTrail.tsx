import { fmtSpan } from "@/features/v2v/model";
import { type ClipQueueTrail, commands, unwrap } from "@/lib/ipc";
import { useEffect, useState } from "react";

/**
 * 一条 clip 的排队位次轨迹。
 *
 * ## 为什么详情栏光有「第 4485 位」不够
 *
 * 因为那个数字单独看**没有信息量**。同样是第 4485 位，队列每小时消化两千位时是
 * 「今晚就能出片」，每小时消化二十位时是「后天见」——而这两个结论指挥的是完全相反的
 * 排产动作。区别只存在于位次对时间的斜率里，一个标量存不下，也就显示不出来。
 *
 * 所以这里画的是**折线**，旁边那句话是它的导数。
 *
 * ## 少于两点时不画线
 *
 * 一个点算不出斜率。这时只说「已入队 · 第 N 位 · 还在等下一次采样」，
 * **绝不用一条水平线冒充「队列没动」**——那是一个结论，而此刻我们没有结论。
 */
export function V2vQueueTrail({ clipId, queueIdx }: { clipId: number; queueIdx: number | null }) {
  const [trail, setTrail] = useState<ClipQueueTrail | null>(null);

  useEffect(() => {
    let alive = true;
    setTrail(null);
    // 只要这一条的轨迹：`hours` 对 clip 那一半不起作用（它按 clip_id 全取），
    // 传 24 只是为了让同一条命令的全局那一半不至于扫全表。
    void unwrap(commands.v2vQueueTrend(24, clipId))
      .then(([, t]) => alive && setTrail(t))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [clipId]);

  const pts = trail?.points ?? [];
  if (pts.length === 0) {
    return (
      <div className="fs10 t3 mt4">
        {queueIdx == null ? "还没问到排队位次" : "刚入队，还没有第二个采样点"}
      </div>
    );
  }

  const idxs = pts.map((p) => p.queueIdx);
  const hi = Math.max(...idxs);
  const lo = Math.min(...idxs);
  const t0 = pts[0]?.at ?? 0;
  const span = Math.max(1, (pts[pts.length - 1]?.at ?? 0) - t0);
  // 位次**越小越靠前**，所以 y 要翻过来：线往下走 = 在前进，与人的直觉一致。
  const range = Math.max(1, hi - lo);
  const d = pts
    .map((p, i) => {
      const x = ((p.at - t0) / span) * 100;
      const y = ((p.queueIdx - lo) / range) * 100;
      return `${i === 0 ? "M" : "L"}${x.toFixed(2)} ${y.toFixed(2)}`;
    })
    .join(" ");

  const rate = trail?.ratePerHour ?? null;
  const eta = trail?.etaSecs ?? null;

  return (
    <div className="qtrail mt4">
      {pts.length >= 2 && (
        <svg viewBox="0 0 100 100" preserveAspectRatio="none" aria-label="排队位次轨迹">
          <path d={d} />
        </svg>
      )}
      <div className="fs10 t3">
        {pts.length < 2 ? (
          `已采样 1 次 · 第 ${idxs[0]} 位`
        ) : (
          <>
            {/* 三个数出自**同一段采样**（入队到此刻）：走了多少位 · 平均多快 · 还剩多久。
                速度从前取的是「近 1 小时」，而旁边这条曲线画的一直是全程 ——
                「已前进 3200 位」配「近 1 小时 40 位/时」两个都对，并排读却像在说
                队列刚停住了。现在统一显示这条任务从入队起的平均速度。 */}
            {hi - lo > 0 ? `已前进 ${hi - lo} 位` : "位次尚未变化"}
            {rate != null && rate > 0 && ` · 全程均速 ${Math.round(rate)} 位/时`}
            {eta != null && ` · 按此速度约还需 ${fmtSpan(eta)}`}
          </>
        )}
      </div>
    </div>
  );
}
