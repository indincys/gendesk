import { ConfirmModal } from "@/components/ui/Modal";
import { moveByRow } from "@/features/_shared/justified";
import { TrashLightbox } from "@/features/trash/TrashLightbox";
import {
  type TileHandlers,
  TrashCardsRow,
  TrashHeadRow,
  TrashListRow,
} from "@/features/trash/TrashRows";
import { TrashSide } from "@/features/trash/TrashSide";
import {
  TRASH_GAP,
  TRASH_LIST_ROW_H,
  blockHeight,
  buildBlocks,
  buildListBlocks,
  groupTrash,
} from "@/features/trash/model";
import { type TrashItemView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { TRASH_SIZE_MAX, TRASH_SIZE_MIN, useTrashUiStore } from "@/stores/trash";
import { useUiStore } from "@/stores/ui";
import { useVirtualizer } from "@tanstack/react-virtual";
import { LayoutGrid, List, Trash2, Undo2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 废纸篓工作台（按视频流水线那一页的骨架重做）。
 *
 * ## 这一页要回答的是「那次任务出了什么问题」
 *
 * 它此前是一张按删除时刻倒序的平铺列表：三百行一模一样的 54px 高的行，谁也认不出
 * 哪几条是同一次跑出来的。而人打开废纸篓，十次里有九次带着一个具体的问题 ——
 * 「前天那批脸怎么都糊了」「昨天下午误删的那张还在不在」。所以这一版做了两件事：
 *
 * 1. **时间线切两级**（日 → 任务簇，判据见 `model.ts`）。段与段之间是看得见的断口。
 * 2. **默认按真实比例铺成网格**。一批图的毛病（构图全歪、人物全糊）是**看**出来的，
 *    不是从编号里读出来的；而列表模式仍在，因为「找编号 XX-0042 那一条」反过来
 *    是扫读最快。
 *
 * ## 三块的分工（同视频工作台）
 *
 * 主区（这一屏有哪些）｜ 详情栏（光标这一条是什么、还回得去吗）｜ 底坞（拿它们怎么办）。
 * 详情栏常驻而不是弹窗：逐条排查时，开窗-看-关窗会把一屏能过的条数打掉一个数量级。
 *
 * ## 键盘
 *
 * ↑/↓/←/→ 移动光标 · 空格 勾选 · ⇧/⌘ 点选一段/加选 · ⏎ 放大查看 · R 还原 ·
 * ⌫ 彻底删除（走确认卡）· ⌘A 全选 · Esc 清除勾选 / 退出放大。
 */
export function TrashPage() {
  const [rows, setRows] = useState<TrashItemView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [focus, setFocus] = useState(0);
  /** 正在放大查看的全局序号（null = 没开）。 */
  const [zoom, setZoom] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<null | { ids: number[]; all: boolean }>(null);
  /** ⇧ 范围选的锚点（全局序号）。 */
  const anchor = useRef<number | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  /**
   * 齐行排版所需的容器宽度。**只随窗口/侧栏变化**，与图片加载无关 ——
   * 用图片自身尺寸去反推布局才会抖，用容器宽度不会。
   */
  const [measureW, setMeasureW] = useState(900);
  /**
   * 「今天/昨天」的参照时刻。整页只取一次（进页面时），不跟着秒走：
   * 跨零点时标题从「今天」变「昨天」是对的，但那要等下一次进页面 ——
   * 每秒重算会让整棵派生树每秒重建一遍，而这一页可能挂着几百个 <img>。
   */
  const [now] = useState(() => Math.floor(Date.now() / 1000));

  const mode = useTrashUiStore((s) => s.mode);
  const size = useTrashUiStore((s) => s.size);
  const setMode = useTrashUiStore((s) => s.setMode);
  const setSize = useTrashUiStore((s) => s.setSize);

  const load = useCallback(async () => {
    try {
      setRows(await unwrap(commands.listTrash()));
      setSel(new Set());
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  // 容器宽度：齐行的行高由它推出，故必须在渲染前就是对的。
  // 依赖 hasRows 是因为滚动容器只在非空时挂载（空态是另一棵子树）。
  const hasRows = rows.length > 0;
  // biome-ignore lint/correctness/useExhaustiveDependencies: 容器挂载/卸载随 hasRows 切换，须重挂观察器
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => {
      const w = e?.contentRect.width;
      if (w) setMeasureW((cur) => (Math.abs(w - cur) > 1 ? w : cur));
    });
    // 初值**只**由观察器给（它 observe 后立刻回调一次）。别拿 `clientWidth` 兜底：
    // 它把左右内边距算进去，于是每一行都会比可用宽度宽 32px —— 那正是「最后一张
    // 被挤出边界、整页横向出滚动条」的来路。`contentRect` 才是已扣掉内边距的那个。
    ro.observe(el);
    return () => ro.disconnect();
  }, [hasRows]);

  const days = useMemo(() => groupTrash(rows, now), [rows, now]);
  // 两种布局产出同一种块（列表就是「每行一条」），于是键盘导航、选区、光标滚动
  // 三处对两种布局是同一份代码 —— 换布局时光标停在原处。
  const { blocks, cardRow, flat } = useMemo(
    () =>
      mode === "list"
        ? buildListBlocks(days)
        : buildBlocks(days, { width: Math.max(240, measureW), perRow: size, gap: TRASH_GAP }),
    [days, measureW, size, mode],
  );

  /**
   * 虚拟化 —— 「滚动不卡顿」的全部依据。
   *
   * 三千张缩略图全挂在 DOM 上时，滚动会一路解码图片、一路重算样式。这里只渲染视口
   * 附近的十来块。**块高精确可算**（`blockHeight`，与 CSS 里的定高是一对），
   * 故不需要 `measureElement` 逐块回测 —— 那既是滚动时的强制同步布局，
   * 也会在测出来之前让相邻两块叠在一起。
   */
  const virt = useVirtualizer({
    count: blocks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => {
      const b = blocks[i];
      return b ? blockHeight(b, mode) : TRASH_LIST_ROW_H;
    },
    overscan: 6,
  });

  const curItem = flat[focus] ?? null;
  const zoomItem = zoom === null ? null : (flat[zoom] ?? null);

  // 条目少掉之后（还原/删除）把光标收回界内 —— 落空时详情栏会莫名其妙地空掉。
  useEffect(() => {
    if (focus >= flat.length) setFocus(Math.max(0, flat.length - 1));
  }, [flat.length, focus]);
  useEffect(() => {
    if (zoom !== null && zoom >= flat.length) setZoom(flat.length > 0 ? flat.length - 1 : null);
  }, [flat.length, zoom]);

  const purge = useCallback(
    async (ids: number[], all: boolean) => {
      try {
        const n = all
          ? await unwrap(commands.purgeAllTrash())
          : await unwrap(commands.purgeTrashItems(ids));
        toast(`已彻底删除 ${n} 项 · 编号已回收`);
        await load();
      } catch (e) {
        if (e instanceof Error) toast.error(e.message);
      }
    },
    [load],
  );

  /**
   * 还原回原位。
   *
   * 这里之所以能做到，是因为「不通过」从来只是记账：原图与缩略图一直躺在盘上，
   * 物理删要等到「彻底删除/清空」（E02 的决定）。故还原不动任何文件，
   * 只把那条记录的状态拨回去 —— 未通过的图回待验收，删掉的作品写回作品库。
   */
  const restore = useCallback(
    async (ids: number[]) => {
      if (ids.length === 0) return;
      try {
        const res = await unwrap(commands.restoreTrashItems(ids));
        if (res.restored > 0) toast.success(`已还原 ${res.restored} 项回原位`);
        // 失败逐条报出来：「点了还原却没回去」比直接说还不回去更难查。
        for (const f of res.failures) toast.error(f);
        await load();
      } catch (e) {
        if (e instanceof Error) toast.error(e.message);
      }
    },
    [load],
  );

  /**
   * 点一格 —— 平点/⌘点都是「切换这一格的勾选」，⇧ 点是「从锚点选到这里」。
   *
   * 与视频工作台不同（那里平点只移光标、不动勾选）：那一页的行是**动作的对象**，
   * 光标与勾选是两套作用域；这一页更像相册，点一张图就是挑中它，
   * 而光标只是键盘的落点，跟着点走没有歧义。
   */
  const pick = useCallback(
    (idx: number, pickMode: "set" | "toggle" | "range") => {
      const it = flat[idx];
      if (!it) return;
      setFocus(idx);
      if (pickMode === "range" && anchor.current !== null) {
        const a = Math.min(anchor.current, idx);
        const b = Math.max(anchor.current, idx);
        setSel((s) => {
          const n = new Set(s);
          for (let i = a; i <= b; i += 1) {
            const x = flat[i];
            if (x) n.add(x.id);
          }
          return n;
        });
        return;
      }
      anchor.current = idx;
      setSel((s) => {
        const n = new Set(s);
        if (n.has(it.id)) n.delete(it.id);
        else n.add(it.id);
        return n;
      });
    },
    [flat],
  );

  /** 整簇选中 / 取消 —— 「那一批整个不要了」是这一页最常见的批量动作。 */
  const pickCluster = useCallback((ids: number[]) => {
    setSel((s) => {
      const n = new Set(s);
      const allIn = ids.every((id) => n.has(id));
      for (const id of ids) {
        if (allIn) n.delete(id);
        else n.add(id);
      }
      return n;
    });
  }, []);

  const onZoom = useCallback((idx: number) => setZoom(idx), []);
  const tileHandlers: TileHandlers = useMemo(() => ({ onPick: pick, onZoom }), [pick, onZoom]);

  /** 作用域：勾了就作用于勾选的，没勾就作用于光标这一条。 */
  const scope = useCallback(
    (): number[] => (sel.size > 0 ? [...sel] : curItem ? [curItem.id] : []),
    [sel, curItem],
  );

  // ── 键盘 ─────────────────────────────────────────────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el?.closest("input, textarea, select, [contenteditable=true]")) return;
      // 速查面板或确认卡开着时整页让路：那时按 R 是想读那一行说明、按 ⌫ 是想改主意，
      // 而这一页的 R 会当场还原一批东西。
      if (confirm || useUiStore.getState().helpOpen) return;
      if ((e.metaKey || e.ctrlKey) && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        setSel((old) => {
          const allIn = flat.length > 0 && flat.every((r) => old.has(r.id));
          return allIn ? new Set<number>() : new Set(flat.map((r) => r.id));
        });
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if (e.key === "Escape") {
        if (zoom !== null) setZoom(null);
        else if (sel.size > 0) setSel(new Set());
        return;
      }
      // 放大时左右是换条，网格里左右是移光标 —— 两者共用同一个序列，故退出放大后
      // 光标就停在刚才看的那一条上。
      if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
        e.preventDefault();
        const d = e.key === "ArrowRight" ? 1 : -1;
        if (zoom !== null) setZoom((z) => clamp((z ?? 0) + d, flat.length));
        else setFocus((f) => clamp(f + d, flat.length));
        return;
      }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const d = e.key === "ArrowDown" ? 1 : -1;
        // 列表里一行就是一条，上下就是前后一条。网格里不能这么算 —— 齐行下每行张数
        // 是变的（一行竖幅塞 5 张、一行横幅只塞 2 张），照搬「±列数」会跳得毫无规律，
        // 故走行模型。两种布局共用同一个 `flat`，所以换布局时光标停在原处。
        if (zoom === null) {
          setFocus((f) =>
            mode === "list" ? clamp(f + d, flat.length) : moveByRow(blocks, cardRow, f, d),
          );
        }
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        if (zoom === null && curItem) setZoom(focus);
        return;
      }
      if (e.key === " ") {
        e.preventDefault();
        if (zoom === null) pick(focus, "toggle");
        return;
      }
      if (e.key === "r" || e.key === "R") {
        const ids = zoom !== null && zoomItem ? [zoomItem.id] : scope();
        void restore(ids);
        return;
      }
      if (e.key === "Backspace" || e.key === "Delete") {
        e.preventDefault();
        const ids = zoom !== null && zoomItem ? [zoomItem.id] : scope();
        if (ids.length > 0) setConfirm({ ids, all: false });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    blocks,
    cardRow,
    flat,
    focus,
    zoom,
    zoomItem,
    curItem,
    sel,
    confirm,
    mode,
    pick,
    restore,
    scope,
  ]);

  // 光标滚进视野走**虚拟化的定位**，不是 `scrollIntoView`：后者要求那个元素此刻在
  // DOM 里，而虚拟化下视口外的块根本没渲染 —— 光标一旦跳出 overscan（⌘A 之后按方向键、
  // 或删掉一批之后光标被收回末尾），那一下就会什么都不发生。
  // biome-ignore lint/correctness/useExhaustiveDependencies: virt 实例稳定，仅在焦点/块模型变化时滚动
  useEffect(() => {
    const at = cardRow[focus];
    if (at !== undefined) virt.scrollToIndex(at, { align: "auto" });
  }, [focus, cardRow]);

  const selCount = sel.size;
  const restorableSel = useMemo(
    () => flat.filter((r) => sel.has(r.id) && r.restorable).length,
    [flat, sel],
  );

  return (
    <div className="trwb" data-screen-label="废纸篓">
      <div className="phd">
        <span className="ptt">废纸篓</span>
        <span className="pcap">清理前可还原回原位 · 清理后彻底删除、编号回收、不可恢复</span>
        <div className="f1" />
        <span className="cnt">{rows.length} 项</span>
        <div className="seg">
          <span
            className={cn("sgi", mode === "grid" && "on")}
            title="网格：按真实比例铺开，一屏看出那批图长什么样"
            onClick={() => setMode("grid")}
          >
            <LayoutGrid className="ic12" />
            网格
          </span>
          <span
            className={cn("sgi", mode === "list" && "on")}
            title="列表：一行一条，找编号最快"
            onClick={() => setMode("list")}
          >
            <List className="ic12" />
            列表
          </span>
        </div>
        {mode === "grid" && (
          <>
            {/* 齐行下每行张数随比例浮动，故这里给的是**大小**而不是精确列数。 */}
            <span className="fs11 t3 nowrap" title="图片显示大小（每行放几张的目标值）">
              大小
            </span>
            <input
              type="range"
              min={TRASH_SIZE_MIN}
              max={TRASH_SIZE_MAX}
              value={size}
              className="rng"
              onChange={(e) => setSize(Number(e.target.value))}
              // 拖完失焦，否则方向键继续改滑块而不是移光标。
              onMouseUp={(e) => e.currentTarget.blur()}
              onKeyUp={(e) => e.currentTarget.blur()}
            />
          </>
        )}
      </div>

      {rows.length === 0 ? (
        <div className="bigempty">
          <Trash2 className="ic" style={{ width: 26, height: 26, opacity: 0.5 }} />
          <div className="fs13 fw5 t2">废纸篓是空的</div>
          <div className="fs12 t3">验收未通过与手动删除的内容会先进入这里等待清理</div>
        </div>
      ) : (
        <div className="trwbrow">
          <div className="trmain" ref={scrollRef}>
            <div className="trvirt" style={{ height: virt.getTotalSize() }}>
              {virt.getVirtualItems().map((v) => {
                const b = blocks[v.index];
                if (!b) return null;
                const kindCls = b.kind === "head" ? (b.head.kind === "day" ? "day" : "clu") : "row";
                return (
                  <div
                    key={b.key}
                    className={`trblk ${kindCls}`}
                    style={{ transform: `translateY(${v.start}px)` }}
                  >
                    {b.kind === "head" ? (
                      <TrashHeadRow head={b.head} sel={sel} onPickCluster={pickCluster} />
                    ) : mode === "list" ? (
                      b.items.map(({ it, idx }) => (
                        <TrashListRow
                          key={it.id}
                          it={it}
                          idx={idx}
                          selected={sel.has(it.id)}
                          focused={idx === focus}
                          handlers={tileHandlers}
                        />
                      ))
                    ) : (
                      <TrashCardsRow
                        items={b.items}
                        h={b.h}
                        gap={TRASH_GAP}
                        sel={sel}
                        focus={focus}
                        handlers={tileHandlers}
                      />
                    )}
                  </div>
                );
              })}
            </div>
          </div>
          <TrashSide
            item={curItem}
            now={now}
            onZoom={() => setZoom(focus)}
            onRestore={() => curItem && void restore([curItem.id])}
            onPurge={() => curItem && setConfirm({ ids: [curItem.id], all: false })}
          />
        </div>
      )}

      {rows.length > 0 && (
        <div className="trdock">
          <span className="gl">这一屏</span>
          <span className="fs11 t3 nowrap">
            {selCount > 0 ? `已选 ${selCount} 项` : "⇧ 选一段 · ⌘ 点加选 · ⌘A 全选"}
          </span>
          {selCount > 0 && (
            <>
              <button
                type="button"
                className="btn sm"
                // 数字写的是**还原得回去的那几条**，不是勾选总数：旧版本删掉的作品还不回去，
                // 而按钮上写着「还原所选 · 12」却只回来 9 条，是这一栏最不能犯的错。
                title={
                  restorableSel === selCount
                    ? undefined
                    : `勾选的 ${selCount} 项里有 ${selCount - restorableSel} 条是旧版本删除的，没留下可还原的记录`
                }
                onClick={() => void restore([...sel])}
              >
                <Undo2 className="ic12" />
                还原所选 · {restorableSel}
              </button>
              <button
                type="button"
                className="btn sm gho dng"
                onClick={() => setConfirm({ ids: [...sel], all: false })}
              >
                <Trash2 className="ic12" />
                彻底删除所选 · {selCount}
              </button>
              <button type="button" className="btn sm gho" onClick={() => setSel(new Set())}>
                清除勾选
              </button>
            </>
          )}
          <div className="f1" />
          <button
            type="button"
            className="btn sm gho dng"
            onClick={() => setConfirm({ ids: [], all: true })}
          >
            清空废纸篓 · {rows.length}
          </button>
        </div>
      )}

      {zoomItem && zoom !== null && (
        <TrashLightbox
          item={zoomItem}
          index={zoom}
          total={flat.length}
          now={now}
          onSeek={(d) => setZoom((z) => clamp((z ?? 0) + d, flat.length))}
          onRestore={() => void restore([zoomItem.id])}
          onPurge={() => setConfirm({ ids: [zoomItem.id], all: false })}
          onClose={() => {
            // 退出时把光标停在刚才看的那一条上 —— 否则接着按方向键会从头跳一次。
            setFocus(zoom);
            setZoom(null);
          }}
        />
      )}

      {confirm && (
        <ConfirmModal
          title={
            confirm.all ? `清空废纸篓 · ${rows.length} 项` : `彻底删除 ${confirm.ids.length} 项`
          }
          desc="将物理删除文件（含未通过原图）、级联清除记录并回收编号，此操作不可恢复。"
          confirmLabel="彻底删除"
          danger
          onConfirm={() => void purge(confirm.ids, confirm.all)}
          onClose={() => setConfirm(null)}
        />
      )}
    </div>
  );
}

/** 夹取到 [0, n-1]；空列表回 0。 */
function clamp(i: number, n: number): number {
  return Math.max(0, Math.min(n - 1, i));
}
