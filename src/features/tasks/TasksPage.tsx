import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type BatchView, type TaskView, commands, unwrap } from "@/lib/ipc";
import { errorLabel, statusVisual } from "@/lib/status";
import { cn, promptLabel } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import { ChevronDown, FolderOpen, Repeat } from "lucide-react";
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
  const autoPauseReason = useEngineStore((s) => s.autoPauseReason);
  const setPaused = useEngineStore((s) => s.setPaused);
  const loadBatchTasks = useEngineStore((s) => s.loadBatchTasks);
  const restoreFromBatch = useGenerateStore((s) => s.restoreFromBatch);

  const [batches, setBatches] = useState<BatchView[]>([]);
  const [filter, setFilter] = useState("all");
  const [showBatchPicker, setShowBatchPicker] = useState(false);
  const [interrupted, setInterrupted] = useState(0);
  const [intDismissed, setIntDismissed] = useState(false);
  // E03：取消剩余任务确认框。
  const [cancelConfirm, setCancelConfirm] = useState(false);
  // E04：批次总进度 ETA 估算所需——历史单张均值 + 有效并发。
  const [avgSec, setAvgSec] = useState<number | null>(null);
  const [concurrency, setConcurrency] = useState(0);
  // E34：改词重试目标 + 编辑文本。
  const [rewordTarget, setRewordTarget] = useState<TaskView | null>(null);
  const [rewordText, setRewordText] = useState("");
  // E35：展开查看原始报错的失败行。
  const [expandedErr, setExpandedErr] = useState<Set<number>>(new Set());
  // E10：任务搜索（参考图名 / 提示词编号）。
  const [search, setSearch] = useState("");
  // E10：批次备注行内编辑。
  const [editNoteId, setEditNoteId] = useState<number | null>(null);
  const [noteText, setNoteText] = useState("");

  const refresh = useCallback(async () => {
    try {
      const bs = await unwrap(commands.listBatches());
      setBatches(bs);
      const target = currentBatchId ?? bs[0]?.id ?? null;
      if (target != null) await loadBatchTasks(target, null);
      setInterrupted(await unwrap(commands.countInterrupted()));
      setAvgSec(await unwrap(commands.estimateTaskSeconds()).catch(() => null));
      const keys = await unwrap(commands.listApiKeys()).catch(() => []);
      setConcurrency(keys.filter((k) => k.enabled).reduce((s, k) => s + k.concurrencyLimit, 0));
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
    const q = search.trim().toLowerCase();
    return tasks.filter(
      (t) =>
        match(t.status) &&
        (q === "" || t.refName.toLowerCase().includes(q) || t.promptCode.toLowerCase().includes(q)),
    );
  }, [tasks, filter, search]);

  // E06：失败任务按错误类型分组计数。
  const failByType = useMemo(() => {
    const m = new Map<string, number>();
    for (const t of tasks) {
      if (t.status !== "fail") continue;
      const k = t.errorType ?? "Other";
      m.set(k, (m.get(k) ?? 0) + 1);
    }
    return m;
  }, [tasks]);

  const failedCount = counts.fail;
  const violationCount = failByType.get("ContentPolicy") ?? 0;
  const retryableFailed = failedCount - violationCount;
  const curBatch = batches.find((b) => b.id === currentBatchId);

  const switchBatch = async (id: number) => {
    setShowBatchPicker(false);
    await loadBatchTasks(id, null);
  };

  // E07：按此批次配置再来一批——还原挂靠与参数到生成页。
  const reuseBatch = async (id: number) => {
    try {
      const cfg = await unwrap(commands.getBatchConfig(id));
      let params: { size?: string | null; quality?: string | null } = {};
      try {
        params = JSON.parse(cfg.paramsJson);
      } catch {
        // 参数快照损坏则仅还原挂靠
      }
      restoreFromBatch(cfg.refs, params);
      setShowBatchPicker(false);
      if (cfg.refs.length === 0) toast("该批次的参考图或分组已删除，仅还原了参数");
      go("generate");
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
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

  const cancelPending = async () => {
    if (currentBatchId == null) return;
    const n = await unwrap(commands.cancelBatchPending(currentBatchId));
    toast(`已取消 ${n} 个排队任务 · 在途任务将继续跑完`);
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

  // E34：改词重试——预填快照，确认后按编辑文本回队重生。
  const openReword = (t: TaskView) => {
    setRewordTarget(t);
    setRewordText(t.promptTextSnapshot);
  };
  const submitReword = async () => {
    const t = rewordTarget;
    if (!t) return;
    const edited = rewordText.trim() !== t.promptTextSnapshot.trim() ? rewordText : null;
    try {
      await unwrap(commands.retryTask(t.id, edited));
      setRewordTarget(null);
      toast(edited ? "已按改后的提示词重新生成" : "已重新生成");
      if (currentBatchId != null) await loadBatchTasks(currentBatchId, null);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  // E10：批次备注提交。
  const saveNote = async (id: number) => {
    setEditNoteId(null);
    try {
      await unwrap(commands.renameBatch(id, noteText));
      setBatches(await unwrap(commands.listBatches()));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  const toggleErr = (id: number) =>
    setExpandedErr((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  // E35：按错误类型给建议动作（可点链接）。
  const errAction = (t: TaskView): { label: string; run: () => void } => {
    switch (t.errorType) {
      case "Auth":
        return { label: "检查设置", run: () => go("settings") };
      case "ContentPolicy":
        return { label: "改词重试", run: () => openReword(t) };
      default:
        return { label: "重试", run: () => void retryOne(t) };
    }
  };

  // E04：批次总进度（终态数/总数）+ 预计剩余时间。
  const total = tasks.length;
  const terminal = counts.done + counts.fail; // pass + rej + fail
  const remaining = counts.q + counts.run; // 仍需生成的任务
  const progressPct = total > 0 ? Math.round((terminal / total) * 100) : 0;
  const showProgress = total > 0 && remaining > 0;

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
        {counts.q > 0 && (
          <button type="button" className="btn sm gho dng" onClick={() => setCancelConfirm(true)}>
            取消剩余任务 · {counts.q}
          </button>
        )}
        {failedCount > 0 && (
          <>
            <button
              type="button"
              className="btn sm gho"
              onClick={retryAllFailed}
              disabled={retryableFailed === 0}
              title={
                violationCount > 0 ? "违规任务不会被批量重试，请对其单独「改词重试」" : undefined
              }
            >
              重试全部失败 · {retryableFailed}
              {violationCount > 0 && <span className="fs10 t3">（不含违规 {violationCount}）</span>}
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

      {showProgress && (
        <div className="fx ac gap10" style={{ padding: "8px 14px 0" }}>
          <div className="pg f1" style={{ height: 8 }}>
            <i style={{ width: `${progressPct}%` }} />
          </div>
          <span className="fs11 t3 mono nowrap">
            {terminal}/{total} · {progressPct}%
          </span>
          <span className="fs11 t3 nowrap">
            预计剩余 {tasksEta(avgSec, remaining, concurrency)}
          </span>
        </div>
      )}

      {failedCount > 0 && (
        <div className="fx ac gap6 wrap" style={{ padding: "8px 14px 0" }}>
          <span className="fs11 t3 nowrap">失败分类</span>
          {[...failByType.entries()].map(([type, n]) => (
            <span
              key={type}
              className={cn("chip", type === "ContentPolicy" && "dng")}
              title={
                type === "ContentPolicy" ? "违规任务请「改词重试」，不参与批量重试" : undefined
              }
            >
              {errorLabel(type)} · {n}
            </span>
          ))}
        </div>
      )}

      {autoPauseReason && paused && (
        <div className="ban dng">
          <span className="f1">已自动暂停：{autoPauseReason}。请检查 API Key 与网络后再继续。</span>
          <button type="button" className="btn sm gho" onClick={() => go("settings")}>
            检查设置
          </button>
          <button type="button" className="btn sm" onClick={togglePause}>
            继续队列
          </button>
        </div>
      )}

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
        <input
          type="search"
          className="inp"
          placeholder="搜参考图名 / 提示词编号"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ width: 200 }}
        />
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
                {t.status === "fail" &&
                  (() => {
                    const act = errAction(t);
                    return (
                      <span className="col gap2 ohide" style={{ minWidth: 0 }}>
                        <span className="fx ac gap6 ohide">
                          <span className="terr">{errorLabel(t.errorType)}</span>
                          <button
                            type="button"
                            className="fs11 nowrap"
                            style={{ color: "var(--acc2)", textDecoration: "underline" }}
                            onClick={act.run}
                          >
                            {act.label}
                          </button>
                          {t.errorMessage && (
                            <button
                              type="button"
                              className="fs10 t3 nowrap"
                              onClick={() => toggleErr(t.id)}
                            >
                              {expandedErr.has(t.id) ? "收起" : "详情"}
                            </button>
                          )}
                        </span>
                        {expandedErr.has(t.id) && t.errorMessage && (
                          <span className="fs10 t3" style={{ wordBreak: "break-all" }}>
                            {t.errorMessage}
                          </span>
                        )}
                      </span>
                    );
                  })()}
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
                    {t.errorType === "ContentPolicy" ? (
                      <button type="button" className="btn pri sm" onClick={() => openReword(t)}>
                        改词重试
                      </button>
                    ) : (
                      <>
                        <button type="button" className="btn sm gho" onClick={() => retryOne(t)}>
                          重试
                        </button>
                        <button type="button" className="btn sm gho" onClick={() => openReword(t)}>
                          改词重试
                        </button>
                      </>
                    )}
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

      {rewordTarget && (
        <Modal
          title="改词重试"
          width="w420"
          onClose={() => setRewordTarget(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm" onClick={() => setRewordTarget(null)}>
                取消
              </button>
              <button type="button" className="btn pri sm" onClick={() => void submitReword()}>
                重新生成
              </button>
            </>
          }
        >
          <div className="fx ac gap6 wrap" style={{ marginBottom: 10 }}>
            <span className="pid">{rewordTarget.promptCode}</span>
            <span className="chip">{rewordTarget.refName}</span>
            {rewordTarget.errorType === "ContentPolicy" && (
              <span className="bdg b-red">违规 · 建议改词后再试</span>
            )}
          </div>
          <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
            提示词（可修改后重试）
          </div>
          <textarea
            className="ta"
            style={{ width: "100%", minHeight: 140, resize: "vertical" }}
            value={rewordText}
            onChange={(e) => setRewordText(e.target.value)}
            // biome-ignore lint/a11y/noAutofocus: 弹窗即为改词而生，聚焦符合预期
            autoFocus
          />
          <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
            确认后该任务按改后的提示词回队重新出图；未改动则按原文重试。通过验收后改后版本会写回提示词库。
          </div>
        </Modal>
      )}

      {cancelConfirm && (
        <ConfirmModal
          title="取消剩余任务"
          desc={`将删除本批次 ${counts.q} 个尚未开始的排队任务。正在生成中的任务会继续跑完，不受影响；作品与已完成任务不变。此操作不可撤销。`}
          confirmLabel="取消剩余任务"
          danger
          onConfirm={() => void cancelPending()}
          onClose={() => setCancelConfirm(false)}
        />
      )}

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
                  <span className="ph thumb" style={thumbStyle(b.firstThumbPath)} />
                  <div className="col f1" style={{ minWidth: 0, gap: 2 }}>
                    {editNoteId === b.id ? (
                      <input
                        type="text"
                        className="inp"
                        value={noteText}
                        placeholder={`批次 #${b.id} 备注名`}
                        onClick={(e) => e.stopPropagation()}
                        onChange={(e) => setNoteText(e.target.value)}
                        onBlur={() => void saveNote(b.id)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void saveNote(b.id);
                          if (e.key === "Escape") setEditNoteId(null);
                        }}
                        // biome-ignore lint/a11y/noAutofocus: 行内编辑即时聚焦符合预期
                        autoFocus
                      />
                    ) : (
                      <span className="fs12 nowrap ohide fw5">
                        {b.note || `批次 #${b.id}`}
                        <button
                          type="button"
                          className="fs10 t3"
                          style={{ marginLeft: 6 }}
                          title="重命名批次"
                          onClick={(e) => {
                            e.stopPropagation();
                            setNoteText(b.note ?? "");
                            setEditNoteId(b.id);
                          }}
                        >
                          ✎
                        </button>
                      </span>
                    )}
                    <span className="fs10 t3 nowrap">
                      {fmtBatchTime(b.createdAt)} · {b.status === "archived" ? "已归档" : "进行中"}{" "}
                      · {b.taskCount} 任务 · 请求 {b.requestCount} 次 · {paramsLabel(b.paramsJson)}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="btn sm gho"
                    title="打开该批次的输出文件夹"
                    onClick={(e) => {
                      e.stopPropagation();
                      void unwrap(commands.openBatchOutputDir(b.id)).catch(() =>
                        toast.error("打开输出文件夹失败"),
                      );
                    }}
                  >
                    <FolderOpen className="ic12" />
                    输出
                  </button>
                  <button
                    type="button"
                    className="btn sm gho"
                    title="按此批次的参考图挂靠与参数还原到生成页"
                    onClick={(e) => {
                      e.stopPropagation();
                      void reuseBatch(b.id);
                    }}
                  >
                    <Repeat className="ic12" />
                    再来一批
                  </button>
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

/** E10 批次时间：unix 秒 → 本地 MM-DD HH:mm。 */
function fmtBatchTime(unix: number): string {
  const d = new Date(unix * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** E04 剩余耗时：历史单张均值 × 剩余数 ÷ 有效并发；无历史或无并发则不估算。 */
function tasksEta(avgSec: number | null, remaining: number, concurrency: number): string {
  if (avgSec == null || concurrency === 0 || remaining === 0) return "—";
  const seconds = Math.round((avgSec * remaining) / concurrency);
  if (seconds < 60) return `约 ${seconds} 秒`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return s > 0 ? `约 ${m} 分 ${s} 秒` : `约 ${m} 分`;
}

/** 批次生效参数摘要（E16）：无显式参数则「跟随提示词」。 */
function paramsLabel(json: string): string {
  try {
    const p = JSON.parse(json) as { size?: string; quality?: string };
    const parts: string[] = [];
    if (p.size) parts.push(p.size);
    if (p.quality) parts.push(`质量 ${p.quality}`);
    return parts.length > 0 ? parts.join(" · ") : "跟随提示词";
  } catch {
    return "跟随提示词";
  }
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
