import { type CreditDayView, type QueueTrend, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useEffect, useState } from "react";

/**
 * 非 VIP 队列的整体观测 —— 「什么时候提交最划算」。
 *
 * ## 为什么这块能跨条目汇总
 *
 * `queue_idx` 是**全局队列**里的位次（同一份回体里还有 `queue_length: 574522`），
 * 所以每一条在跑的条目都在测同一条队。把各自的斜率按小时归桶取中位数，得到的就是
 * 那个小时里非 VIP 通道的真实消化速度 —— 于是「凌晨三点比晚上八点快一倍」这种事
 * 才有地方看得出来，而那正是排生产队列要的输入。
 *
 * ## 没有采样的小时是**缺席**，不是 0
 *
 * 补 0 会画出一条「那时候队列停了」的假线，而真相只是那时候我们没在跑。
 * 一根不存在的柱子比一根骗人的柱子好。
 */
export function V2vQueueTrend({ hours }: { hours: number }) {
  const [trend, setTrend] = useState<QueueTrend | null>(null);

  useEffect(() => {
    let alive = true;
    void unwrap(commands.v2vQueueTrend(hours, null))
      .then(([t]) => alive && setTrend(t))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [hours]);

  if (!trend || trend.samples === 0) {
    return (
      <div className="fs11 t3" style={{ lineHeight: 1.8 }}>
        还没有排队位次采样。提交一条非 VIP 的条目，两轮查询（约 20 分钟）之后这里就会有曲线。
      </div>
    );
  }

  const peak = Math.max(1, ...trend.hourly.map((h) => Math.abs(h.positionsPerHour)));
  // 覆盖不足两小时时只给读数不给结论：一个从半小时样本里外推出来的「最佳提交时段」
  // 会被人当真，而它其实只是噪声。
  const thin = trend.spanSecs < 2 * 3600;
  const best = thin
    ? null
    : trend.hourly.reduce<(typeof trend.hourly)[number] | null>(
        (a, b) => (a == null || b.positionsPerHour > a.positionsPerHour ? b : a),
        null,
      );

  return (
    <div>
      <div className="fx ac gap8">
        <span className="fs11 t3 nowrap">逐小时消化速度（位/时）</span>
        <div className="spark">
          {trend.hourly.map((h) => (
            <div
              key={h.hourStart}
              className={cn("sparkb", h.positionsPerHour > 0 && "on")}
              style={{ height: `${Math.max(2, (h.positionsPerHour / peak) * 100)}%` }}
              title={`${fmtHour(h.hourStart)} · ${Math.round(h.positionsPerHour)} 位/时 · ${h.clips} 条在测`}
            />
          ))}
        </div>
        <span className="fs10 t3 nowrap">{trend.samples} 个采样</span>
      </div>

      <div className="fs11 t3 mt8" style={{ lineHeight: 1.8 }}>
        {thin ? (
          <>观测跨度还不到两小时，只够给读数、不够给结论。挂着跑一夜再看这里。</>
        ) : (
          best != null && (
            <>
              最近 {Math.round(trend.spanSecs / 3600)} 小时里，队列最快的那一小时是{" "}
              <b>{fmtHour(best.hourStart)}</b>（{Math.round(best.positionsPerHour)} 位/时
              {best.clips === 1 && "，只有 1 条在测，信心有限"}）。
            </>
          )
        )}
      </div>

      {trend.entries.length > 0 && (
        <div className="fs11 t3 mt8" style={{ lineHeight: 1.8 }}>
          {/* 入队位次是排产的另一半：「这个点提交，队列有多深」。
              与消化速度相除就是等待时长，而那才是真正要排的东西。 */}
          入队时队列深度：
          {trend.entries.slice(-6).map((e) => (
            <span key={`${e.at}-${e.queueIdx}`} className="chip" style={{ marginLeft: 5 }}>
              {fmtHour(e.at)} 第 {e.queueIdx} 位
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * 每日额度台账。
 *
 * 它同时是一个实验的读数：即梦「每天登录送 80」这件事，CLI 里**没有**任何领取命令
 * （实测 `dreamina -h`），所以只剩一个可证伪的假设 —— 服务端在检测到有效登录态时
 * 自动发放。`delta`（余额差 + 期间本机花掉）就是那个假设的读数：连着几天稳定 ≈ +80
 * 就说明「后台常驻 + 每天调一次 CLI」本身已经把这件事做完了；≈ 0 则说明必须走网页领取。
 *
 * **首日不给结论**：delta 需要一个对比基准，而首条没有。
 */
export function V2vCreditDaily() {
  const [days, setDays] = useState<CreditDayView[] | null>(null);

  useEffect(() => {
    let alive = true;
    void unwrap(commands.v2vCreditDaily(14))
      .then((d) => alive && setDays(d))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  if (!days || days.length === 0) {
    return (
      <div className="fs11 t3" style={{ lineHeight: 1.8 }}>
        还没有额度快照。应用每天第一次跑轮询循环时记一条，明天这里就会有第一行。
      </div>
    );
  }

  const withDelta = days.filter((d) => d.delta !== null);
  const avg =
    withDelta.length === 0
      ? null
      : withDelta.reduce((a, d) => a + (d.delta ?? 0), 0) / withDelta.length;
  const peak = Math.max(1, ...days.map((d) => Math.abs(d.delta ?? 0)));

  return (
    <div>
      <div className="fx ac gap8">
        <span className="fs11 t3 nowrap">每日进账</span>
        <div className="spark">
          {days.map((d) => (
            <div
              key={d.day}
              className={cn("sparkb", (d.delta ?? 0) > 0 && "on")}
              style={{ height: `${Math.max(2, (Math.abs(d.delta ?? 0) / peak) * 100)}%` }}
              title={`${d.day} · 余额 ${d.balance} · 进账 ${d.delta ?? "—"} · 本机花掉 ${d.spentSincePrev}`}
            />
          ))}
        </div>
        <span className="fs10 t3 nowrap">余额 {days[days.length - 1]?.balance ?? "—"}</span>
      </div>
      <div className="fs11 t3 mt8" style={{ lineHeight: 1.8 }}>
        {withDelta.length === 0 ? (
          <>只有一条快照，还没有可比的基准 —— 明天这一行才会有结论。</>
        ) : (
          <>
            近 {withDelta.length} 天平均每日进账 <b>{Math.round(avg ?? 0)}</b> 额度。
            {avg != null && avg > 40 ? (
              <>
                {" "}
                看起来<b>只要保持登录态、每天调一次 CLI 就会自动到账</b>
                ，不需要额外做领取动作 —— 应用后台常驻着这件事就已经成立了。
              </>
            ) : (
              <>
                {" "}
                没有观察到稳定的每日进账。CLI 里没有领取命令，所以如果确实有这份赠送，
                它多半要在网页上手动领。
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** unix 秒 → 本地「MM-DD HH 时」。趋势看的是「几点」，不需要分秒。 */
function fmtHour(unix: number): string {
  const d = new Date(unix * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())} 时`;
}
