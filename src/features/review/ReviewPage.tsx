import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type ReviewItemView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { Check, Maximize2, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function ReviewPage() {
  const [items, setItems] = useState<ReviewItemView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cols, setCols] = useState(5);
  const [zoom, setZoom] = useState<number | null>(null); // index into items
  const [processed, setProcessed] = useState(0);

  const load = useCallback(async () => {
    try {
      setItems(await unwrap(commands.listPendingReview(null)));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const removeIds = (ids: number[]) => {
    setItems((cur) => cur.filter((i) => !ids.includes(i.id)));
    setSel(new Set());
    setProcessed((p) => p + ids.length);
  };

  const accept = useCallback(async (ids: number[]) => {
    if (ids.length === 0) return;
    try {
      const res = await unwrap(commands.acceptTasks(ids));
      removeIds(ids);
      for (const g of res.promotedGroups) toast(`「${g}」已自动写入提示词库`);
      toast.success(`已通过 ${res.accepted} 张`);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);

  const reject = useCallback(async (ids: number[]) => {
    if (ids.length === 0) return;
    try {
      const n = await unwrap(commands.rejectTasks(ids));
      removeIds(ids);
      toast(`${n} 张移入废纸篓`);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);

  // 大图逐张模式键盘
  useEffect(() => {
    if (zoom === null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") return setZoom(null);
      if (e.key === "ArrowLeft") setZoom((z) => (z === null ? null : Math.max(0, z - 1)));
      else if (e.key === "ArrowRight")
        setZoom((z) => (z === null ? null : Math.min(items.length - 1, z + 1)));
      else if (e.key === "Enter") {
        const it = items[zoom];
        if (it) void accept([it.id]);
      } else if (e.key === "Backspace") {
        const it = items[zoom];
        if (it) void reject([it.id]);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [zoom, items, accept, reject]);

  // 大图模式下 items 变化后修正索引
  useEffect(() => {
    if (zoom !== null && zoom >= items.length) setZoom(items.length > 0 ? items.length - 1 : null);
  }, [items, zoom]);

  const toggleSel = (id: number) =>
    setSel((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const zoomItem = zoom !== null ? items[zoom] : undefined;

  return (
    <PageScaffold title="图片验收" caption="网格粗筛 · 大图逐张精审">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <span className="cnt">{items.length} 待验收</span>
        {processed > 0 && <span className="pcap">本批已处理 {processed}</span>}
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
            {items.map((it, idx) => (
              <div
                key={it.id}
                className={cn("rcard", sel.has(it.id) && "sel")}
                onClick={() => toggleSel(it.id)}
                onDoubleClick={() => setZoom(idx)}
              >
                <div className="ph rcimg" style={bg(it.resultThumbPath)} />
                <span className={cn("rck", sel.has(it.id) && "on")}>
                  <Check className="ic12" />
                </span>
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
            <span className="kbd">双击</span>大图
          </span>
          <span className="fx ac gap6">
            <span className="kbd">⏎</span>通过
          </span>
          <span className="fx ac gap6">
            <span className="kbd">⌫</span>不通过
          </span>
          <span className="fx ac gap6">
            <span className="kbd">←</span>
            <span className="kbd">→</span>切换
          </span>
        </div>
      )}

      {zoomItem && (
        <div className="rdet">
          <div className="rdimgw">
            <div
              className="ph rdimg"
              style={bg(zoomItem.resultImagePath ?? zoomItem.resultThumbPath)}
            >
              <span className="phl">GEN · {zoomItem.promptCode} · 1024×1024</span>
            </div>
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
                {zoom !== null ? zoom + 1 : 0} / {items.length}
              </span>
              <button
                type="button"
                className="icb"
                onClick={() =>
                  setZoom((z) => (z === null ? null : Math.min(items.length - 1, z + 1)))
                }
              >
                ›
              </button>
              <div className="f1" />
              <button
                type="button"
                className="btn sm gho dng"
                onClick={() => reject([zoomItem.id])}
              >
                不通过 ⌫
              </button>
              <button type="button" className="btn pri sm" onClick={() => accept([zoomItem.id])}>
                通过 ⏎
              </button>
            </div>
          </div>
        </div>
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
