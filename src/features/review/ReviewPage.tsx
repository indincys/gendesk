import { Modal } from "@/components/ui/Modal";
import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { moveByRow, packJustifiedRows } from "@/features/review/layout";
import { assetSrc, bg } from "@/lib/img";
import { type ReviewItemView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Check, Clock, Maximize2, RotateCcw, X } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/** 一张图的宽高比。缺尺寸时用 1（历史任务后端会补齐，这里只兜住补不到的极端情况）。 */
function ratioOf(it: ReviewItemView): number {
  const w = it.resultWidth ?? 0;
  const h = it.resultHeight ?? 0;
  return w > 0 && h > 0 ? w / h : 1;
}

// T1：单卡抽为 React.memo 组件——按键/选中/待定变化只重渲变化的 1–2 张，
// 而非整个网格。所有回调由父级 useCallback 稳定化，保证 memo 生效。
type ReviewCardProps = {
  item: ReviewItemView;
  idx: number;
  /** 齐行分配给这张图的宽度（px）。行高由父级统一给，二者构成它的真实比例。 */
  width: number;
  selected: boolean;
  focused: boolean;
  pending: boolean;
  onCardClick: (idx: number, shift: boolean) => void;
  onAccept: (id: number) => void;
  onReject: (id: number) => void;
  onRetry: (item: ReviewItemView) => void;
  onTogglePending: (id: number) => void;
  onZoom: (idx: number) => void;
  onHover: (idx: number) => void;
};

const ReviewCard = memo(function ReviewCard({
  item,
  idx,
  width,
  selected,
  focused,
  pending,
  onCardClick,
  onAccept,
  onReject,
  onRetry,
  onTogglePending,
  onZoom,
  onHover,
}: ReviewCardProps) {
  return (
    <div
      className={cn("rcard rjcard", selected && "sel", focused && "focus", pending && "pend")}
      style={{ width }}
      onClick={(e) => onCardClick(idx, e.shiftKey)}
      onDoubleClick={() => onZoom(idx)}
      // T2：指针跟随焦点——悬停即设焦点，令空格/回车作用于鼠标所指卡片而非默认第 0 张。
      onMouseEnter={() => onHover(idx)}
    >
      {/* 图框铺满这一格：格子本身已按真实宽高比算好，故 cover 不会裁掉任何东西。 */}
      <NatThumb path={item.resultThumbPath} className="rcimg rjimg" />
      <span className={cn("rck", selected && "on")}>
        <Check className="ic12" />
      </span>
      {pending && <span className="rpend">待定</span>}
      <div className="hacts">
        <button
          type="button"
          className="hbtn"
          title="通过"
          onClick={(e) => {
            e.stopPropagation();
            onAccept(item.id);
          }}
        >
          <Check className="ic12" />
        </button>
        <button
          type="button"
          className="hbtn"
          title="不通过"
          onClick={(e) => {
            e.stopPropagation();
            onReject(item.id);
          }}
        >
          <X className="ic12" />
        </button>
        <button
          type="button"
          className="hbtn"
          title="重试（可微调提示词）"
          onClick={(e) => {
            e.stopPropagation();
            (e.currentTarget as HTMLElement).blur();
            onRetry(item);
          }}
        >
          <RotateCcw className="ic12" />
        </button>
        <button
          type="button"
          className={cn("hbtn", pending && "on")}
          title="标记待定（稍后再定）"
          onClick={(e) => {
            e.stopPropagation();
            // T2：点击后失焦，键盘焦点回网格，消除「再按空格/回车双触发」。
            (e.currentTarget as HTMLElement).blur();
            onTogglePending(item.id);
          }}
        >
          <Clock className="ic12" />
        </button>
        <button
          type="button"
          className="hbtn"
          title="大图逐张"
          onClick={(e) => {
            e.stopPropagation();
            (e.currentTarget as HTMLElement).blur();
            onZoom(idx);
          }}
        >
          <Maximize2 className="ic12" />
        </button>
      </div>
      <div className="rmeta">
        <span className="pid">{item.promptCode}</span>
        <span className="fs10 t3 mono nowrap ohide">{item.refName}</span>
      </div>
    </div>
  );
});

