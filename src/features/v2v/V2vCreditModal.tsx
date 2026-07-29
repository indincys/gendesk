import { Modal } from "@/components/ui/Modal";
import { DescriptionHint, Tooltip } from "@/components/ui/Tooltip";
import { type CreditRange, type V2vCreditReport, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useCallback, useEffect, useState } from "react";

const RANGES: { key: CreditRange; label: string }[] = [
  { key: "7d", label: "近 7 天" },
  { key: "30d", label: "近 30 天" },
  { key: "all", label: "全部" },
];

export function V2vCreditModal({ onClose }: { onClose: () => void }) {
  const [range, setRange] = useState<CreditRange>("30d");
  const [report, setReport] = useState<V2vCreditReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (next: CreditRange) => {
    setLoading(true);
    setError(null);
    try {
      setReport(await unwrap(commands.v2vCreditReport(next)));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(range);
  }, [load, range]);

  const maxTrend = Math.max(1, ...(report?.trend.map((p) => p.spent) ?? [0]));
  const pct = report?.passRate == null ? "—" : `${Math.round(report.passRate * 100)}%`;

  return (
    <Modal
      title="余额与消费"
      width="w700"
      onClose={onClose}
      headerExtra={
        <div className="cranges" aria-label="统计范围">
          {RANGES.map((r) => (
            <button
              key={r.key}
              type="button"
              className={cn("crange", range === r.key && "on")}
              aria-pressed={range === r.key}
              onClick={() => setRange(r.key)}
            >
              {r.label}
            </button>
          ))}
        </div>
      }
      footer={
        <>
          <span className="fs10 t3">完整统计自升级日起；较早数据仅包含现存任务回填。</span>
          <div className="f1" />
          <button type="button" className="btn sm pri" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      <div className="creditreport" aria-busy={loading}>
        {error && <div className="vhint er">{error}</div>}
        <div className="creditmetrics">
          <Metric label="账户余额" value={report?.balance == null ? "—" : `${report.balance}`} />
          <Metric label="区间消耗" value={`${report?.spentTotal ?? 0}`} />
          <Metric
            label="验收通过率"
            value={pct}
            title="通过 ÷（通过 + 拒绝）；待验收、失败和放弃不计入分母"
          />
        </div>

        {report?.balanceError && (
          <div className="fs10 wr2">余额暂不可用：{report.balanceError}</div>
        )}

        <section className="creditsection">
          <div className="creditsectionhd">
            <span>消耗趋势</span>
            {loading && <span className="spn s9" />}
          </div>
          <div className="credittrend">
            {(report?.trend ?? []).map((point) => (
              <Tooltip key={point.bucket} content={`${point.bucket} · ${point.spent} 额度`}>
                <button
                  type="button"
                  className="credittrendcol"
                  aria-label={`${point.bucket} · ${point.spent} 额度`}
                >
                  <div
                    className="credittrendbar"
                    style={{ height: `${Math.max(4, (point.spent / maxTrend) * 100)}%` }}
                  />
                  <span>{point.bucket.slice(range === "all" ? 2 : 5)}</span>
                </button>
              </Tooltip>
            ))}
            {!loading && (report?.trend.length ?? 0) === 0 && (
              <div className="creditempty">这段时间没有消费记录</div>
            )}
          </div>
        </section>

        <section className="creditsection">
          <div className="creditsectionhd">
            <span>通道统计</span>
            <span className="fs10 t3">按消耗排序</span>
          </div>
          <div className="creditchannels">
            {(report?.channels ?? []).map((ch) => (
              <div className="creditrow" key={ch.channelKey || "(default)"}>
                <div className="creditrowhd">
                  <span className="fw6">{ch.label}</span>
                  <span className="mono">{ch.spentTotal}</span>
                  <span className="t3">
                    通过率 {ch.passRate == null ? "—" : `${Math.round(ch.passRate * 100)}%`}
                  </span>
                </div>
                <Tooltip
                  content={`通过 ${ch.spentPass} · 拒绝 ${ch.spentRej} · 待验收 ${ch.spentPending} · 失败/放弃 ${ch.spentFailedAbandoned}`}
                >
                  <button
                    type="button"
                    className="creditsegments"
                    aria-label={`通过 ${ch.spentPass} · 拒绝 ${ch.spentRej} · 待验收 ${ch.spentPending} · 失败/放弃 ${ch.spentFailedAbandoned}`}
                  >
                    <Segment value={ch.spentPass} total={ch.spentTotal} tone="pass" />
                    <Segment value={ch.spentRej} total={ch.spentTotal} tone="rej" />
                    <Segment value={ch.spentPending} total={ch.spentTotal} tone="pending" />
                    <Segment value={ch.spentFailedAbandoned} total={ch.spentTotal} tone="failed" />
                  </button>
                </Tooltip>
                <div className="creditlegend">
                  <Legend tone="pass" label={`通过 ${ch.spentPass}`} />
                  <Legend tone="rej" label={`拒绝 ${ch.spentRej}`} />
                  <Legend tone="pending" label={`待验收 ${ch.spentPending}`} />
                  <Legend tone="failed" label={`失败/放弃 ${ch.spentFailedAbandoned}`} />
                </div>
              </div>
            ))}
            {!loading && (report?.channels.length ?? 0) === 0 && (
              <div className="creditempty">暂无通道消费</div>
            )}
          </div>
        </section>
      </div>
    </Modal>
  );
}

function Metric({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="creditmetric">
      <span className="fx ac gap4">
        {label}
        {title && <DescriptionHint label={`${label}说明`}>{title}</DescriptionHint>}
      </span>
      <b>{value}</b>
    </div>
  );
}

function Segment({
  value,
  total,
  tone,
}: {
  value: number;
  total: number;
  tone: "pass" | "rej" | "pending" | "failed";
}) {
  if (value <= 0 || total <= 0) return null;
  return <span data-tone={tone} style={{ width: `${(value / total) * 100}%` }} />;
}

function Legend({
  tone,
  label,
}: {
  tone: "pass" | "rej" | "pending" | "failed";
  label: string;
}) {
  return (
    <span>
      <i data-tone={tone} />
      {label}
    </span>
  );
}
