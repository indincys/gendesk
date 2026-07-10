import { Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type BatchView, type ReviewItemView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { Check, ChevronDown, Clock, Maximize2, RotateCcw, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

export function ReviewPage() {
  const [items, setItems] = useState<ReviewItemView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cols, setCols] = useState(5);
  // E09：网格键盘焦点（索引进 displayed）。
  const [focus, setFocus] = useState(0);
  // E38：待定标记（纯 UI 态，不入库）——沉底 + 角标 + 可筛选。
  const [pending, setPending] = useState<Set<number>>(new Set());
  const [onlyPending, setOnlyPending] = useState(false);
  // E38：shift 范围多选锚点（索引进 displayed）。
  const lastClicked = useRef<number | null>(null);
  // E08：大图参考图对比——持久切换 compareRef，或按住空格临时 peek。
  const [compareRef, setCompareRef] = useState(false);
  const [holdRef, setHoldRef] = useState(false);
  // E29：按批次筛选（null = 全部批次混排）。
  const [batches, setBatches] = useState<BatchView[]>([]);
  const [batchFilter, setBatchFilter] = useState<number | null>(null);
  const [showBatchPicker, setShowBatchPicker] = useState(false);
  const [zoom, setZoom] = useState<number | null>(null); // index into items
  const [processed, setProcessed] = useState(0);
  // 「重试 + 微调提示词」目标（E01）：打开编辑框，确认后微调写快照并回队。
  const [retryTarget, setRetryTarget] = useState<ReviewItemView | null>(null);
  const [retryText, setRetryText] = useState("");
  // 正在处理中的任务 id（防长按 ⏎ / 连点重复提交同一任务，后端另有幂等守卫兜底）。
  const inFlight = useRef<Set<number>>(new Set());

  const load = useCallback(async () => {
    try {
      setItems(await unwrap(commands.listPendingReview(batchFilter)));
      setSel(new Set());
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, [batchFilter]);
  useEffect(() => {
    void load();
  }, [load]);
  // 批次列表用于筛选器（仅需展示存在待验收项的批次即可，这里取全部批次）。
  useEffect(() => {
    void (async () => {
      try {
        setBatches(await unwrap(commands.listBatches()));
      } catch {
        /* 筛选器可用性非关键，静默 */
      }
    })();
  }, []);

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

  const toggleSel = (id: number) =>
    setSel((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const togglePending = (id: number) =>
    setPending((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  // E38：显示序——「仅看待定」筛选；否则待定项稳定沉底（保留原相对序）。
  const displayed = useMemo(() => {
    if (onlyPending) return items.filter((i) => pending.has(i.id));
    return [...items].sort((a, b) => Number(pending.has(a.id)) - Number(pending.has(b.id)));
  }, [items, pending, onlyPending]);

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
  // biome-ignore lint/correctness/useExhaustiveDependencies: toggleSel/togglePending 为稳定内联 setter，无需入依赖
  useEffect(() => {
    if (zoom !== null || retryTarget) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
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
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocus((f) => Math.min(n - 1, f + cols));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocus((f) => Math.max(0, f - cols));
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
  }, [zoom, retryTarget, displayed, focus, cols, sel, accept, reject]);

  // E09：焦点越界修正 + 滚动进视野。
  useEffect(() => {
    if (focus >= displayed.length) setFocus(Math.max(0, displayed.length - 1));
  }, [displayed, focus]);
  useEffect(() => {
    document.querySelector(`[data-ridx="${focus}"]`)?.scrollIntoView({ block: "nearest" });
  }, [focus]);

  // E38：网格点选——shift 从锚点范围加选，否则切换单项并设锚点。
  const onCardClick = (idx: number, shift: boolean) => {
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
  };

  const zoomItem = zoom !== null ? displayed[zoom] : undefined;

  return (
    <PageScaffold title="图片验收" caption="网格粗筛 · 大图逐张精审">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <button type="button" className="btn sm gho" onClick={() => setShowBatchPicker(true)}>
          {batchFilter == null ? "全部批次" : `批次 #${batchFilter}`}
          <ChevronDown className="ic12" />
        </button>
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
        <span className="fs11 t3 nowrap">每行</span>
        <input
          type="range"
          min={3}
          max={8}
          value={cols}
          onChange={(e) => setCols(Number(e.target.value))}
          className="rng"
        />
      </div>

      {items.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">没有待验收的图片</div>
          <div className="fs12 t3">生成完成的任务会自动进入这里 — 网格粗筛，大图逐张精审</div>
        </div>
      ) : (
        <div className="pbody">
          <div className="rgrid" style={{ gridTemplateColumns: `repeat(${cols},1fr)` }}>
            {displayed.map((it, idx) => (
              <div
                key={it.id}
                data-ridx={idx}
                className={cn(
                  "rcard",
                  sel.has(it.id) && "sel",
                  idx === focus && "focus",
                  pending.has(it.id) && "pend",
                )}
                onClick={(e) => onCardClick(idx, e.shiftKey)}
                onDoubleClick={() => setZoom(idx)}
              >
                <div className="ph rcimg" style={bg(it.resultThumbPath)} />
                <span className={cn("rck", sel.has(it.id) && "on")}>
                  <Check className="ic12" />
                </span>
                {pending.has(it.id) && <span className="rpend">待定</span>}
                <div className="hacts">
                  <button
                    type="button"
                    className="hbtn"
                    title="通过"
                    onClick={(e) => {
                      e.stopPropagation();
                      void accept([it.id]);
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
                      void reject([it.id]);
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
                      openRetry(it);
                    }}
                  >
                    <RotateCcw className="ic12" />
                  </button>
                  <button
                    type="button"
                    className={cn("hbtn", pending.has(it.id) && "on")}
                    title="标记待定（稍后再定）"
                    onClick={(e) => {
                      e.stopPropagation();
                      togglePending(it.id);
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
                      setZoom(idx);
                    }}
                  >
                    <Maximize2 className="ic12" />
                  </button>
                </div>
                <div className="rmeta">
                  <span className="pid">{it.promptCode}</span>
                  <span className="fs10 t3 mono nowrap ohide">{it.refName}</span>
                </div>
              </div>
            ))}
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

      {showBatchPicker && (
        <div className="ovl" onClick={() => setShowBatchPicker(false)}>
          <div className="mdl w420" onClick={(e) => e.stopPropagation()}>
            <div className="mhead">
              <span className="fw6 fs13">按批次筛选</span>
              <div className="f1" />
            </div>
            <div className="mlist">
              <div
                className="pickrow"
                onClick={() => {
                  setBatchFilter(null);
                  setShowBatchPicker(false);
                }}
              >
                <span className={cn("ckb", batchFilter == null && "on")} />
                <span className="fs12 f1">全部批次</span>
              </div>
              {batches.map((b) => (
                <div
                  key={b.id}
                  className="pickrow"
                  onClick={() => {
                    setBatchFilter(b.id);
                    setShowBatchPicker(false);
                  }}
                >
                  <span className={cn("ckb", b.id === batchFilter && "on")} />
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

function bg(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
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
