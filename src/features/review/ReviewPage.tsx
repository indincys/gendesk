import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { moveByRow, packGroups } from "@/features/_shared/justified";
import { dayLabel, hhmm } from "@/features/_shared/timeline";
import { assetSrc, bg } from "@/lib/img";
import { type ReviewItemView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useEngineStore } from "@/stores/engine";
import { modKeyLabel, useUiStore } from "@/stores/ui";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Check, Clock, Maximize2, Trash2, X } from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import {
  type ReviewHead,
  type ReviewMode,
  buildSections,
  itemLabel,
  readableName,
  refLabel,
} from "./model";

/** 一张图的宽高比。缺尺寸时用 1（历史任务后端会补齐，这里只兜住补不到的极端情况）。 */
function ratioOf(it: ReviewItemView): number {
  const w = it.resultWidth ?? 0;
  const h = it.resultHeight ?? 0;
  return w > 0 && h > 0 ? w / h : 1;
}

/** 卡片间距，参与齐行的宽度分配 —— 不算进去每行最后一张会被挤出边界。 */
const GAP = 10;

// 与 globals.css 中卡片元数据、行距和分组头的固定尺寸一致。
const CARD_META_H = 26;
const ROW_GAP = 14;
const DAY_HEAD_H = 50;
const CLUSTER_HEAD_H = 68;

const OTHER_MODES = [
  { id: "time", label: "按时间" },
  { id: "ref", label: "按参考图" },
  { id: "group", label: "按提示词组" },
  { id: "key", label: "按 Key" },
] as const;

function ClusterHead({
  head,
  busy,
  onAccept,
  onReject,
}: {
  head: Extract<ReviewHead, { kind: "cluster" }>;
  busy: boolean;
  onAccept: (ids: number[]) => void;
  onReject: (ids: number[]) => void;
}) {
  const count = head.ids.length;
  return (
    <div className="rclhead">
      <div className="rclcopy">
        <h2 className="rclname" title={head.name}>
          {head.name}
        </h2>
        <span className="rcltime" title={head.detail}>
          {head.detail}
        </span>
      </div>
      <span className="rclcnt">{count} 张待验收</span>
      <button
        type="button"
        className="btn xs"
        disabled={busy}
        title={`通过「${head.name}」当前显示的 ${count} 张图片`}
        onClick={() => onAccept(head.ids)}
      >
        通过本组
      </button>
      <button
        type="button"
        className="btn xs gho"
        disabled={busy}
        aria-label={`将「${head.name}」当前显示的 ${count} 张图片移入废纸篓`}
        title="本组移入废纸篓"
        onClick={() => onReject(head.ids)}
      >
        <Trash2 className="ic12" />
      </button>
    </div>
  );
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
  busy: boolean;
  onCardClick: (idx: number, shift: boolean) => void;
  onAccept: (id: number) => void;
  onReject: (id: number) => void;
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
  busy,
  onCardClick,
  onAccept,
  onReject,
  onTogglePending,
  onZoom,
  onHover,
}: ReviewCardProps) {
  const name = itemLabel(item);
  return (
    <div
      className={cn("rcard rjcard", selected && "sel", focused && "focus", pending && "pend")}
      style={{ width }}
      title={[name, item.promptCode, readableName(item.groupName), item.skill]
        .filter(Boolean)
        .join(" · ")}
      onClick={(e) => onCardClick(idx, e.shiftKey)}
      onDoubleClick={() => onZoom(idx)}
      // T2：指针跟随焦点——悬停即设焦点，令空格/回车作用于鼠标所指卡片而非默认第 0 张。
      onMouseEnter={() => onHover(idx)}
    >
      {/* 图框铺满这一格：格子本身已按真实宽高比算好，故 cover 不会裁掉任何东西。 */}
      <NatThumb path={item.resultThumbPath} className="rcimg rjimg" />
      <button
        type="button"
        className="rck"
        aria-label={`选择 ${name}`}
        aria-pressed={selected}
        onFocus={() => onHover(idx)}
        onClick={(e) => {
          e.stopPropagation();
          onCardClick(idx, e.shiftKey);
        }}
      >
        <Check className="ic12" />
      </button>
      {pending && <span className="rpend">待定</span>}
      <div className="hacts">
        <button
          type="button"
          className="hbtn"
          title="通过"
          disabled={busy}
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
          title="移入废纸篓"
          disabled={busy}
          onClick={(e) => {
            e.stopPropagation();
            onReject(item.id);
          }}
        >
          <X className="ic12" />
        </button>
        <button
          type="button"
          className={cn("hbtn", pending && "on")}
          aria-pressed={pending}
          title={pending ? "取消待定" : "标记待定"}
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
        <span className="rtitle">{name}</span>
      </div>
    </div>
  );
});