export function ReviewPage() {
  const [items, setItems] = useState<ReviewItemView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cols, setCols] = useState(5);
  // E09：网格键盘焦点（索引进 displayed）。
  const [focus, setFocus] = useState(0);
  // E38：待定标记（纯 UI 态，不入库）——沉底 + 角标 + 可筛选。
  const [pending, setPending] = useState<Set<number>>(new Set());
  const [onlyPending, setOnlyPending] = useState(false);
  // E24：排序模式——时间序 / 按参考图聚类 / 按提示词组聚类。
  // 任务7：默认按提示词组分组显示（分组头醒目、便于成组验收）。
  // 默认按批次聚类：最近一批在最顶部，往下依次是更早的批次。
  const [sortMode, setSortMode] = useState<"batch" | "time" | "ref" | "group" | "key">("batch");
  // E38：shift 范围多选锚点（索引进 displayed）。
  const lastClicked = useRef<number | null>(null);
  // E08：大图参考图对比——持久切换 compareRef，或按住空格临时 peek。
  const [compareRef, setCompareRef] = useState(false);
  const [holdRef, setHoldRef] = useState(false);
  // 齐行排版所需的容器宽度。**只随窗口/侧栏变化**，与图片加载无关——
  // 用图片自身尺寸去反推布局才会抖，用容器宽度不会。
  const [measureW, setMeasureW] = useState(1100);
  const [zoom, setZoom] = useState<number | null>(null); // index into items
  const [processed, setProcessed] = useState(0);
  // 「重试 + 微调提示词」目标（E01）：打开编辑框，确认后微调写快照并回队。
  const [retryTarget, setRetryTarget] = useState<ReviewItemView | null>(null);
  const [retryText, setRetryText] = useState("");
  // 正在处理中的任务 id（防长按 ⏎ / 连点重复提交同一任务，后端另有幂等守卫兜底）。
  const inFlight = useRef<Set<number>>(new Set());
  // T1：虚拟化滚动容器（`.pbody` 承载滚动）。
  const parentRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    try {
      setItems(await unwrap(commands.listPendingReview(null)));
      setSel(new Set());
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  // 容器宽度：齐行的行高由它推出，故必须在渲染前就是对的。
  // 依赖 items.length 是因为滚动容器只在有待验收项时才挂载（空态是另一棵子树）。
  const hasItems = items.length > 0;
  // biome-ignore lint/correctness/useExhaustiveDependencies: 容器挂载/卸载随 hasItems 切换，须重挂观察器
  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => {
      const w = e?.contentRect.width;
      if (w) setMeasureW((cur) => (Math.abs(w - cur) > 1 ? w : cur));
    });
    ro.observe(el);
    setMeasureW(el.clientWidth);
    return () => ro.disconnect();
  }, [hasItems]);

  const removeIds = (ids: number[]) => {
    setItems((cur) => cur.filter((i) => !ids.includes(i.id)));
    setSel(new Set());
    setProcessed((p) => p + ids.length);
  };

  const accept = useCallback(async (ids: number[]) => {
    const fresh = ids.filter((id) => !inFlight.current.has(id));
    if (fresh.length === 0) return;
    for (const id of fresh) inFlight.current.add(id);
    try {
      const res = await unwrap(commands.acceptTasks(fresh));
      removeIds(fresh);
      for (const g of res.promotedGroups) toast(`「${g}」已自动写入提示词库`);
      toast.success(`已通过 ${res.accepted} 张`);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      for (const id of fresh) inFlight.current.delete(id);
    }
  }, []);

  const reject = useCallback(async (ids: number[]) => {
    const fresh = ids.filter((id) => !inFlight.current.has(id));
    if (fresh.length === 0) return;
    for (const id of fresh) inFlight.current.add(id);
    try {
      const n = await unwrap(commands.rejectTasks(fresh));
      removeIds(fresh);
      toast(`${n} 张移入废纸篓`);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      for (const id of fresh) inFlight.current.delete(id);
    }
  }, []);

  const openRetry = useCallback((it: ReviewItemView) => {
    setRetryTarget(it);
    setRetryText(it.promptText);
  }, []);

  const submitRetry = useCallback(async () => {
    const it = retryTarget;
    if (!it) return;
    if (inFlight.current.has(it.id)) return;
    inFlight.current.add(it.id);
    // 仅在提示词实际改动时传微调文本（否则传 null，避免无谓写快照）。
    const edited = retryText.trim() !== it.promptText.trim() ? retryText : null;
    try {
      await unwrap(commands.retryTask(it.id, edited));
      removeIds([it.id]);
      setRetryTarget(null);
      toast(edited ? "已按微调提示词重新生成" : "已重新生成");
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      inFlight.current.delete(it.id);
    }
  }, [retryTarget, retryText]);

  const toggleSel = useCallback(
    (id: number) =>
      setSel((s) => {
        const n = new Set(s);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        return n;
      }),
    [],
  );

  const togglePending = useCallback(
    (id: number) =>
      setPending((s) => {
        const n = new Set(s);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        return n;
      }),
    [],
  );

  // E38/E24：显示序——「仅看待定」筛选；按排序模式聚类；时间序下待定项稳定沉底。
  const displayed = useMemo(() => {
    const arr = onlyPending ? items.filter((i) => pending.has(i.id)) : [...items];
    if (sortMode === "batch") {
      // 批次倒序（新批在上），批次内保持生成序。后端已是此序，这里显式排一遍，
      // 使「仅看待定」筛掉部分项后顺序依然确定。
      arr.sort((a, b) => b.batchId - a.batchId || a.id - b.id);
    } else if (sortMode === "ref") {
      arr.sort((a, b) => a.refName.localeCompare(b.refName) || a.id - b.id);
    } else if (sortMode === "group") {
      arr.sort((a, b) => a.groupName.localeCompare(b.groupName) || a.id - b.id);
    } else if (sortMode === "key") {
      // "~" 使无 Key（keyAlias 为空）项排到末尾。
      arr.sort((a, b) => (a.keyAlias ?? "~").localeCompare(b.keyAlias ?? "~") || a.id - b.id);
    } else if (!onlyPending) {
      // 时间序（后端已按批次倒序 + 组内 id 升序）：仅让待定项稳定沉底。
      arr.sort((a, b) => Number(pending.has(a.id)) - Number(pending.has(b.id)));
    }
    return arr;
  }, [items, pending, onlyPending, sortMode]);

  // E24：聚类模式下某项所属的分段键（时间序无分段）。
  const clusterKey = useCallback(
    (it: ReviewItemView): string | null =>
      sortMode === "batch"
        ? `#${it.batchId}`
        : sortMode === "ref"
          ? it.refName
          : sortMode === "group"
            ? it.groupName
            : sortMode === "key"
              ? (it.keyAlias ?? "未标注 Key")
              : null,
    [sortMode],
  );

  // 任务7：每个分组的待验收张数（分组头右侧显示）。
  const clusterCounts = useMemo(() => {
    const m = new Map<string, number>();
    if (sortMode === "time") return m;
    for (const it of displayed) {
      const k = clusterKey(it);
      if (k !== null) m.set(k, (m.get(k) ?? 0) + 1);
    }
    return m;
  }, [displayed, sortMode, clusterKey]);

  // 齐行打包走抽出来的纯函数（`layout.ts`，另有测试）：算的是「每张占多宽、每行多高」，
  // 而验收判的恰恰是构图与边缘，排错一格等于给人看了一张裁过的图。
  // 容器左右各 14px 内边距，与 .rvscroll 一致。
  // biome-ignore lint/correctness/useExhaustiveDependencies: measureW 决定行高，必须进依赖
  const { rows, cardRow } = useMemo(
    () =>
      packJustifiedRows(displayed, {
        width: Math.max(240, measureW - 28),
        perRow: cols,
        ratioOf,
        clusterKey,
        counts: clusterCounts,
      }),
    [displayed, cols, clusterKey, clusterCounts, measureW],
  );

  // 行高**精确可算**（上面已经算好），故 estimateSize 就是真值：
  // 虚拟化不必再回头测量任何一行，滚动全程零重排。
  const virt = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (i) => {
      const r = rows[i];
      if (!r) return 220;
      return r.kind === "header" ? 56 : r.h + 34; // +34 = 卡片下方编号行 + 行间距
    },
    overscan: 6,
  });

  // 大图逐张模式键盘
  useEffect(() => {
    if (zoom === null) return;
    const onKey = (e: KeyboardEvent) => {
      // 重试编辑框打开时让位给输入，Modal 自行处理 Esc。
      if (retryTarget) return;
      if (e.key === "Escape") return setZoom(null);
      if (e.key === "ArrowLeft") setZoom((z) => (z === null ? null : Math.max(0, z - 1)));
      else if (e.key === "ArrowRight")
        setZoom((z) => (z === null ? null : Math.min(displayed.length - 1, z + 1)));
      else if (e.key === "Enter") {
        const it = displayed[zoom];
        if (it) void accept([it.id]);
      } else if (e.key === "Backspace") {
        const it = displayed[zoom];
        if (it) void reject([it.id]);
      } else if (e.key === "r" || e.key === "R") {
        const it = displayed[zoom];
        if (it) openRetry(it);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [zoom, displayed, accept, reject, retryTarget, openRetry]);

  // 大图模式下列表变化后修正索引
  useEffect(() => {
    if (zoom !== null && zoom >= displayed.length)
      setZoom(displayed.length > 0 ? displayed.length - 1 : null);
  }, [displayed, zoom]);

  // E08：切换到另一张图时复位参考图对比态。
  useEffect(() => {
    setCompareRef(false);
    setHoldRef(false);
  }, [zoom]);

  // E08：大图模式按住空格临时 peek 参考图，松开回到生成图。
  useEffect(() => {
    if (zoom === null || retryTarget) return;
    const down = (e: KeyboardEvent) => {
      if (e.key === " ") {
        e.preventDefault();
        setHoldRef(true);
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.key === " ") setHoldRef(false);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [zoom, retryTarget]);

  // E09：网格模式键盘流（大图/重试框打开时让位）。
  useEffect(() => {
    if (zoom !== null || retryTarget) return;
    const onKey = (e: KeyboardEvent) => {
      // T2：焦点落在任何交互控件（悬浮按钮 / 每行滑块 / 输入）上时让位给原生行为，
      // 避免 window 处理器与控件双触发、或方向键被滑块吞掉导致网格导航失灵。
      const el = e.target as HTMLElement | null;
      if (el?.closest("button, input, textarea, select, [contenteditable=true]")) return;
      const n = displayed.length;
      if (n === 0) return;
      if ((e.metaKey || e.ctrlKey) && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        setSel(new Set(displayed.map((i) => i.id)));
        return;
      }
      if (e.key === "ArrowRight") {
        e.preventDefault();
        setFocus((f) => Math.min(n - 1, f + 1));
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        setFocus((f) => Math.max(0, f - 1));
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        // 齐行下每行张数不固定，故上下移动要走行模型而不是「加减 cols」。
        // 保持列位：落到目标行里同样的第几张（不够就取最后一张）。
        setFocus((f) => moveByRow(rows, cardRow, f, e.key === "ArrowDown" ? 1 : -1));
      } else if (e.key === " ") {
        e.preventDefault();
        const it = displayed[focus];
        if (it) toggleSel(it.id);
      } else if (e.key === "Enter") {
        const ids = sel.size > 0 ? [...sel] : displayed[focus] ? [displayed[focus].id] : [];
        if (ids.length) void accept(ids);
      } else if (e.key === "Backspace") {
        const it = displayed[focus];
        if (it) void reject([it.id]);
      } else if (e.key === "z" || e.key === "Z") {
        if (displayed[focus]) setZoom(focus);
      } else if (e.key === "s" || e.key === "S") {
        const it = displayed[focus];
        if (it) togglePending(it.id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    zoom,
    retryTarget,
    displayed,
    focus,
    rows,
    cardRow,
    sel,
    accept,
    reject,
    toggleSel,
    togglePending,
  ]);

  // E09：焦点越界修正 + 滚动进视野（T1：虚拟化 scrollToIndex 定位所在行）。
  useEffect(() => {
    if (focus >= displayed.length) setFocus(Math.max(0, displayed.length - 1));
  }, [displayed, focus]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: virt 实例稳定，仅在焦点/行模型变化时滚动
  useEffect(() => {
    const row = cardRow[focus];
    if (row !== undefined) virt.scrollToIndex(row, { align: "auto" });
  }, [focus, cardRow]);

  // E38：网格点选——shift 从锚点范围加选，否则切换单项并设锚点。
  const onCardClick = useCallback(
    (idx: number, shift: boolean) => {
      setFocus(idx);
      if (shift && lastClicked.current !== null) {
        const a = Math.min(lastClicked.current, idx);
        const b = Math.max(lastClicked.current, idx);
        setSel((s) => {
          const n = new Set(s);
          for (let i = a; i <= b; i++) {
            const it = displayed[i];
            if (it) n.add(it.id);
          }
          return n;
        });
      } else {
        const it = displayed[idx];
        if (it) toggleSel(it.id);
        lastClicked.current = idx;
      }
    },
    [displayed, toggleSel],
  );

  // T1：卡片稳定回调（id/idx 基），保证 ReviewCard memo 生效。
  const onAccept = useCallback((id: number) => void accept([id]), [accept]);
  const onReject = useCallback((id: number) => void reject([id]), [reject]);
  const onZoom = useCallback((idx: number) => setZoom(idx), []);
  const onHover = useCallback((idx: number) => setFocus(idx), []);

  // T3：分类验收——对某聚类键（ref/group/key）下全部 displayed 项批量通过 / 移废纸篓。
  // 复用 accept/reject（含 inFlight 幂等守卫），触发既有自动转正/写回等副作用。
  const clusterIds = useCallback(
    (key: string) => displayed.filter((it) => clusterKey(it) === key).map((it) => it.id),
    [displayed, clusterKey],
  );
  const acceptCluster = useCallback(
    (key: string) => void accept(clusterIds(key)),
    [accept, clusterIds],
  );
  const rejectCluster = useCallback(
    (key: string) => void reject(clusterIds(key)),
    [reject, clusterIds],
  );

  const zoomItem = zoom !== null ? displayed[zoom] : undefined;

  return (
    <PageScaffold title="图片验收" caption="按原图比例排版 · 网格粗筛 · 大图逐张精审">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <span className="cnt">{items.length} 待验收</span>
        {processed > 0 && <span className="pcap">本批已处理 {processed}</span>}
        {pending.size > 0 && (
          <button
            type="button"
            className={cn("btn sm gho", onlyPending && "on")}
            onClick={() => setOnlyPending((v) => !v)}
            title="仅看标记为待定的图片"
          >
            {onlyPending ? "显示全部" : `仅看待定 · ${pending.size}`}
          </button>
        )}
        <div className="f1" />
        {sel.size > 0 && (
          <>
            <span className="fs12 t2 nowrap">已选 {sel.size}</span>
            <button type="button" className="btn sm" onClick={() => accept([...sel])}>
              通过所选
            </button>
            <button type="button" className="btn sm gho dng" onClick={() => reject([...sel])}>
              移入废纸篓
            </button>
            <button type="button" className="btn sm gho" onClick={() => setSel(new Set())}>
              清除
            </button>
            <span style={{ width: 1, height: 16, background: "var(--line2)" }} />
          </>
        )}
        <div className="seg">
          <span
            className={cn("sgi", sortMode === "batch" && "on")}
            onClick={() => setSortMode("batch")}
            title="最近一批在最顶部，往下依次是更早的批次"
          >
            按批次
          </span>
          <span
            className={cn("sgi", sortMode === "time" && "on")}
            onClick={() => setSortMode("time")}
          >
            时间
          </span>
          <span
            className={cn("sgi", sortMode === "ref" && "on")}
            onClick={() => setSortMode("ref")}
          >
            按参考图
          </span>
          <span
            className={cn("sgi", sortMode === "group" && "on")}
            onClick={() => setSortMode("group")}
          >
            按提示词组
          </span>
          <span
            className={cn("sgi", sortMode === "key" && "on")}
            onClick={() => setSortMode("key")}
          >
            按 Key
          </span>
        </div>
        {/* 齐行下每行张数随比例浮动，故这里给的是**大小**而不是精确列数。 */}
        <span className="fs11 t3 nowrap" title="图片显示大小（每行放几张的目标值）">
          大小
        </span>
        <input
          type="range"
          min={3}
          max={8}
          value={cols}
          onChange={(e) => setCols(Number(e.target.value))}
          // T2：拖动/调整后失焦，方向键网格导航恢复（否则方向键继续改滑块值）。
          onMouseUp={(e) => e.currentTarget.blur()}
          onKeyUp={(e) => e.currentTarget.blur()}
          className="rng"
        />
      </div>

      {items.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">没有待验收的图片</div>
          <div className="fs12 t3">生成完成的任务会自动进入这里 — 网格粗筛，大图逐张精审</div>
        </div>
      ) : (
        <div className="pbody rvscroll" ref={parentRef}>
          <div className="rvirt" style={{ height: virt.getTotalSize() }}>
            {virt.getVirtualItems().map((v) => {
              const row = rows[v.index];
              if (!row) return null;
              return (
                <div
                  key={v.key}
                  data-index={v.index}
                  ref={virt.measureElement}
                  className={cn("rvrow", v.index === 0 && "first", row.kind === "header" && "head")}
                  style={{ transform: `translateY(${v.start}px)` }}
                >
                  {row.kind === "header" ? (
                    <div className="rclhead">
                      <span className="rcltag">
                        {sortMode === "batch"
                          ? "批次"
                          : sortMode === "ref"
                            ? "参考图"
                            : sortMode === "key"
                              ? "Key"
                              : "提示词组"}
                      </span>
                      <span className="rclname" title={row.key}>
                        {row.key}
                      </span>
                      <span className="rclcnt">{row.count} 张</span>
                      {/* T3：分类验收——对本聚类下全部项批量通过 / 移废纸篓（ref/group/key 三种都生效）。 */}
                      <button
                        type="button"
                        className="btn xs"
                        onClick={() => acceptCluster(row.key)}
                      >
                        通过本组
                      </button>
                      <button
                        type="button"
                        className="btn xs gho dng"
                        onClick={() => rejectCluster(row.key)}
                      >
                        本组移废纸篓
                      </button>
                    </div>
                  ) : (
                    <div className="rjrow" style={{ "--rjh": `${row.h}px` } as React.CSSProperties}>
                      {row.items.map(({ it, idx, w }) => (
                        <ReviewCard
                          key={it.id}
                          item={it}
                          idx={idx}
                          width={w}
                          selected={sel.has(it.id)}
                          focused={idx === focus}
                          pending={pending.has(it.id)}
                          onCardClick={onCardClick}
                          onAccept={onAccept}
                          onReject={onReject}
                          onRetry={openRetry}
                          onTogglePending={togglePending}
                          onZoom={onZoom}
                          onHover={onHover}
                        />
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {items.length > 0 && (
        <div className="hintbar">
          <span className="fx ac gap6">
            <span className="kbd">↑</span>
            <span className="kbd">↓</span>
            <span className="kbd">←</span>
            <span className="kbd">→</span>移动焦点
          </span>
          <span className="fx ac gap6">
            <span className="kbd">空格</span>选中
          </span>
          <span className="fx ac gap6">
            <span className="kbd">⏎</span>通过焦点/所选
          </span>
          <span className="fx ac gap6">
            <span className="kbd">⌫</span>不通过
          </span>
          <span className="fx ac gap6">
            <span className="kbd">S</span>待定
          </span>
          <span className="fx ac gap6">
            <span className="kbd">Z</span>大图
          </span>
          <span className="fx ac gap6">
            <span className="kbd">⌘A</span>全选 · <span className="kbd">⇧</span>点选范围
          </span>
        </div>
      )}

      {zoomItem &&
        (() => {
          const refPath = zoomItem.refImagePath ?? zoomItem.refThumbPath;
          const showingRef = (compareRef || holdRef) && !!refPath;
          return (
            <div className="rdet dark">
              <div className="rdimgw">
                <BigImage
                  path={
                    showingRef ? refPath : (zoomItem.resultImagePath ?? zoomItem.resultThumbPath)
                  }
                  caption={
                    showingRef ? `REF · ${zoomItem.refName}` : `GEN · ${zoomItem.promptCode}`
                  }
                />
                {showingRef && <span className="refbadge">参考图</span>}
              </div>
              <div className="rdside">
                <div
                  className="fx ac gap8"
                  style={{ padding: "12px 14px", borderBottom: "1px solid var(--line)" }}
                >
                  <span className="pid">{zoomItem.promptCode}</span>
                  <span className="fs12 fw5 nowrap ohide f1">{zoomItem.groupName}</span>
                  <button type="button" className="icb" onClick={() => setZoom(null)}>
                    <X className="ic12" />
                  </button>
                </div>
                <div className="f1" style={{ overflow: "auto", padding: "12px 14px" }}>
                  <div className="fx ac gap6 wrap">
                    <span className="chip">{zoomItem.refName}</span>
                    {zoomItem.keyAlias && <span className="chip">{zoomItem.keyAlias}</span>}
                  </div>
                  {refPath && (
                    <>
                      <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
                        参考图对比
                      </div>
                      <button
                        type="button"
                        className={cn("refcmp mt6", showingRef && "on")}
                        title="点击切换对比 · 或在大图上按住空格临时查看"
                        onClick={() => setCompareRef((v) => !v)}
                      >
                        <span className="ph refcmpimg" style={bg(zoomItem.refThumbPath)} />
                        <span className="fs11 t2 f1">
                          {showingRef ? "正在看参考图 · 点击回到生成图" : "点击对比参考图"}
                        </span>
                        <span className="kbd">空格</span>
                      </button>
                    </>
                  )}
                  <div className="fs11 fw6 t3 mt14" style={{ letterSpacing: ".05em" }}>
                    提示词原文
                  </div>
                  <div className="ptext mt6">{zoomItem.promptText}</div>
                </div>
                <div className="rdbar">
                  <button
                    type="button"
                    className="icb"
                    onClick={() => setZoom((z) => (z === null ? null : Math.max(0, z - 1)))}
                  >
                    ‹
                  </button>
                  <span className="mono fs11 t3 nowrap">
                    {zoom !== null ? zoom + 1 : 0} / {displayed.length}
                  </span>
                  <button
                    type="button"
                    className="icb"
                    onClick={() =>
                      setZoom((z) => (z === null ? null : Math.min(displayed.length - 1, z + 1)))
                    }
                  >
                    ›
                  </button>
                  <div className="f1" />
                  <button type="button" className="btn sm gho" onClick={() => openRetry(zoomItem)}>
                    重试 R
                  </button>
                  <button
                    type="button"
                    className="btn sm gho dng"
                    onClick={() => reject([zoomItem.id])}
                  >
                    不通过 ⌫
                  </button>
                  <button
                    type="button"
                    className="btn pri sm"
                    onClick={() => accept([zoomItem.id])}
                  >
                    通过 ⏎
                  </button>
                </div>
              </div>
            </div>
          );
        })()}

      {retryTarget && (
        <Modal
          title="重试并微调提示词"
          width="w420"
          onClose={() => setRetryTarget(null)}
          footer={
            <>
              <div className="f1" />
              <button type="button" className="btn sm" onClick={() => setRetryTarget(null)}>
                取消
              </button>
              <button type="button" className="btn pri sm" onClick={() => void submitRetry()}>
                重新生成
              </button>
            </>
          }
        >
          <div className="fx ac gap6 wrap" style={{ marginBottom: 10 }}>
            <span className="pid">{retryTarget.promptCode}</span>
            <span className="chip">{retryTarget.refName}</span>
          </div>
          <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em", marginBottom: 6 }}>
            提示词（可修改后重试）
          </div>
          <textarea
            className="ta"
            style={{ width: "100%", minHeight: 140, resize: "vertical" }}
            value={retryText}
            onChange={(e) => setRetryText(e.target.value)}
            // biome-ignore lint/a11y/noAutofocus: 弹窗即为微调提示词而生，聚焦符合预期
            autoFocus
          />
          <div className="fs11 t3 mt6" style={{ lineHeight: 1.7 }}>
            确认后该任务回到生成队列重新出图；未改动提示词则按原文重试。通过验收后微调版本会写回提示词库。
          </div>
        </Modal>
      )}
    </PageScaffold>
  );
}

/**
 * 验收大图 1:1 查看（E23）：contain 完整显示（不裁剪），滚轮缩放 10%–400%、
 * 拖拽平移、双击在「适应」与「100%（原始像素）」间切换。用于检查 AI 图高发的
 * 手部/文字/边缘缺陷。
 */
function BigImage({ path, caption }: { path?: string | null; caption: string }) {
  const src = assetSrc(path);
  const containerRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const [scale, setScale] = useState(1); // 相对「适应」尺寸的倍数
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  // 自然像素 / 适应像素，用于双击 100% 与真实百分比显示。
  const [fitRatio, setFitRatio] = useState(1);
  const drag = useRef<{ x: number; y: number; px: number; py: number } | null>(null);

  // 切换图片时复位缩放/平移。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 仅在图源变化时复位视图
  useEffect(() => {
    setScale(1);
    setPan({ x: 0, y: 0 });
  }, [src]);

  // 滚轮缩放：以非 passive 原生监听保证 preventDefault 生效。
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      setScale((s) => {
        const next = e.deltaY < 0 ? s * 1.15 : s / 1.15;
        return Math.min(4, Math.max(0.1, next));
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // 拖拽平移：拖动期间挂 window 监听，松开即卸。
  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      setPan({ x: d.px + (e.clientX - d.x), y: d.py + (e.clientY - d.y) });
    };
    const onUp = () => {
      setDragging(false);
      drag.current = null;
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [dragging]);

  const onDoubleClick = () => {
    if (scale !== 1) {
      setScale(1);
      setPan({ x: 0, y: 0 });
      return;
    }
    setScale(Math.min(4, Math.max(1, fitRatio))); // 适应 → 100% 原始像素
  };

  const onLoad = () => {
    const img = imgRef.current;
    if (!img || img.clientWidth === 0) return;
    setFitRatio(img.naturalWidth / img.clientWidth);
  };

  // 真实百分比 = 当前显示像素 / 自然像素。
  const percent = Math.round((scale / fitRatio) * 100);

  return (
    <div
      ref={containerRef}
      className={cn("rzoom", dragging && "drag")}
      onMouseDown={(e) => {
        drag.current = { x: e.clientX, y: e.clientY, px: pan.x, py: pan.y };
        setDragging(true);
      }}
      onDoubleClick={onDoubleClick}
    >
      {src ? (
        <img
          ref={imgRef}
          src={src}
          alt={caption}
          className="rzimg"
          draggable={false}
          onLoad={onLoad}
          style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale})` }}
        />
      ) : (
        <div className="ph rdimg" />
      )}
      <span className="rzbadge">{Number.isFinite(percent) ? percent : 100}%</span>
      <span className="rzhint">滚轮缩放 · 拖拽平移 · 双击 100%</span>
    </div>
  );
}
