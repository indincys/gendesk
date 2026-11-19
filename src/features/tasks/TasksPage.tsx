import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { MediaActions, OriginalPreview, copyText } from "@/features/_shared/MediaActions";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type TaskInspection, type TaskView, commands, unwrap } from "@/lib/ipc";
import { errorLabel, statusVisual } from "@/lib/status";
import { cn, promptLabel } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { useUiStore } from "@/stores/ui";
import { useVirtualizer } from "@tanstack/react-virtual";
import { FolderOpen } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

const FILTERS: { key: string; label: string; match: (s: string) => boolean }[] = [
  { key: "all", label: "全部", match: () => true },
  { key: "pending", label: "待处理", match: (s) => s === "q" },
  { key: "running", label: "生成中", match: (s) => s === "run" || s === "retry" },
  { key: "failed", label: "异常", match: (s) => s === "fail" },
  { key: "review", label: "待验收", match: (s) => s === "rev" },
  { key: "done", label: "已完成", match: (s) => s === "pass" || s === "rej" },
];

/** 普通任务行定高；展开错误详情时由 virtualizer 只测量视口内的少数动态行。 */
const TASK_ROW_HEIGHT = 40;

export function TasksPage() {
  const go = useUiStore((s) => s.go);
  const openTask = useUiStore((s) => s.openTask);
  const jumpTask = useUiStore((s) => s.jumpTask);
  const jumpBatch = useUiStore((s) => s.jumpBatch);
  const openSettings = useUiStore((s) => s.openSettings);
  const tasks = useEngineStore((s) => s.tasks);
  const progress = useEngineStore((s) => s.progress);
  const paused = useEngineStore((s) => s.paused);
  const autoPauseReason = useEngineStore((s) => s.autoPauseReason);
  const setPaused = useEngineStore((s) => s.setPaused);
  const loadBatchTasks = useEngineStore((s) => s.loadBatchTasks);
  const dropTasks = useEngineStore((s) => s.dropTasks);

  const remembered =
    jumpBatch === null && jumpTask === null ? useUiStore.getState().queueLocation : null;
  const [filter, setFilter] = useState(remembered?.filter ?? "all");
  const [errorFilter, setErrorFilter] = useState<string | null>(remembered?.error ?? null);
  const [loadState, setLoadState] = useState<"loading" | "ready" | "error">("loading");
  const [deleteTarget, setDeleteTarget] = useState<TaskView | "all" | null>(null);
  const [interrupted, setInterrupted] = useState(0);
  const [intDismissed, setIntDismissed] = useState(false);
  // E04：总进度 ETA 估算所需——历史单张均值 + 有效并发。
  const [avgSec, setAvgSec] = useState<number | null>(null);
  const [concurrency, setConcurrency] = useState(0);
  // E34：改词后恢复目标 + 编辑文本。
  const [rewordTarget, setRewordTarget] = useState<TaskView | null>(null);
  const [rewordText, setRewordText] = useState("");
  // E35：展开查看原始报错的失败行。
  const [expandedErr, setExpandedErr] = useState<Set<number>>(new Set());
  // 任务5：查看单个任务的提示词快照原文。
  const [promptView, setPromptView] = useState<TaskView | null>(null);
  const [inspection, setInspection] = useState<TaskInspection | null>(null);
  const [inspectionError, setInspectionError] = useState("");
  const inspectionId = promptView?.id;
  const inspect = useCallback(async () => {
    if (inspectionId === undefined) return;
    setInspectionError("");
    try {
      setInspection(await unwrap(commands.inspectTask(inspectionId)));
    } catch (error) {
      setInspectionError(String(error));
    }
  }, [inspectionId]);
  useEffect(() => {
    setInspection(null);
    void inspect();
  }, [inspect]);
  // 任务多选（跨筛选保留选择）+ shift 范围锚点 + 批量确认框。
  const [sel, setSel] = useState<Set<number>>(new Set());
  const lastPicked = useRef<number | null>(null);
  const [confirmBulk, setConfirmBulk] = useState<null | "delete" | "cancel">(null);
  // E10：任务搜索（参考图名 / 提示词编号）。
  const [search, setSearch] = useState(
    jumpTask ? `任务 #${jumpTask}` : jumpBatch ? `#${jumpBatch}` : (remembered?.search ?? ""),
  );
  const [original, setOriginal] = useState<{ path: string; name: string } | null>(null);

  // 拉全部批次的任务。**批次不再是可切换的对象**（v0.21.0）：跑完就退出历史，
  // 而人在这一页问的从来是「现在还有哪些活」，不是「第 N 批做到哪了」。
  const refresh = useCallback(async () => {
    try {
      await loadBatchTasks(null, null);
      setLoadState("ready");
      setInterrupted(await unwrap(commands.countInterrupted()).catch(() => 0));
      setAvgSec(await unwrap(commands.estimateTaskSeconds()).catch(() => null));
      const keys = await unwrap(commands.listApiKeys()).catch(() => []);
      setConcurrency(
        keys
          .filter((k) => k.enabled && !k.circuitBroken && !k.secretMissing)
          .reduce((s, k) => s + k.concurrencyLimit, 0),
      );
    } catch (e) {
      setLoadState("error");
      toast.error(String(e));
    }
  }, [loadBatchTasks]);

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
        (!errorFilter || (t.errorType ?? "Other") === errorFilter) &&
        (!q ||
          (/^任务 #\d+$/.test(q)
            ? q === `任务 #${t.id}`
            : /^#\d+$/.test(q)
              ? q === `#${t.batchId}`
              : [
                  t.refName,
                  t.promptCode,
                  t.promptTitle,
                  t.groupName,
                  t.style,
                  `#${t.batchId}`,
                  String(t.id),
                ].some((value) => value?.toLowerCase().includes(q)))),
    );
  }, [tasks, filter, errorFilter, search]);

  // 500+ 任务若全部挂进 DOM，每次进度刷新都会让 WebView 对几百行做样式与布局。
  // 这里只保留视口附近的行；任务 id 做稳定 key，状态更新不会把测量缓存按索引串行。
  const taskScrollRef = useRef<HTMLDivElement>(null);
  const taskVirt = useVirtualizer({
    count: visible.length,
    getScrollElement: () => taskScrollRef.current,
    estimateSize: () => TASK_ROW_HEIGHT,
    getItemKey: (index) => visible[index]?.id ?? index,
    overscan: 8,
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: 重置由筛选条件触发。
  useEffect(() => {
    taskScrollRef.current?.scrollTo?.({ top: 0 });
  }, [filter, errorFilter, search]);

  const scrollRestore = useRef(remembered?.top ?? 0);
  useEffect(() => {
    if (loadState !== "ready" || !scrollRestore.current) return;
    taskScrollRef.current?.scrollTo({ top: scrollRestore.current });
    scrollRestore.current = 0;
  }, [loadState]);

  const keepQueuePosition = () =>
    useUiStore.getState().rememberQueue({
      filter,
      error: errorFilter,
      search,
      top: taskScrollRef.current?.scrollTop ?? 0,
    });

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
  const recoverableFailed = failedCount - violationCount;

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

  const recoverAllFailed = async () => {
    try {
      const res = await unwrap(commands.recoverFailedTasks());
      toast(`已恢复 ${res.affected} 个任务${res.skipped > 0 ? ` · 跳过 ${res.skipped}` : ""}`);
      await refresh();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  const deleteAllFailed = async () => {
    try {
      const n = await unwrap(commands.deleteFailedTasks());
      toast(`已删除 ${n} 个失败任务`);
      await refresh();
    } catch (error) {
      toast.error(String(error));
    }
  };

  const deleteOne = async (t: TaskView) => {
    try {
      await unwrap(commands.deleteTask(t.id));
      dropTasks([t.id]);
      await refresh();
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  // ── 多选批量操作（恢复 / 中止 / 删除） ────────────────────────────
  // 只有无输出的失败任务可恢复；待验收、通过与拒绝都已有输出，不再重新生成。
  const isRecoverable = (task: TaskView) =>
    task.status === "fail" && task.errorType !== "ContentPolicy";
  // 可中止的只有排队态：请求一旦发出去钱就花了，硬掐只会让结果无处可写。
  const ABORTABLE = new Set(["q"]);
  const clearSel = () => {
    setSel(new Set());
    lastPicked.current = null;
  };
  // 勾选：shift 从锚点范围加选，否则切换单项并设锚点（索引进 visible）。
  const pickSel = (idx: number, id: number, shift: boolean) => {
    if (shift && lastPicked.current !== null) {
      const a = Math.min(lastPicked.current, idx);
      const b = Math.max(lastPicked.current, idx);
      setSel((s) => {
        const n = new Set(s);
        for (let i = a; i <= b; i++) {
          const it = visible[i];
          if (it) n.add(it.id);
        }
        return n;
      });
    } else {
      setSel((s) => {
        const n = new Set(s);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        return n;
      });
      lastPicked.current = idx;
    }
  };
  const allVisibleSelected = visible.length > 0 && visible.every((t) => sel.has(t.id));
  const toggleAllVisible = () => {
    setSel((s) => {
      const n = new Set(s);
      if (allVisibleSelected) {
        for (const t of visible) n.delete(t.id);
      } else {
        for (const t of visible) n.add(t.id);
      }
      return n;
    });
    lastPicked.current = null;
  };
  // 选中任务里各类可操作的数量（供按钮显示与禁用）。
  const selTasks = tasks.filter((t) => sel.has(t.id));
  const selRecoverable = selTasks.filter(isRecoverable).length;
  const selAbortable = selTasks.filter((t) => ABORTABLE.has(t.status)).length;

  /**
   * 三个批量动作走同一条路：**一次 IPC 把整份 id 交给后端**，而不是前端 for 循环
   * 逐个发命令。选中 200 个任务时那是 200 次往返，中途任何一次失败都会留下一个
   * 说不清删到哪儿的中间态；后端一条 SQL 则是全有或全无。
   *
   * 跳过数一律报出来：中止放过在途、删除放过生成中，「我选了 30 个怎么只没了 22 个」
   * 如果没人解释，下一步就是再点一次。
   */
  const runBulk = async (kind: "recover" | "delete" | "cancel") => {
    const ids = [...sel];
    if (ids.length === 0) return;
    try {
      const res = await unwrap(
        kind === "recover"
          ? commands.recoverTasks(ids)
          : kind === "delete"
            ? commands.deleteTasks(ids)
            : commands.cancelTasks(ids),
      );
      const verb = kind === "recover" ? "已恢复" : kind === "delete" ? "已删除" : "已中止";
      const why =
        kind === "recover"
          ? "只有无输出的失败任务可恢复"
          : kind === "delete"
            ? "生成中不可删除"
            : "已开跑或已完成，中止不了";
      toast(
        `${verb} ${res.affected} 个任务${res.skipped > 0 ? ` · ${res.skipped} 个跳过（${why}）` : ""}`,
      );
      if (res.skipped === 0) clearSel();
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      setConfirmBulk(null);
      await refresh();
    }
  };

  const recoverInterrupted = async () => {
    try {
      const res = await unwrap(commands.recoverInterruptedTasks());
      toast(`已恢复 ${res.affected} 个中断任务${res.skipped > 0 ? ` · 跳过 ${res.skipped}` : ""}`);
      setInterrupted(0);
      await refresh();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  const recoverOne = async (t: TaskView) => {
    try {
      const res = await unwrap(commands.recoverTask(t.id, null));
      if (res.affected === 0) toast("任务状态已变化，未恢复");
      await refresh();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  // E34：改词后恢复——预填快照，确认后按编辑文本恢复失败任务。
  const openReword = (t: TaskView) => {
    setRewordTarget(t);
    setRewordText(t.promptTextSnapshot);
  };
  const submitReword = async () => {
    const t = rewordTarget;
    if (!t) return;
    const edited = rewordText.trim() !== t.promptTextSnapshot.trim() ? rewordText : null;
    try {
      const res = await unwrap(commands.recoverTask(t.id, edited));
      if (res.affected === 0) {
        toast("任务状态已变化，未恢复");
        return;
      }
      setRewordTarget(null);
      toast(edited ? "已改词并恢复" : "已恢复");
      await refresh();
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
        return { label: "检查设置", run: () => openSettings("s-keys") };
      case "ContentPolicy":
        return { label: "改词后恢复", run: () => openReword(t) };
      default:
        return { label: "恢复", run: () => void recoverOne(t) };
    }
  };

  // E04：批次总进度（终态数/总数）+ 预计剩余时间。
  const total = tasks.length;
  const terminal = counts.done + counts.fail; // pass + rej + fail
  const remaining = counts.q + counts.run; // 仍需生成的任务
  const progressPct = total > 0 ? Math.round((terminal / total) * 100) : 0;
  const showProgress = total > 0 && remaining > 0;

  return (
    <PageScaffold title="任务队列" caption="全部任务" bodyClassName="taskpage">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <div className="fx ac gap6">
          <SumBadge cls="b-gray" n={counts.q} label="待处理" />
          <SumBadge cls="b-blue" n={counts.run} label="生成中" />
          <SumBadge cls="b-red" n={counts.fail} label="异常" />
          <SumBadge cls="b-amber" n={counts.rev} label="待验收" />
          <SumBadge cls="b-green" n={counts.done} label="已完成" />
        </div>
        <div className="f1" />
        <button
          type="button"
          className="btn sm gho"
          title="打开输出根目录（验收通过的图按 批次/分组 落在里面）"
          onClick={() =>
            void unwrap(commands.openOutputsDir()).catch(() => toast.error("打开输出目录失败"))
          }
        >
          <FolderOpen className="ic12" />
          输出目录
        </button>
        {failedCount > 0 && (
          <>
            <button
              type="button"
              className="btn sm gho"
              onClick={recoverAllFailed}
              disabled={recoverableFailed === 0}
              title={violationCount > 0 ? "违规任务需要修改提示词后逐条恢复" : undefined}
            >
              恢复全部失败 · {recoverableFailed}
              {violationCount > 0 && <span className="fs10 t3">（不含违规 {violationCount}）</span>}
            </button>
            <button type="button" className="btn sm gho" onClick={() => setDeleteTarget("all")}>
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
            <button
              type="button"
              key={type}
              aria-pressed={errorFilter === type}
              onClick={() => {
                setFilter("failed");
                setErrorFilter(errorFilter === type ? null : type);
              }}
              className={cn(
                "btn xs",
                errorFilter === type && "on",
                type === "ContentPolicy" && "dng",
              )}
              title={type === "ContentPolicy" ? "违规任务需要修改提示词后逐条恢复" : undefined}
            >
              {errorLabel(type)} · {n}
            </button>
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
          <button type="button" className="btn sm" onClick={recoverInterrupted}>
            恢复中断任务
          </button>
          <button type="button" className="icb" onClick={() => setIntDismissed(true)}>
            ×
          </button>
        </div>
      )}

      <div className="fx ac gap8" style={{ padding: "10px 14px 8px" }}>
        <div className="seg">
          {FILTERS.map((f) => (
            <button
              type="button"
              key={f.key}
              className={cn("sgi", filter === f.key && "on")}
              aria-pressed={filter === f.key}
              onClick={() => {
                setFilter(f.key);
                setErrorFilter(null);
              }}
            >
              {f.label}
              <span className="fs10 mono" style={{ opacity: 0.55 }}>
                {f.key === "all" ? tasks.length : visibleCount(tasks, f)}
              </span>
            </button>
          ))}
        </div>
        <div className="f1" />
        <input
          type="search"
          className="inp"
          placeholder="搜素材、提示词、批次 #编号"
          aria-label="搜索任务"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ width: 200 }}
        />
      </div>

      {sel.size > 0 && (
        <div
          className="fx ac gap8"
          style={{ padding: "0 14px 8px", borderBottom: "1px solid var(--line)" }}
        >
          <span className="fs12 t2 nowrap">已选 {sel.size}</span>
          <button
            type="button"
            className="btn sm"
            disabled={selRecoverable === 0}
            title={selRecoverable === 0 ? "所选中没有无输出的失败任务" : undefined}
            onClick={() => void runBulk("recover")}
          >
            恢复所选{selRecoverable > 0 ? ` · ${selRecoverable}` : ""}
          </button>
          <button
            type="button"
            className="btn sm gho"
            disabled={selAbortable === 0}
            title={
              selAbortable === 0
                ? "所选中没有还在排队的任务——已经发出去的请求中止不了，钱在那一刻就花了"
                : "删掉尚未开跑的排队任务；在途的会继续跑完"
            }
            onClick={() => setConfirmBulk("cancel")}
          >
            中止所选{selAbortable > 0 ? ` · ${selAbortable}` : ""}
          </button>
          <button type="button" className="btn sm gho dng" onClick={() => setConfirmBulk("delete")}>
            删除所选
          </button>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={clearSel}>
            清除选择
          </button>
        </div>
      )}

      <div className="taskscroll" ref={taskScrollRef}>
        <div className="tgrid th">
          <span className="fx ac">
            <button
              type="button"
              className={cn("ckb", allVisibleSelected && "on")}
              title={allVisibleSelected ? "取消全选（本筛选）" : "全选（本筛选）"}
              onClick={toggleAllVisible}
            />
          </span>
          <span>状态</span>
          <span>结果 / 参考图名</span>
          <span>提示词</span>
          <span>Key</span>
          <span>进度 / 结果</span>
          <span className="tc">自动恢复</span>
          <span />
        </div>
        <div className="taskvirt" style={{ height: taskVirt.getTotalSize() }}>
          {taskVirt.getVirtualItems().map((row) => {
            const t = visible[row.index];
            if (!t) return null;
            const idx = row.index;
            const v = statusVisual(t.status);
            const pct =
              progress[t.id]?.pct ?? (t.status === "rev" || t.status === "pass" ? 100 : 0);
            return (
              <div
                ref={taskVirt.measureElement}
                data-index={row.index}
                className={cn("tgrid tr taskvrow", sel.has(t.id) && "selrow")}
                key={row.key}
                style={{ transform: `translateY(${row.start}px)` }}
              >
                <span className="fx ac">
                  <button
                    type="button"
                    className={cn("ckb", sel.has(t.id) && "on")}
                    title="选择任务（⇧ 范围多选）"
                    onClick={(e) => pickSel(idx, t.id, e.shiftKey)}
                  />
                </span>
                <span>
                  <span className={cn("bdg", v.badgeClass)}>
                    {v.spinner && <i className="spn s9" />}
                    {v.label}
                  </span>
                </span>
                <span className="fx ac gap8 ohide">
                  {t.resultImagePath ? (
                    <button
                      type="button"
                      className="ph thumb"
                      style={thumbStyle(t.resultThumbPath)}
                      aria-label={`预览任务 #${t.id} 的生成图`}
                      onClick={() =>
                        t.resultImagePath &&
                        setOriginal({ path: t.resultImagePath, name: `任务 #${t.id} · 生成图` })
                      }
                    />
                  ) : (
                    <span className="ph thumb" />
                  )}
                  <span className="mono fs11 nowrap ohide">{t.refName}</span>
                </span>
                <span className="fx ac gap7 ohide">
                  <button
                    type="button"
                    className="pid nowrap ohide"
                    title="查看提示词原文"
                    onClick={() => setPromptView(t)}
                  >
                    {promptLabel(t.promptCode, t.promptTitle)}
                  </button>
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
                              {t.errorMessage}{" "}
                              <button
                                type="button"
                                className="btn xs"
                                onClick={() =>
                                  void copyText(
                                    [
                                      `任务 #${t.id} / 批次 #${t.batchId}`,
                                      errorLabel(t.errorType),
                                      t.errorMessage,
                                    ].join("\n"),
                                    "任务错误详情",
                                  )
                                }
                              >
                                复制
                              </button>
                            </span>
                          )}
                        </span>
                      );
                    })()}
                  {t.status === "rev" && (
                    <button
                      type="button"
                      className="btn xs"
                      onClick={() => {
                        keepQueuePosition();
                        openTask("review", t.batchId, t.id);
                      }}
                    >
                      已生成 · 去验收
                    </button>
                  )}
                  {t.status === "pass" && (
                    <button
                      type="button"
                      className="btn xs"
                      onClick={() => {
                        keepQueuePosition();
                        if (t.workId !== null) useUiStore.getState().openWork(t.workId);
                        else useUiStore.getState().openBatch("library", t.batchId);
                      }}
                    >
                      已通过 · 看作品
                    </button>
                  )}
                  {t.status === "rej" && <span className="fs11 t3">未通过</span>}
                </span>
                <span className="tc mono fs11 t3">{t.retryCount || ""}</span>
                <span className="tract">
                  {t.status === "fail" && (
                    <>
                      {t.errorType === "ContentPolicy" ? null : (
                        <>
                          {t.errorType === "Auth" && (
                            <button
                              type="button"
                              className="btn sm gho"
                              onClick={() => recoverOne(t)}
                            >
                              恢复
                            </button>
                          )}
                          <button
                            type="button"
                            className="btn sm gho"
                            onClick={() => openReword(t)}
                          >
                            改词后恢复
                          </button>
                        </>
                      )}
                      <button
                        type="button"
                        className="btn sm gho"
                        onClick={() => setDeleteTarget(t)}
                      >
                        删除
                      </button>
                    </>
                  )}
                </span>
              </div>
            );
          })}
        </div>
        {(visible.length === 0 || loadState === "error") && (
          <div className="bigempty" role="status">
            <div className="fs13 fw5 t2">
              {loadState === "loading"
                ? "正在读取任务…"
                : loadState === "error"
                  ? "无法读取任务"
                  : tasks.length > 0
                    ? "当前筛选无匹配任务"
                    : "当前没有任务"}
            </div>
            {loadState === "error" ? (
              <button type="button" className="btn mt10" onClick={() => void refresh()}>
                重新读取
              </button>
            ) : (
              loadState === "ready" &&
              (tasks.length > 0 ? (
                <button
                  type="button"
                  className="btn mt10"
                  onClick={() => {
                    setFilter("all");
                    setErrorFilter(null);
                    setSearch("");
                  }}
                >
                  清除筛选
                </button>
              ) : (
                <button type="button" className="btn mt10" onClick={() => go("generate")}>
                  去生成
                </button>
              ))
            )}
          </div>
        )}
      </div>

      {rewordTarget && (
        <Modal
          title="改词后恢复"
          width="w420"
          onClose={() => setRewordTarget(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm" onClick={() => setRewordTarget(null)}>
                取消
              </button>
              <button
                type="button"
                className="btn pri sm"
                disabled={
                  rewordTarget.errorType === "ContentPolicy" &&
                  rewordText.trim() === rewordTarget.promptTextSnapshot.trim()
                }
                title={
                  rewordTarget.errorType === "ContentPolicy" &&
                  rewordText.trim() === rewordTarget.promptTextSnapshot.trim()
                    ? "违规任务必须先修改提示词"
                    : undefined
                }
                onClick={() => void submitReword()}
              >
                恢复任务
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
            提示词
          </div>
          <textarea
            className="ta"
            style={{ width: "100%", minHeight: 140, resize: "vertical" }}
            value={rewordText}
            onChange={(e) => setRewordText(e.target.value)}
            // biome-ignore lint/a11y/noAutofocus: 弹窗即为改词而生，聚焦符合预期
            autoFocus
          />
          {rewordTarget.errorType !== "ContentPolicy" && (
            <div className="fs11 t3 mt6">未修改时将按原提示词恢复。</div>
          )}
        </Modal>
      )}

      {original && (
        <OriginalPreview
          title={original.name}
          path={original.path}
          onClose={() => setOriginal(null)}
        />
      )}
      {promptView && (
        <Modal
          title={promptLabel(promptView.promptCode, promptView.promptTitle)}
          width="w420"
          onClose={() => setPromptView(null)}
          headerExtra={<span className="chip">{promptView.groupName}</span>}
          footer={
            <button
              type="button"
              className="btn sm"
              onClick={() => {
                void navigator.clipboard
                  .writeText(promptView.promptTextSnapshot)
                  .then(() => toast.success("已复制提示词"))
                  .catch((error) => toast.error(String(error)));
              }}
            >
              复制原文
            </button>
          }
        >
          <div className="fx ac gap6 wrap" style={{ marginBottom: 10 }}>
            <button
              type="button"
              className="btn xs"
              onClick={() => void copyText(String(promptView.id), "任务编号")}
            >
              任务 #{promptView.id}
            </button>
            <button
              type="button"
              className="btn xs"
              onClick={() => void copyText(String(promptView.batchId), "批次编号")}
            >
              批次 #{promptView.batchId}
            </button>
            <span className="chip">{promptView.refName}</span>
            {promptView.refImagePath && (
              <MediaActions path={promptView.refImagePath} label="源参考图" />
            )}
            {promptView.resultImagePath && (
              <MediaActions path={promptView.resultImagePath} label="生成结果" />
            )}
            {promptView.keyAlias && (
              <button type="button" className="btn xs" onClick={() => openSettings("s-keys")}>
                {promptView.keyAlias}
              </button>
            )}
            <span
              className={cn(
                "bdg",
                statusVisual((inspection?.task ?? promptView).status).badgeClass,
              )}
            >
              {statusVisual((inspection?.task ?? promptView).status).label}
            </span>
            <button type="button" className="btn xs" onClick={() => void inspect()}>
              刷新该条状态
            </button>
          </div>
          {inspectionError && (
            <p className="fs11" role="alert">
              {inspectionError}
            </p>
          )}
          <p className="fs11 t3">
            最后更新 {new Date((inspection?.task ?? promptView).updatedAt * 1000).toLocaleString()}{" "}
            · 已自动重试 {(inspection?.task ?? promptView).retryCount} 次
            {inspection?.retryReadyAt != null &&
              ` · ${new Date(inspection.retryReadyAt * 1000).toLocaleTimeString()} 后等待可用并发`}
          </p>
          {!!inspection?.outputDirectories.length && (
            <details className="mt6">
              <summary className="fs11">
                本批落盘目录 · {inspection.outputDirectories.length} 个
              </summary>
              {inspection.outputDirectories.map((directory) => (
                <div className="fx ac gap6 fs11 mt6 wrap" key={directory}>
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() =>
                      void unwrap(commands.openPathInFolder(directory)).catch((error) =>
                        toast.error(String(error)),
                      )
                    }
                  >
                    显示目录
                  </button>
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() => void copyText(directory, "目录路径")}
                  >
                    复制路径
                  </button>
                  <span>{directory}</span>
                </div>
              ))}
            </details>
          )}
          <div className="fs11 fw6 t3 mt10" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
            提示词原文（快照）
          </div>
          <div className="ptext" style={{ maxHeight: 360, overflow: "auto" }}>
            {promptView.promptTextSnapshot}
          </div>
        </Modal>
      )}

      {deleteTarget && (
        <ConfirmModal
          title={
            deleteTarget === "all"
              ? `删除全部失败任务 · ${failedCount} 条`
              : `删除任务 #${deleteTarget.id}`
          }
          desc={
            deleteTarget === "all"
              ? "删除所有批次的失败任务及其生成记录，包含当前筛选外的失败项。此操作不可撤销。"
              : `删除批次 #${deleteTarget.batchId} 内该任务及其生成记录。此操作不可撤销。`
          }
          confirmLabel="删除任务"
          danger
          onConfirm={() =>
            void (deleteTarget === "all" ? deleteAllFailed() : deleteOne(deleteTarget))
          }
          onClose={() => setDeleteTarget(null)}
        />
      )}

      {confirmBulk === "delete" && (
        <ConfirmModal
          title={`删除所选 ${sel.size} 个任务`}
          desc="将删除所选任务及其生成记录（生成中/自动恢复中的任务会被跳过）。已通过归档的作品不受影响。此操作不可撤销。"
          confirmLabel="删除所选"
          danger
          onConfirm={() => void runBulk("delete")}
          onClose={() => setConfirmBulk(null)}
        />
      )}

      {confirmBulk === "cancel" && (
        <ConfirmModal
          title={`中止所选 ${selAbortable} 个排队任务`}
          desc="只会中止尚未开跑的排队任务。已经发出请求的任务会继续完成；费用已在请求发出时产生。作品与已完成任务不变。"
          confirmLabel="中止排队任务"
          danger
          onConfirm={() => void runBulk("cancel")}
          onClose={() => setConfirmBulk(null)}
        />
      )}
    </PageScaffold>
  );
}

function visibleCount(tasks: TaskView[], f: { match: (s: string) => boolean }): number {
  return tasks.filter((t) => f.match(t.status)).length;
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