export function ReviewPage() {
  const [items, setItems] = useState<ReviewItemView[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cols, setCols] = useState(5);
  const [focusId, setFocusId] = useState<number | null>(null);
  const [pending, setPending] = useState<Set<number>>(new Set());
  const [onlyPending, setOnlyPending] = useState(false);
  const [sortMode, setSortMode] = useState<ReviewMode>("requirement");
  const lastClicked = useRef<number | null>(null);
  const [compareRef, setCompareRef] = useState(false);
  const [holdRef, setHoldRef] = useState(false);
  const [measureW, setMeasureW] = useState(0);
  const [zoom, setZoom] = useState(false);
  const [processed, setProcessed] = useState(0);
  const [busy, setBusy] = useState(false);
  const [now] = useState(() => Math.floor(Date.now() / 1000));
  const inFlight = useRef(false);
  const loadSequence = useRef(0);
  const parentRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  const wasZoom = useRef(false);
  const reviewRevision = useEngineStore((s) => s.reviewRevision);
  const modKey = useUiStore((s) => modKeyLabel(s.platform));

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current;
    try {
      const result = await unwrap(commands.listPendingReview(null));
      if (sequence !== loadSequence.current) return;
      setItems(result);
      setLoadError("");
    } catch (error) {
      if (sequence !== loadSequence.current) return;
      setLoadError(error instanceof Error ? error.message : "无法读取待验收图片");
    }
  }, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 生图结束后的批量事件触发重载
  useEffect(() => {
    void load();
    return () => {
      loadSequence.current++;
    };
  }, [load, reviewRevision]);

  useLayoutEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setMeasureW(entry.contentRect.width);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const process = useCallback(
    async (ids: number[], action: "accept" | "reject") => {
      if (ids.length === 0 || inFlight.current) return;
      inFlight.current = true;
      setBusy(true);
      try {
        if (action === "accept") {
          const result = await unwrap(commands.acceptTasks(ids));
          setProcessed((count) => count + result.accepted);
          for (const name of result.promotedGroups) {
            toast(`「${readableName(name) || "本组提示词"}」已写入文案库`);
          }
          toast.success(`已通过 ${result.accepted} 张`);
        } else {
          const count = await unwrap(commands.rejectTasks(ids));
          setProcessed((total) => total + count);
          toast(`${count} 张移入废纸篓`);
        }
      } catch (error) {
        toast.error(error instanceof Error ? error.message : "验收操作失败");
      } finally {
        // 部分成功或状态已被其它入口改动时，按后端的实际待验收列表继续。
        await load();
        inFlight.current = false;
        setBusy(false);
      }
    },
    [load],
  );
  const accept = useCallback((ids: number[]) => void process(ids, "accept"), [process]);
  const reject = useCallback((ids: number[]) => void process(ids, "reject"), [process]);

  const toggleSel = useCallback((id: number) => {
    setSel((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const togglePending = useCallback((id: number) => {
    setPending((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const pendingCount = items?.filter((item) => pending.has(item.id)).length ?? 0;
  const filtered = useMemo(
    () => (onlyPending ? (items ?? []).filter((item) => pending.has(item.id)) : (items ?? [])),
    [items, onlyPending, pending],
  );
  const visibleIds = useMemo(() => new Set(filtered.map((item) => item.id)), [filtered]);
  const selectedIds = useMemo(() => [...sel].filter((id) => visibleIds.has(id)), [sel, visibleIds]);
  const sections = useMemo(() => buildSections(filtered, sortMode, now), [filtered, sortMode, now]);
  const { blocks, cardRow, flat } = useMemo(
    () => packGroups(sections, { width: measureW, perRow: cols, gap: GAP, ratioOf }),
    [sections, cols, measureW],
  );

  // 同样的行数也可能有不同的行高，令虚拟列表随排版重新计算偏移。
  const getItemKey = useCallback((index: number) => blocks[index]?.key ?? index, [blocks]);
  const virt = useVirtualizer({
    count: measureW > 0 ? blocks.length : 0,
    getScrollElement: () => parentRef.current,
    getItemKey,
    paddingStart: 8,
    paddingEnd: 12,
    estimateSize: (index) => {
      const block = blocks[index];
      if (block?.kind === "head") return block.head.kind === "day" ? DAY_HEAD_H : CLUSTER_HEAD_H;
      return (block?.h ?? 0) + CARD_META_H + ROW_GAP;
    },
    overscan: 6,
  });

  const focus = flat.findIndex((item) => item.id === focusId);
  const previousFlat = useRef(flat);
  useLayoutEffect(() => {
    if (focus < 0) {
      const previous = previousFlat.current;
      const index = previous.findIndex((item) => item.id === focusId);
      // 当前图消失时先找原顺序的下一张，再往前找上一张。
      const next =
        index < 0
          ? flat[0]
          : (previous.slice(index + 1).find((item) => visibleIds.has(item.id)) ??
            previous
              .slice(0, index)
              .reverse()
              .find((item) => visibleIds.has(item.id)) ??
            flat[0]);
      setFocusId(next?.id ?? null);
      if (!next) setZoom(false);
    }
    previousFlat.current = flat;
  }, [flat, focus, focusId, visibleIds]);

  const scrollToCard = useCallback(
    (index: number) => {
      const row = cardRow[index];
      if (row !== undefined) virt.scrollToIndex(row, { align: "auto" });
    },
    [virt, cardRow],
  );

  const onHover = useCallback((index: number) => setFocusId(flat[index]?.id ?? null), [flat]);
  const navigate = useCallback(
    (index: number) => {
      const next = Math.max(0, Math.min(index, flat.length - 1));
      onHover(next);
      if (!zoom) scrollToCard(next);
    },
    [flat.length, onHover, scrollToCard, zoom],
  );
  const onZoom = useCallback(
    (index: number) => {
      onHover(index);
      setZoom(true);
    },
    [onHover],
  );
  const closeZoom = useCallback(() => {
    setZoom(false);
    scrollToCard(focus);
  }, [focus, scrollToCard]);

  useLayoutEffect(() => {
    if (zoom) detailRef.current?.focus();
    else if (wasZoom.current) parentRef.current?.focus({ preventScroll: true });
    wasZoom.current = zoom;
  }, [zoom]);

  // 对比状态属于当前图片。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 切换图片或重开详情时复位
  useLayoutEffect(() => {
    setCompareRef(false);
    setHoldRef(false);
  }, [focusId, zoom]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (
        e.defaultPrevented ||
        e.isComposing ||
        useUiStore.getState().helpOpen ||
        document.querySelector(".ovl")
      )
        return;
      const item = flat[focus];
      if (!item) return;
      const target = e.target as HTMLElement | null;
      if (zoom && e.key === "Escape") {
        e.preventDefault();
        closeZoom();
        return;
      }
      if (zoom && e.key === "Tab") {
        const buttons =
          detailRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
        if (!buttons?.length) return;
        const first = buttons[0];
        const last = buttons[buttons.length - 1];
        if (e.shiftKey ? target === first || target === detailRef.current : target === last) {
          e.preventDefault();
          (e.shiftKey ? last : first)?.focus();
        }
        return;
      }
      if (target?.closest("input, select, textarea, [contenteditable=true]")) return;
      if (target?.closest("button") && ["Enter", " ", "Backspace"].includes(e.key)) return;
      if (!zoom && (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        setSel(new Set(flat.map((entry) => entry.id)));
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      switch (e.key) {
        case "ArrowLeft":
        case "ArrowRight":
          e.preventDefault();
          navigate(focus + (e.key === "ArrowLeft" ? -1 : 1));
          break;
        case "ArrowUp":
        case "ArrowDown":
          if (zoom) return;
          e.preventDefault();
          navigate(moveByRow(blocks, cardRow, focus, e.key === "ArrowDown" ? 1 : -1));
          break;
        case " ":
          e.preventDefault();
          if (zoom) setHoldRef(true);
          else if (!e.repeat) toggleSel(item.id);
          break;
        case "Enter":
          e.preventDefault();
          if (!e.repeat) accept(!zoom && selectedIds.length ? selectedIds : [item.id]);
          break;
        case "Backspace":
          e.preventDefault();
          if (!e.repeat) reject([item.id]);
          break;
        case "s":
        case "S":
          if (!e.repeat) togglePending(item.id);
          break;
        case "z":
        case "Z":
          if (!zoom && !e.repeat) onZoom(focus);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    zoom,
    flat,
    focus,
    blocks,
    cardRow,
    selectedIds,
    accept,
    reject,
    toggleSel,
    togglePending,
    navigate,
    onZoom,
    closeZoom,
  ]);

  useEffect(() => {
    if (!zoom) return;
    const release = () => setHoldRef(false);
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === " ") release();
    };
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", release);
    return () => {
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", release);
    };
  }, [zoom]);

  const onCardClick = useCallback(
    (index: number, shift: boolean) => {
      const item = flat[index];
      if (!item) return;
      setFocusId(item.id);
      const anchor = shift ? flat.findIndex((entry) => entry.id === lastClicked.current) : -1;
      if (anchor >= 0) {
        const range = flat.slice(Math.min(anchor, index), Math.max(anchor, index) + 1);
        setSel((prev) => new Set([...prev, ...range.map((entry) => entry.id)]));
      } else {
        toggleSel(item.id);
        lastClicked.current = item.id;
      }
    },
    [flat, toggleSel],
  );

  const onAccept = useCallback((id: number) => accept([id]), [accept]);
  const onReject = useCallback((id: number) => reject([id]), [reject]);

  function changeView(mode: ReviewMode, pendingOnly: boolean) {
    if (mode === sortMode && pendingOnly === onlyPending) return;
    setSortMode(mode);
    setOnlyPending(pendingOnly);
    setSel(new Set());
    setFocusId(null);
    lastClicked.current = null;
    parentRef.current?.scrollTo({ top: 0 });
  }

  const zoomItem = zoom ? flat[focus] : undefined;

  return (
    <PageScaffold title="图片验收" caption="浏览生成结果，标记待定或逐张精审" bodyClassName="rbody">
      <div className="rworkspace" inert={zoom}>
        <div className="phd rtoolbar">
          <div className="fx ac gap8">
            <span className="rstatus" aria-live="polite">
              {items === null ? "正在加载" : `${items.length} 张待验收`}
            </span>
            {processed > 0 && <span className="pcap">本次已处理 {processed} 张</span>}
          </div>
          <div className="f1" />
          {pendingCount > 0 || onlyPending ? (
            <button
              type="button"
              className={cn("btn sm gho", onlyPending && "on")}
              aria-pressed={onlyPending}
              onClick={() => changeView(sortMode, !onlyPending)}
            >
              {onlyPending ? "显示全部" : `仅看待定 · ${pendingCount}`}
            </button>
          ) : null}
          <div className="seg" role="group" aria-label="验收分组方式">
            <button
              type="button"
              className={cn("sgi", sortMode === "requirement" && "on")}
              aria-pressed={sortMode === "requirement"}
              onClick={() => changeView("requirement", onlyPending)}
            >
              按需求
            </button>
            <button
              type="button"
              className={cn("sgi", sortMode === "batch" && "on")}
              aria-pressed={sortMode === "batch"}
              onClick={() => changeView("batch", onlyPending)}
            >
              按提交
            </button>
          </div>
          <select
            className="rmode"
            aria-label="其他分组方式"
            value={sortMode === "requirement" || sortMode === "batch" ? "" : sortMode}
            onChange={(e) => {
              const mode = OTHER_MODES.find(({ id }) => id === e.target.value);
              if (mode) changeView(mode.id, onlyPending);
            }}
          >
            <option value="" disabled>
              其他分组
            </option>
            {OTHER_MODES.map(({ id, label }) => (
              <option key={id} value={id}>
                {label}
              </option>
            ))}
          </select>
          <label className="fx ac gap8 fs11 t2 nowrap">
            缩略图大小
            <input
              type="range"
              min={3}
              max={8}
              value={11 - cols}
              onChange={(e) => setCols(11 - Number(e.target.value))}
              className="rng"
            />
          </label>
        </div>
        {selectedIds.length > 0 && (
          <div className="rselection">
            <span className="fw6">已选 {selectedIds.length} 张</span>
            <div className="f1" />
            <button
              type="button"
              className="btn xs"
              disabled={busy}
              onClick={() => accept(selectedIds)}
            >
              通过所选
            </button>
            <button
              type="button"
              className="btn xs gho dng"
              disabled={busy}
              onClick={() => reject(selectedIds)}
            >
              移入废纸篓
            </button>
            <button type="button" className="btn xs gho" onClick={() => setSel(new Set())}>
              清除选择
            </button>
          </div>
        )}
        {loadError && (
          <div className="rselection" role="alert">
            <span>读取待验收图片失败 · {loadError}</span>
            <button type="button" className="btn xs" onClick={() => void load()}>
              重新加载
            </button>
          </div>
        )}
        <div className="pbody rvscroll" ref={parentRef} tabIndex={-1}>
          {filtered.length === 0 ? (
            <div className="bigempty">
              <div className="fs13 fw5 t2">
                {onlyPending
                  ? "没有标记为待定的图片"
                  : items === null
                    ? loadError
                      ? "暂时无法打开验收列表"
                      : "正在加载图片…"
                    : "没有待验收的图片"}
              </div>
              {onlyPending ? (
                <button
                  type="button"
                  className="btn sm"
                  onClick={() => changeView(sortMode, false)}
                >
                  返回全部图片
                </button>
              ) : (
                items && <div className="fs12 t3">生成完成的图片会自动进入这里</div>
              )}
            </div>
          ) : (
            <div className="rvirt" style={{ height: virt.getTotalSize() }}>
              {virt.getVirtualItems().map((v) => {
                const block = blocks[v.index];
                if (!block) return null;
                return (
                  <div
                    key={v.key}
                    className={cn("rvrow", block.kind === "head" && `head ${block.head.kind}`)}
                    style={{ transform: `translateY(${v.start}px)` }}
                  >
                    {block.kind === "head" ? (
                      block.head.kind === "day" ? (
                        <div className="rvday">
                          <span className="d">{block.head.label}</span>
                          <span className="ln" />
                          <span className="n">{block.head.count} 张</span>
                        </div>
                      ) : (
                        <ClusterHead
                          head={block.head}
                          busy={busy}
                          onAccept={accept}
                          onReject={reject}
                        />
                      )
                    ) : (
                      <div
                        className="rjrow"
                        style={{ "--rjh": `${block.h}px` } as React.CSSProperties}
                      >
                        {block.items.map(({ it, idx, w }) => (
                          <ReviewCard
                            key={it.id}
                            item={it}
                            idx={idx}
                            width={w}
                            selected={sel.has(it.id)}
                            focused={it.id === focusId}
                            pending={pending.has(it.id)}
                            busy={busy}
                            onCardClick={onCardClick}
                            onAccept={onAccept}
                            onReject={onReject}
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
          )}
        </div>
        {filtered.length > 0 && (
          <div className="hintbar">
            <span>
              <kbd>↑ ↓ ← →</kbd> 移动
            </span>
            <span>
              <kbd>空格</kbd> 选中
            </span>
            <span>
              <kbd>⏎</kbd> 通过
            </span>
            <span>
              <kbd>⌫</kbd> 移入废纸篓
            </span>
            <span>
              <kbd>S</kbd> 待定
            </span>
            <span>
              <kbd>Z</kbd> 大图
            </span>
            <span>
              <kbd>{modKey === "⌘" ? "⌘A" : "Ctrl+A"}</kbd> 全选
            </span>
            <span>
              <kbd>⇧</kbd> 点选范围
            </span>
          </div>
        )}
      </div>

      {zoomItem &&
        (() => {
          const refPath = zoomItem.refImagePath ?? zoomItem.refThumbPath;
          const showingRef = (compareRef || holdRef) && !!refPath;
          const organization = zoomItem.organization;
          const context = [organization?.product, organization?.style, organization?.requirement]
            .filter(Boolean)
            .join(" · ");
          return (
            <div
              className="rdet dark"
              ref={detailRef}
              tabIndex={-1}
              role="dialog"
              aria-modal="true"
              aria-label="图片精审"
            >
              <div className="rdimgw">
                <BigImage
                  path={
                    showingRef ? refPath : (zoomItem.resultImagePath ?? zoomItem.resultThumbPath)
                  }
                  caption={showingRef ? refLabel(zoomItem) : itemLabel(zoomItem)}
                />
                {showingRef && <span className="refbadge">参考图</span>}
              </div>
              <div className="rdside">
                <div className="rdhead">
                  <div className="rclcopy">
                    <h2 className="rclname" title={itemLabel(zoomItem)}>
                      {itemLabel(zoomItem)}
                    </h2>
                    <span className="rcltime" title={context}>
                      {context || readableName(zoomItem.groupName) || "图片精审"}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="icb"
                    aria-label="关闭大图"
                    title="关闭大图 Esc"
                    onClick={closeZoom}
                  >
                    <X className="ic12" />
                  </button>
                </div>
                <div className="f1" style={{ overflow: "auto", padding: "12px 14px" }}>
                  <div className="fx ac gap6 wrap">
                    {zoomItem.promptCode && <span className="pid">{zoomItem.promptCode}</span>}
                    <span className="chip" title={zoomItem.batchNote ?? undefined}>
                      提交 #{zoomItem.batchId}
                    </span>
                    <span className="chip">
                      {dayLabel(zoomItem.createdAt, now)} {hhmm(zoomItem.createdAt)}
                    </span>
                    {zoomItem.skill && <span className="sk">{zoomItem.skill}</span>}
                  </div>
                  {refPath && (
                    <>
                      <div className="fs11 fw6 t3 mt14">参考图对比</div>
                      <button
                        type="button"
                        className={cn("refcmp mt6", showingRef && "on")}
                        aria-pressed={compareRef}
                        title={refLabel(zoomItem)}
                        onClick={(e) => {
                          setCompareRef((value) => !value);
                          if (e.detail) detailRef.current?.focus();
                        }}
                      >
                        <span className="ph refcmpimg" style={bg(zoomItem.refThumbPath)} />
                        <span className="fs11 f1">
                          {showingRef ? "返回生成图" : "查看主参考图"}
                        </span>
                        <span className="kbd">按住空格</span>
                      </button>
                    </>
                  )}
                  <div className="fs11 fw6 t3 mt14">提示词原文</div>
                  <div className="ptext mt6">{zoomItem.promptText}</div>
                </div>
                <div className="rdbar">
                  <button
                    type="button"
                    className="icb"
                    aria-label="上一张"
                    disabled={focus === 0}
                    onClick={(e) => {
                      navigate(focus - 1);
                      if (e.detail) detailRef.current?.focus();
                    }}
                  >
                    ‹
                  </button>
                  <span className="mono fs11 t3 nowrap" aria-live="polite">
                    {focus + 1} / {flat.length}
                  </span>
                  <button
                    type="button"
                    className="icb"
                    aria-label="下一张"
                    disabled={focus + 1 >= flat.length}
                    onClick={(e) => {
                      navigate(focus + 1);
                      if (e.detail) detailRef.current?.focus();
                    }}
                  >
                    ›
                  </button>
                  <div className="f1" />
                  <button
                    type="button"
                    className="btn xs gho"
                    title={pending.has(zoomItem.id) ? "取消待定 S" : "标记待定 S"}
                    aria-label="待定"
                    aria-pressed={pending.has(zoomItem.id)}
                    onClick={(e) => {
                      togglePending(zoomItem.id);
                      if (e.detail) detailRef.current?.focus();
                    }}
                  >
                    <Clock className="ic12" />
                  </button>
                  <button
                    type="button"
                    className="btn xs gho dng"
                    title="移入废纸篓 ⌫"
                    disabled={busy}
                    onClick={() => reject([zoomItem.id])}
                  >
                    移废纸篓
                  </button>
                  <button
                    type="button"
                    className="btn pri xs"
                    title="通过 ⏎"
                    disabled={busy}
                    onClick={() => accept([zoomItem.id])}
                  >
                    通过
                  </button>
                </div>
              </div>
            </div>
          );
        })()}
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
