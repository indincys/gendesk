import { Modal } from "@/components/ui/Modal";
import { DescriptionHint, Tooltip } from "@/components/ui/Tooltip";
import { copyText } from "@/features/_shared/MediaActions";
import {
  type CreditEntry,
  type CreditRange,
  type V2vCreditReport,
  commands,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { useCallback, useEffect, useRef, useState } from "react";

const OUTCOMES = {
  pass: "通过",
  rej: "拒绝",
  pending: "待验收",
  failed: "失败或放弃",
  abandoned: "放弃",
} as const;
type Outcome = keyof typeof OUTCOMES;
type DetailFilter = {
  bucket: string | null;
  channel: string | null;
  outcome: Outcome | null;
  title: string;
};

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

  const [checkedAt, setCheckedAt] = useState<number | null>(null);
  const [detail, setDetail] = useState<DetailFilter | null>(null);
  const [entries, setEntries] = useState<CreditEntry[] | null>(null);
  const [entriesError, setEntriesError] = useState("");
  const [page, setPage] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const reportSeq = useRef(0);

  const selectDetail = (filter: DetailFilter) => {
    setDetail(filter);
    setPage(0);
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt explicitly retries the same range.
  useEffect(() => {
    if (!detail) return;
    let active = true;
    setEntries(null);
    setEntriesError("");
    void unwrap(
      commands.v2vCreditEntries(range, detail.bucket, detail.channel, detail.outcome, page * 100),
    )
      .then((value) => {
        if (active) setEntries(value);
      })
      .catch((error) => {
        if (active) setEntriesError(String(error));
      });
    return () => {
      active = false;
    };
  }, [range, detail, page, attempt]);

  const load = useCallback(async (next: CreditRange) => {
    const seq = ++reportSeq.current;
    setLoading(true);
    setError(null);
    try {
      const value = await unwrap(commands.v2vCreditReport(next));
      if (seq === reportSeq.current) {
        setReport(value);
        setCheckedAt(Date.now());
      }
    } catch (e) {
      if (seq === reportSeq.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (seq === reportSeq.current) setLoading(false);
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
              onClick={() => {
                if (r.key !== range) {
                  setRange(r.key);
                  setReport(null);
                  setDetail(null);
                }
              }}
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

        <div className="fx ac gap8 fs11 t3">
          {checkedAt && `上次查询 ${new Date(checkedAt).toLocaleString()}`}
          {report?.balanceError && (
            <span className="wr2">余额暂不可用 · {report.balanceError}</span>
          )}
          <button
            type="button"
            className="btn xs"
            disabled={loading}
            onClick={() => void load(range)}
          >
            重新查询余额
          </button>
        </div>

        {detail && (
          <section className="creditsection" aria-label="消费明细">
            <div className="creditsectionhd">
              <strong>消费明细 · {detail.title}</strong>
              <button type="button" className="btn xs" onClick={() => setDetail(null)}>
                收起明细
              </button>
            </div>
            {entriesError ? (
              <p className="fs12">
                {entriesError}{" "}
                <button type="button" className="btn xs" onClick={() => setAttempt((n) => n + 1)}>
                  重试
                </button>
              </p>
            ) : entries === null ? (
              <p className="fs12 t3" role="status">
                正在读取明细…
              </p>
            ) : entries.length === 0 ? (
              <p className="fs12 t3">当前范围没有明细</p>
            ) : (
              <div style={{ maxHeight: "25vh", overflow: "auto" }}>
                {entries.slice(0, 100).map((item) => (
                  <div className="fx ac gap6 fs11 mt6 wrap" key={item.submitId}>
                    <span>
                      {new Date(item.chargedAt * 1000).toLocaleString()} · {item.credits} 额度 ·{" "}
                      {OUTCOMES[item.outcome as Outcome] ?? item.outcome}
                    </span>
                    <button
                      type="button"
                      className="btn xs"
                      onClick={() => void copyText(item.submitId, "提交编号")}
                    >
                      复制提交编号
                    </button>
                    {item.clipId === null ? (
                      <span className="t3">条目已移除</span>
                    ) : (
                      <button
                        type="button"
                        className="btn xs"
                        onClick={() => {
                          if (item.clipId === null) return;
                          onClose();
                          useUiStore
                            .getState()
                            .openClip(
                              item.clipStage === "pass" || item.clipStage === "rej"
                                ? "clips"
                                : "v2v",
                              item.clipId,
                            );
                        }}
                      >
                        定位 {item.clipCode || `#${item.clipId}`}
                      </button>
                    )}
                  </div>
                ))}
              </div>
            )}
            <div className="fx ac gap8 mt6">
              <button
                type="button"
                className="btn xs"
                disabled={page === 0}
                onClick={() => setPage((n) => n - 1)}
              >
                上一页
              </button>
              <button
                type="button"
                className="btn xs"
                disabled={(entries?.length ?? 0) <= 100}
                onClick={() => setPage((n) => n + 1)}
              >
                下一页
              </button>
            </div>
          </section>
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
                  aria-label={`${point.bucket} · ${point.spent} 额度 · 查看明细`}
                  aria-pressed={detail?.bucket === point.bucket}
                  onClick={() =>
                    selectDetail({
                      bucket: point.bucket,
                      channel: null,
                      outcome: null,
                      title: point.bucket,
                    })
                  }
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
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() =>
                      selectDetail({
                        bucket: null,
                        channel: ch.channelKey,
                        outcome: null,
                        title: ch.label,
                      })
                    }
                  >
                    {ch.label}
                  </button>
                  <span className="mono">{ch.spentTotal}</span>
                  <span className="t3">
                    通过率 {ch.passRate == null ? "—" : `${Math.round(ch.passRate * 100)}%`}
                  </span>
                </div>
                <div className="creditsegments" aria-label={`${ch.label}消费结果`}>
                  {(
                    [
                      ["pass", ch.spentPass],
                      ["rej", ch.spentRej],
                      ["pending", ch.spentPending],
                      ["failed", ch.spentFailedAbandoned],
                    ] as const
                  ).map(([tone, value]) => (
                    <Segment
                      key={tone}
                      value={value}
                      total={ch.spentTotal}
                      tone={tone}
                      onClick={() =>
                        selectDetail({
                          bucket: null,
                          channel: ch.channelKey,
                          outcome: tone,
                          title: `${ch.label} · ${OUTCOMES[tone]}`,
                        })
                      }
                    />
                  ))}
                </div>
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
  onClick,
}: {
  value: number;
  total: number;
  tone: "pass" | "rej" | "pending" | "failed";
  onClick: () => void;
}) {
  if (value <= 0 || total <= 0) return null;
  return (
    <button
      type="button"
      data-tone={tone}
      style={{ width: `${(value / total) * 100}%` }}
      aria-label={`${OUTCOMES[tone]} ${value} 额度 · 查看明细`}
      title={`${OUTCOMES[tone]} ${value} 额度`}
      onClick={onClick}
    />
  );
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
