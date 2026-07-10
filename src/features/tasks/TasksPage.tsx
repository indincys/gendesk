import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type BatchView, type TaskView, commands, unwrap } from "@/lib/ipc";
import { errorLabel, statusVisual } from "@/lib/status";
import { cn, promptLabel } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useUiStore } from "@/stores/ui";
import { ChevronDown } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

const FILTERS: { key: string; label: string; match: (s: string) => boolean }[] = [
  { key: "all", label: "全部", match: () => true },
  { key: "pending", label: "待处理", match: (s) => s === "q" },
  { key: "running", label: "生成中", match: (s) => s === "run" || s === "retry" },
  { key: "failed", label: "异常", match: (s) => s === "fail" },
  { key: "review", label: "待验收", match: (s) => s === "rev" },
  { key: "done", label: "已完成", match: (s) => s === "pass" || s === "rej" },
];

export function TasksPage() {
  const go = useUiStore((s) => s.go);
  const tasks = useEngineStore((s) => s.tasks);
  const currentBatchId = useEngineStore((s) => s.currentBatchId);
  const progress = useEngineStore((s) => s.progress);
  const paused = useEngineStore((s) => s.paused);
  const setPaused = useEngineStore((s) => s.setPaused);
  const loadBatchTasks = useEngineStore((s) => s.loadBatchTasks);

  const [batches, setBatches] = useState<BatchView[]>([]);
  const [filter, setFilter] = useState("all");
  const [showBatchPicker, setShowBatchPicker] = useState(false);
  const [interrupted, setInterrupted] = useState(0);
  const [intDismissed, setIntDismissed] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const bs = await unwrap(commands.listBatches());
      setBatches(bs);
      const target = currentBatchId ?? bs[0]?.id ?? null;
      if (target != null) await loadBatchTasks(target, null);
      setInterrupted(await unwrap(commands.countInterrupted()));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, [currentBatchId, loadBatchTasks]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 仅挂载时引导一次
  useEffect(() => {
    void refresh();
  }, []);

  const counts = useMemo(() => {
    const c = { q: 0, run: 0, fail: 0, rev: 0, done: 0 };
    for (const t of tasks) {
      if (t.status === "q") c.q++;
      else if (t.status === "run" || t.status === "retry") c.run++;
      else if (t.status === "fail") c.fail++;
      else if (t.status === "rev") c.rev++;
      else c.done++;
    }
    return c;
  }, [tasks]);

  const visible = useMemo(() => {
    const match = FILTERS.find((x) => x.key === filter)?.match ?? (() => true);
    return tasks.filter((t) => match(t.status));
  }, [tasks, filter]);

  const failedCount = counts.fail;
  const curBatch = batches.find((b) => b.id === currentBatchId);

  const switchBatch = async (id: number) => {
    setShowBatchPicker(false);
    await loadBatchTasks(id, null);
  };

  const togglePause = async () => {
    const next = !paused;
    try {
      if (paused) await unwrap(commands.resumeQueue());
      else await unwrap(commands.pauseQueue());
      // 命令不会立即回推汇总事件，乐观更新按钮态。
      setPaused(next);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  const retryAllFailed = async () => {
    if (currentBatchId == null) return;
    const n = await unwrap(commands.retryFailedTasks(currentBatchId));
    toast(`已重试 ${n} 个失败任务`);
    await loadBatchTasks(currentBatchId, null);
  };

  const deleteAllFailed = async () => {
    if (currentBatchId == null) return;
    const n = await unwrap(commands.deleteFailedTasks(currentBatchId));
    toast(`已删除 ${n} 个失败任务`);
    await loadBatchTasks(currentBatchId, null);
  };

  const deleteOne = async (t: TaskView) => {
    try {
      await unwrap(commands.deleteTask(t.id));
      if (currentBatchId != null) await loadBatchTasks(currentBatchId, null);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  const retryInterrupted = async () => {
    const n = await unwrap(commands.retryInterruptedTasks());
    toast(`已重试 ${n} 个中断任务`);
    setInterrupted(0);
    if (currentBatchId != null) await loadBatchTasks(currentBatchId, null);
  };

  const retryOne = async (t: TaskView) => {
    await unwrap(commands.retryTask(t.id, null));
    if (currentBatchId != null) await loadBatchTasks(currentBatchId, null);
  };

  return (
    <PageScaffold title="任务队列" caption="事件推送 250ms 节流 · 列表虚拟滚动">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <button type="button" className="btn sm gho" onClick={() => setShowBatchPicker(true)}>
          {curBatch ? `批次 #${curBatch.id}` : "选择批次"}
          <ChevronDown className="ic12" />
        </button>
        <div className="fx ac gap6">
          <SumBadge cls="b-gray" n={counts.q} label="待处理" />
          <SumBadge cls="b-blue" n={counts.run} label="生成中" />
          <SumBadge cls="b-red" n={counts.fail} label="异常" />
          <SumBadge cls="b-amber" n={counts.rev} label="待验收" />
          <SumBadge cls="b-green" n={counts.done} label="已完成" />
        </div>
        <div className="f1" />
        {failedCount > 0 && (
          <>
            <button type="button" className="btn sm gho" onClick={retryAllFailed}>
              重试全部失败 · {failedCount}
            </button>
            <button type="button" className="btn sm gho" onClick={deleteAllFailed}>
              删除全部失败 · {failedCount}
            </button>
          </>
        )}
        <button type="button" className="btn sm" onClick={togglePause}>
          {paused ? "继续队列" : "暂停队列"}
        </button>
      </div>

      {interrupted > 0 && !intDismissed && (
        <div className="ban">
          <span className="f1">
            {interrupted} 个任务因上次退出被中断 — 任务现场已保留，排队任务未清空，可直接继续。
          </span>
          <button type="button" className="btn sm" onClick={retryInterrupted}>
            重试中断任务
          </button>
          <button type="button" className="icb" onClick={() => setIntDismissed(true)}>
            ×
          </button>
        </div>
      )}

      <div className="fx ac gap8" style={{ padding: "10px 14px 8px" }}>
        <div className="seg">
          {FILTERS.map((f) => (
            <span
              key={f.key}
              className={cn("sgi", filter === f.key && "on")}
              onClick={() => setFilter(f.key)}
            >
              {f.label}
              <span className="fs10 mono" style={{ opacity: 0.55 }}>
                {f.key === "all" ? tasks.length : visibleCount(tasks, f)}
              </span>
            </span>
          ))}
        </div>
        <div className="f1" />
        <span className="fs11 t3 nowrap">事件推送 250ms 节流 · 列表虚拟滚动</span>
      </div>

      <div style={{ overflow: "auto", minHeight: 0 }}>
        <div className="tgrid th">
          <span>状态</span>
          <span>参考图</span>
          <span>提示词</span>
          <span>Key</span>
          <span>进度 / 结果</span>
          <span className="tc">重试</span>
          <span />
        </div>
        {visible.map((t) => {
          const v = statusVisual(t.status);
          const pct = progress[t.id]?.pct ?? (t.status === "rev" || t.status === "pass" ? 100 : 0);
          return (
            <div className="tgrid tr" key={t.id}>
              <span>
                <span className={cn("bdg", v.badgeClass)}>
                  {v.spinner && <i className="spn s9" />}
                  {v.label}
                </span>
              </span>
              <span className="fx ac gap8 ohide">
                <span className="ph thumb" style={thumbStyle(t.resultThumbPath)} />
                <span className="mono fs11 nowrap ohide">{t.refName}</span>
              </span>
              <span className="fx ac gap7 ohide">
                <span className="pid nowrap ohide">{promptLabel(t.promptCode, t.promptTitle)}</span>
                <span className="t3 fs11 nowrap ohide">{t.groupName}</span>
              </span>
              <span>{t.keyAlias && <span className="chip">{t.keyAlias}</span>}</span>
              <span className="fx ac gap8 ohide">
                {(t.status === "run" || t.status === "retry") && (
                  <>
                    <div className="pg">
                      <i style={{ width: `${pct}%` }} />
                    </div>
                    <span
                      className="mono fs10 t3 noshrink"
                      style={{ width: 30, textAlign: "right" }}
                    >
                      {pct}%
                    </span>
                  </>
                )}
                {t.status === "fail" && (
                  <span className="terr" title={t.errorMessage ?? ""}>
                    {errorLabel(t.errorType)}
                  </span>
                )}
                {t.status === "rev" && (
                  <span className="fs11 nowrap" style={{ color: "var(--ok)" }}>
                    已生成 · 待验收
                  </span>
                )}
                {t.status === "pass" && <span className="fs11 t3">已通过</span>}
                {t.status === "rej" && <span className="fs11 t3">未通过</span>}
              </span>
              <span className="tc mono fs11 t3">{t.retryCount || ""}</span>
              <span className="tract">
                {t.status === "fail" && (
                  <>
                    <button type="button" className="btn sm gho" onClick={() => retryOne(t)}>
                      重试
                    </button>
                    <button type="button" className="btn sm gho" onClick={() => deleteOne(t)}>
                      删除
                    </button>
                  </>
                )}
              </span>
            </div>
          );
        })}
        {tasks.length === 0 && (
          <div className="bigempty">
            <div className="fs13 fw5 t2">当前没有任务</div>
            <div className="fs12 t3">回到图片生成页创建新批次，或切换批次查看</div>
            <button type="button" className="btn mt10" onClick={() => go("generate")}>
              去生成
            </button>
          </div>
        )}
      </div>

      {showBatchPicker && (
        <div className="ovl" onClick={() => setShowBatchPicker(false)}>
          <div className="mdl w420" onClick={(e) => e.stopPropagation()}>
            <div className="mhead">
              <span className="fw6 fs13">切换批次</span>
              <div className="f1" />
            </div>
            <div className="mlist">
              {batches.map((b) => (
                <div key={b.id} className="pickrow" onClick={() => switchBatch(b.id)}>
                  <span className={cn("ckb", b.id === currentBatchId && "on")} />
                  <span className="fs12 f1">
                    批次 #{b.id} · {b.status === "archived" ? "已归档" : "进行中"} · {b.taskCount}{" "}
                    任务
                  </span>
                </div>
              ))}
              {batches.length === 0 && (
                <div className="fs12 t3" style={{ padding: 12 }}>
                  暂无批次
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </PageScaffold>
  );
}

function visibleCount(tasks: TaskView[], f: { match: (s: string) => boolean }): number {
  return tasks.filter((t) => f.match(t.status)).length;
}

function SumBadge({ cls, n, label }: { cls: string; n: number; label: string }) {
  return (
    <span className={cn("bdg", cls)} title={label}>
      <span className="dt" />
      {n}
    </span>
  );
}

function thumbStyle(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  if (src)
    return {
      backgroundImage: `url(${src})`,
      backgroundSize: "cover",
      backgroundPosition: "center",
    };
  return {};
}
