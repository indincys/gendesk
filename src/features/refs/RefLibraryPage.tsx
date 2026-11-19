import { ConfirmModal, Modal } from "@/components/ui/Modal";
import {
  MediaActions,
  OriginalPreview,
  copyText,
  exportOriginals,
} from "@/features/_shared/MediaActions";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { RefImportOverlay, type RefImportState } from "@/features/_shared/RefImport";
import { assetSrc } from "@/lib/img";
import {
  type LibraryCategoryView,
  type LibraryImagePatch,
  type LibraryImageView,
  type LibraryLimits,
  type LibraryOverview,
  type LibraryPickResult,
  type LibraryStyleView,
  commands,
  subscribeFileDrop,
  subscribeLibrary,
  subscribeRefImportProgress,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Dices, ImagePlus, Pencil, Plus, Settings2 } from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { LibraryImportDialog, LibraryUsageDialog, RelinkDirectoryDialog } from "./LibraryDialogs";

const NO_PATCH: LibraryImagePatch = { categoryId: null, enabled: null, note: null, removed: null };

/*
 * 网格几何。与 globals.css 里 `.rlrow` / `.rlcard` / `.rlmeta` / `.rlsub` 是一对，改一处必须改另一处：
 * 虚拟列表按这里算出的行高摆位，不逐行回测（回测是滚动时的强制同步布局，见 CLAUDE.md「图片网格」②）。
 */
/** 卡片最小列宽：每行放得下几张由它和容器宽度决定。 */
const CARD_MIN_W = 150;
/** 列间距与行间距。 */
const GRID_GAP = 14;
/** 网格左右内边距（`.rlrow` 的 padding）。 */
const GRID_PAD_X = 18;
const GRID_PAD_TOP = 16;
const GRID_PAD_BOTTOM = 40;
/** 方图下方的文字区：卡片 gap 5 + 徽章行 20 + gap 5 + 用量行 14。 */
const CARD_META_H = 44;
/** 外部改动（CLI 导入、工单抽图）合并重读的窗口：一次投单可能连发几声。 */
const EXTERNAL_REFRESH_MS = 300;

/** 「11-14」这种短日期。 */
function shortDate(unix: number): string {
  const d = new Date(unix * 1000);
  return `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** 卡片上的状态徽章：已移出 / 原图丢失 / 停用 / 冷却到哪天 / 可用。 */
function statusBadge(img: LibraryImageView): { text: string; tone: string } {
  if (img.removed) return { text: "已移出", tone: "b-gray" };
  // 图库映射本地原图：原图被挪走、删掉或所在的盘没接上，这张就抽不到了。
  if (img.missing) return { text: "原图丢失", tone: "b-red" };
  if (!img.enabled) return { text: "停用", tone: "b-gray" };
  if (img.coolingUntil && img.coolingUntil > Date.now() / 1000)
    return { text: `冷却至 ${shortDate(img.coolingUntil)}`, tone: "b-amber" };
  return { text: "可用", tone: "b-green" };
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 参考图库：按款式、分类长期保存参考图。工单清单里写 `{"category":"正面图"}`，
 * 收录时由应用随机抽图，冷却期内用过的图优先跳过（冷却天数按分类设）。
 */
export function RefLibraryPage() {
  const [overview, setOverview] = useState<LibraryOverview | null>(null);
  const [styleId, setStyleId] = useState<number | null>(null);
  const [categoryId, setCategoryId] = useState<number | null>(null);
  const [showRemoved, setShowRemoved] = useState(false);
  const [styleQuery, setStyleQuery] = useState("");
  const [productFilter, setProductFilter] = useState("");
  const [viewArchived, setViewArchived] = useState(false);
  const [failedImport, setFailedImport] = useState<{
    category: number;
    paths: string[];
    report: string;
  } | null>(null);
  const [usageImage, setUsageImage] = useState<LibraryImageView | null>(null);
  const [showRelink, setShowRelink] = useState<LibraryImageView[] | null>(null);
  const [pickResult, setPickResult] = useState<LibraryPickResult | null>(null);
  const [picking, setPicking] = useState(false);
  const [imageQuery, setImageQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [preview, setPreview] = useState<{ path: string; name: string } | null>(null);
  const anchor = useRef<number | null>(null);
  const [images, setImages] = useState<LibraryImageView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [styleEdit, setStyleEdit] = useState<number | "new" | null>(null);
  const [deletedStylesOpen, setDeletedStylesOpen] = useState(false);
  const [restoringStyleId, setRestoringStyleId] = useState<number | null>(null);
  const [catsOpen, setCatsOpen] = useState(false);
  const [importAsk, setImportAsk] = useState<string[] | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [pickCount, setPickCount] = useState(3);
  const [importing, setImporting] = useState<RefImportState | null>(null);
  const busy = useRef(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  /** 只认最后一次发出的读图请求：快速切款式时，先发后到的旧结果不许盖掉新的。 */
  const imagesSeq = useRef(0);

  useEffect(() => {
    if (styleId === null) return;
    let active = true;
    const subscription = subscribeFileDrop((paths) => {
      if (!active || busy.current) return;
      if (paths.length) setImportAsk(paths);
    });
    return () => {
      active = false;
      void subscription.then((unsub) => unsub());
    };
  }, [styleId]);

  const refreshOverview = useCallback(async () => {
    try {
      const o = await unwrap(commands.libraryOverview());
      setOverview(o);
      setStyleId((cur) =>
        cur !== null && o.styles.some((s) => s.id === cur)
          ? cur
          : (o.styles.find((item) => !item.archived)?.id ?? null),
      );
    } catch (e) {
      toast.error(`读取参考图库失败：${message(e)}`);
    }
  }, []);

  const refreshImages = useCallback(async () => {
    const seq = ++imagesSeq.current;
    if (styleId === null) {
      setImages([]);
      return;
    }
    try {
      const rows = await unwrap(commands.listLibraryImages(styleId, categoryId, showRemoved));
      if (seq === imagesSeq.current) setImages(rows);
    } catch (e) {
      if (seq === imagesSeq.current) toast.error(`读取图片失败：${message(e)}`);
    }
  }, [styleId, categoryId, showRemoved]);

  const refresh = useCallback(async () => {
    await Promise.all([refreshOverview(), refreshImages()]);
  }, [refreshOverview, refreshImages]);

  useEffect(() => {
    void refreshOverview();
  }, [refreshOverview]);
  useEffect(() => {
    setSel(new Set());
    void refreshImages();
  }, [refreshImages]);

  // 应用界面之外的改动（Agent 经 CLI 导入/改图、工单收录抽中图）由后端推一声，这里合并后重读。
  // 用 ref 取最新的 refresh，订阅只建一次，不随筛选条件反复挂退。
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unsub: (() => void) | undefined;
    let alive = true;
    void subscribeLibrary(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        // 自己正在导入时不插队：导入结束的 finally 里会统一重读。
        if (!busy.current) void refreshRef.current();
      }, EXTERNAL_REFRESH_MS);
    }).then((un) => {
      if (alive) unsub = un;
      else un();
    });
    return () => {
      alive = false;
      clearTimeout(timer);
      unsub?.();
    };
  }, []);

  // 冷却到期不会有任何事件：在最早一张到期的那一刻重读一次，「可用」计数与徽章随之更新。
  const nextCoolEnd = useMemo(() => {
    let min = Number.POSITIVE_INFINITY;
    for (const i of images) if (i.coolingUntil && i.coolingUntil < min) min = i.coolingUntil;
    return min;
  }, [images]);
  useEffect(() => {
    if (!Number.isFinite(nextCoolEnd)) return;
    const ms = Math.max(1000, nextCoolEnd * 1000 - Date.now() + 1000);
    // setTimeout 上限约 24.8 天；更远的到期等下次重读时再排。
    if (ms > 2 ** 31 - 1) return;
    const t = setTimeout(() => void refreshRef.current(), ms);
    return () => clearTimeout(t);
  }, [nextCoolEnd]);

  const style = overview?.styles.find((s) => s.id === styleId) ?? null;
  const editedStyle = overview?.styles.find((item) => item.id === styleEdit) ?? null;
  const deletedStyles = overview?.deletedStyles ?? [];
  const categories = overview?.categories ?? [];
  const limits = overview?.limits ?? null;
  const category = categories.find((c) => c.id === categoryId) ?? null;
  const countOf = (cid: number) => style?.byCategory.find((c) => c.categoryId === cid);
  const selected = useMemo(() => images.filter((i) => sel.has(i.id)), [images, sel]);
  const visibleImages = useMemo(
    () =>
      images.filter((item) => {
        const query = imageQuery.trim().toLowerCase();
        return (
          (!query ||
            [item.name, item.filePath, item.note, `#${item.id}`].some((value) =>
              value.toLowerCase().includes(query),
            )) &&
          (status === "all" ||
            (status === "missing" && item.missing) ||
            (status === "removed" && item.removed) ||
            (status === "disabled" && !item.enabled && !item.removed) ||
            (status === "cooling" && !!item.coolingUntil && !item.removed))
        );
      }),
    [images, imageQuery, status],
  );
  const previewImage = useCallback(async (item: LibraryImageView) => {
    try {
      const path = await unwrap(commands.previewLibraryOriginal(item.id));
      setPreview({ path, name: item.name });
    } catch (error) {
      toast.error(message(error));
    }
  }, []);

  // 稳定引用：卡片是 memo 的，点一张只重绘这一张，而不是整屏。
  const toggle = useCallback(
    (id: number, shift: boolean) => {
      const from = shift ? visibleImages.findIndex((item) => item.id === anchor.current) : -1;
      const to = visibleImages.findIndex((item) => item.id === id);
      anchor.current = id;
      setSel((current) => {
        const next = new Set(current);
        if (from >= 0 && to >= 0) {
          for (const item of visibleImages.slice(Math.min(from, to), Math.max(from, to) + 1))
            next.add(item.id);
        } else if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      });
    },
    [visibleImages],
  );

  /*
   * 虚拟化 —— 上万张时「滚动不卡顿」的依据。全量挂 DOM 时，每张卡都要解码缩略图、参与样式
   * 计算，点一下选中还会整屏重绘。这里按行虚拟，只渲染视口附近的几行；行高精确可算。
   * 容器宽度只取 ResizeObserver 的 contentRect（见 CLAUDE.md「图片网格」③）。
   */
  const [gridW, setGridW] = useState(0);
  const hasImages = visibleImages.length > 0;
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || !hasImages) return;
    const ro = new ResizeObserver(([entry]) => {
      const w = entry?.contentRect.width ?? 0;
      if (w) setGridW((cur) => (Math.abs(w - cur) > 1 ? w : cur));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [hasImages]);
  const inner = Math.max(CARD_MIN_W, gridW - GRID_PAD_X * 2);
  const cols = Math.max(1, Math.floor((inner + GRID_GAP) / (CARD_MIN_W + GRID_GAP)));
  const colW = (inner - GRID_GAP * (cols - 1)) / cols;
  const rowH = colW + CARD_META_H + GRID_GAP;
  const rowCount = gridW > 0 ? Math.ceil(visibleImages.length / cols) : 0;
  const virt = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowH,
    paddingStart: GRID_PAD_TOP,
    paddingEnd: GRID_PAD_BOTTOM - GRID_GAP,
    overscan: 3,
  });
  // 列数或列宽一变，所有行的偏移都要重算。
  // biome-ignore lint/correctness/useExhaustiveDependencies: rowH/cols 是重算的触发条件
  useLayoutEffect(() => {
    virt.measure();
  }, [rowH, cols]);
  // 换款式/分类就是换了一份列表：回到顶部，别停在上一份的第 3000 张那里。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 只在筛选条件变化时回顶
  useLayoutEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
    anchor.current = null;
  }, [styleId, categoryId, showRemoved, imageQuery, status]);

  async function runImport(paths: string[], cid: number) {
    if (styleId === null || busy.current) return;
    busy.current = true;
    setImporting({ done: 0, total: paths.length, name: "", failed: 0 });
    const unsub = await subscribeRefImportProgress((p) =>
      setImporting({ done: p.done, total: p.total, name: p.name, failed: p.failed }),
    );
    try {
      const r = await unwrap(commands.importLibraryImages(styleId, cid, paths));
      const parts = [`导入 ${r.imported.length} 张`];
      if (r.relinked.length) parts.push(`${r.relinked.length} 张旧副本改为映射原图`);
      if (r.restored.length) parts.push(`放回 ${r.restored.length} 张`);
      if (r.duplicates.length) parts.push(`${r.duplicates.length} 张已在库里`);
      if (r.failed.length) parts.push(`${r.failed.length} 张读不了`);
      if (r.failed.length) {
        const report = r.failed.map((item) => `${item.path} · ${item.message}`).join("\n");
        setFailedImport({ category: cid, paths: r.failed.map((item) => item.path), report });
        toast.warning(parts.join("，"), {
          action: {
            label: "复制失败清单",
            onClick: () => void copyText(report, "失败清单"),
          },
        });
      } else {
        setFailedImport(null);
        toast.success(parts.join("，"));
      }
      setCategoryId(cid);
    } catch (e) {
      toast.error(`导入失败：${message(e)}`);
    } finally {
      unsub();
      setImporting(null);
      busy.current = false;
      await refresh();
    }
  }

  async function startImport() {
    if (!style) {
      toast.info("先新建一个款式");
      return;
    }
    const paths = await unwrap(commands.pickImageFiles()).catch((error) => {
      toast.error(message(error));
      return [];
    });
    if (paths.length) setImportAsk(paths);
  }

  async function patch(p: Partial<LibraryImagePatch>, done: string) {
    try {
      const n = await unwrap(commands.updateLibraryImages([...sel], { ...NO_PATCH, ...p }));
      const changed = selected.filter((item) => p.removed !== item.removed).map((item) => item.id);
      toast.success(`${done} ${n}/${sel.size} 张`, {
        action:
          p.removed !== undefined && changed.length === n
            ? {
                label: "撤销",
                onClick: () => {
                  void unwrap(
                    commands.updateLibraryImages(changed, { ...NO_PATCH, removed: !p.removed }),
                  )
                    .then(async (count) => {
                      toast.success(`已撤销 ${count} 张`);
                      await refreshRef.current();
                    })
                    .catch((error) => toast.error(message(error)));
                },
              }
            : undefined,
      });
      if (n === sel.size) setSel(new Set());
      await refresh();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function tryPick() {
    if (!style || !category || picking) return;
    setPicking(true);
    try {
      setPickResult(
        await unwrap(commands.pickLibraryImages(style.id, category.id, pickCount, false)),
      );
    } catch (e) {
      toast.error(message(e));
    } finally {
      setPicking(false);
    }
  }

  const addToGeneration = useCallback(async (items: LibraryImageView[]) => {
    const results = await Promise.allSettled(
      items.map((item) => unwrap(commands.generationLibraryRef(item.id))),
    );
    const ids = results.flatMap((result) => (result.status === "fulfilled" ? [result.value] : []));
    if (ids.length) {
      useGenerateStore.getState().setSelRefIds((current) => [...new Set([...current, ...ids])]);
      useUiStore.getState().go("generate");
    }
    if (ids.length < items.length) {
      const reasons = results.flatMap((result, index) =>
        result.status === "rejected" ? [`#${items[index]?.id} · ${message(result.reason)}`] : [],
      );
      toast.warning(`已加入 ${ids.length}/${items.length} 张`, {
        description: reasons.slice(0, 4).join("\n"),
      });
    } else toast.success(`已加入本次生成 · ${ids.length} 张`);
  }, []);

  async function restoreStyle(item: LibraryStyleView) {
    setRestoringStyleId(item.id);
    try {
      await unwrap(commands.restoreLibraryStyle(item.id));
      if (item.archived) setViewArchived(true);
      setStyleId(item.id);
      setCategoryId(null);
      setImageQuery("");
      setStatus("all");
      setStyleQuery("");
      setProductFilter("");
      setFailedImport(null);
      setDeletedStylesOpen(false);
      await refreshOverview();
      toast.success(`已恢复「${item.name}」及其 ${item.total} 张在库图片`);
    } catch (error) {
      toast.error(message(error));
      await refreshOverview();
    } finally {
      setRestoringStyleId(null);
    }
  }

  async function deleteStyle(item: LibraryStyleView) {
    try {
      await unwrap(commands.deleteLibraryStyle(item.id, item.total + item.removedCount));
      setStyleEdit(null);
      if (item.id === styleId) {
        setStyleId(null);
        setSel(new Set());
        setImages([]);
        setCategoryId(null);
        setFailedImport(null);
      }
      await refreshOverview();
      toast.success(`已删除「${item.name}」及其 ${item.total} 张在库图片`, {
        action: { label: "撤销", onClick: () => void restoreStyle(item) },
      });
    } catch (error) {
      toast.error(message(error));
      await refreshOverview();
    }
  }

  const right = (
    <>
      <button type="button" className="btn sm gho" onClick={() => setCatsOpen(true)}>
        <Settings2 className="ic12" />
        分类与冷却
      </button>
      <button type="button" className="btn sm" onClick={() => setStyleEdit("new")}>
        <Plus className="ic12" />
        新建款式
      </button>
      <button
        type="button"
        className="btn sm pri"
        disabled={!style || style.archived}
        onClick={startImport}
      >
        <ImagePlus className="ic12" />
        导入图片
      </button>
    </>
  );

  return (
    <PageScaffold
      title="参考图库"
      caption="映射本地原图（不另存副本）· 工单按款式和分类随机抽图，冷却期内用过的图优先跳过"
      right={right}
      bodyClassName="rlwb"
    >
      <div className="rlside">
        {overview && overview.styles.length === 0 && (
          <div className="fs12 t3 rlhint">还没有款式。点「新建款式」，再导入它的参考图。</div>
        )}
        <input
          type="search"
          className="inp sm"
          aria-label="搜索款式"
          placeholder="搜款式／产品"
          value={styleQuery}
          onChange={(event) => setStyleQuery(event.target.value)}
        />
        <select
          className="inp sm"
          aria-label="按产品筛选款式"
          value={productFilter}
          onChange={(event) => setProductFilter(event.target.value)}
        >
          <option value="">所有产品</option>
          {[...new Set(overview?.styles.map((item) => item.product).filter(Boolean))]
            .sort()
            .map((product) => (
              <option key={product} value={product}>
                {product}
              </option>
            ))}
        </select>
        <label className="fx ac gap6 fs11 t3">
          <input
            type="checkbox"
            checked={viewArchived}
            onChange={(event) => {
              setViewArchived(event.target.checked);
              if (!event.target.checked && style?.archived)
                setStyleId(overview?.styles.find((item) => !item.archived)?.id ?? null);
            }}
          />
          显示归档款式
        </label>
        <button type="button" className="btn sm gho" onClick={() => setDeletedStylesOpen(true)}>
          已删除款式{deletedStyles.length > 0 && ` · ${deletedStyles.length}`}
        </button>
        {overview?.styles
          .filter(
            (item) =>
              (viewArchived || !item.archived) &&
              (!productFilter || item.product === productFilter) &&
              `${item.name} ${item.product}`
                .toLowerCase()
                .includes(styleQuery.trim().toLowerCase()),
          )
          .map((s) => (
            <div
              key={s.id}
              className={cn("rlstyle", s.id === styleId && "on")}
              onClick={() => {
                setStyleId(s.id);
                setFailedImport(null);
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                setStyleEdit(s.id);
              }}
              onKeyDown={(e) => {
                if (e.target !== e.currentTarget) return;
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setStyleId(s.id);
                  setFailedImport(null);
                }
                if (e.key === "F2" || e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
                  e.preventDefault();
                  setStyleEdit(s.id);
                }
              }}
              role="button"
              tabIndex={0}
            >
              <div className="f1 ohide">
                <div className="fs12 fw5 nowrap ohide">
                  {s.name}
                  {s.archived ? " · 已归档" : ""}
                </div>
                {s.product && <div className="fs10 t3 nowrap ohide">{s.product}</div>}
              </div>
              <span className="fs11 t3 mono nowrap" title="可用 / 总数">
                {s.available}/{s.total}
              </span>
              <button
                type="button"
                className="icb rledit"
                aria-label={`${s.name}款式操作`}
                title="款式操作"
                onClick={(e) => {
                  e.stopPropagation();
                  setStyleEdit(s.id);
                }}
              >
                <Pencil className="ic12" />
              </button>
            </div>
          ))}
      </div>

      <div className="rlmain">
        <div className="fbar">
          <div className="seg">
            <button
              type="button"
              aria-pressed={categoryId === null}
              className={cn("sgi", categoryId === null && "on")}
              onClick={() => setCategoryId(null)}
            >
              全部 <span className="t3 mono">{style?.total ?? 0}</span>
            </button>
            {categories.map((c) => {
              const n = countOf(c.id);
              return (
                <button
                  type="button"
                  key={c.id}
                  aria-pressed={categoryId === c.id}
                  className={cn("sgi", categoryId === c.id && "on")}
                  onClick={() => setCategoryId(c.id)}
                  title={`可用 ${n?.available ?? 0} 张 · 冷却 ${c.cooldownDays} 天`}
                >
                  {c.name}{" "}
                  <span className="t3 mono">
                    {n?.available ?? 0}/{n?.total ?? 0}
                  </span>
                </button>
              );
            })}
          </div>
          <div className="f1" />
          <label className="fx ac gap6 fs12 t2">
            <input
              type="checkbox"
              checked={showRemoved}
              onChange={(e) => setShowRemoved(e.target.checked)}
            />
            显示已移出
          </label>
          {category && !style?.archived && (
            <span className="fx ac gap6">
              <input
                className="inp sm rlnum"
                type="number"
                aria-label="试抽图片数量"
                min={1}
                max={limits?.maxPick}
                value={pickCount}
                onChange={(e) =>
                  setPickCount(
                    Math.max(1, Math.min(limits?.maxPick ?? 1, Number(e.target.value) || 1)),
                  )
                }
              />
              <button
                type="button"
                className="btn sm"
                onClick={tryPick}
                disabled={picking}
                title="按工单规则随机抽，不记为使用"
              >
                <Dices className="ic12" />
                试抽
              </button>
            </span>
          )}
        </div>

        {style && (
          <div className="fbar">
            <input
              type="search"
              className="inp sm"
              placeholder="搜文件名、路径或编号"
              aria-label="搜索参考图"
              value={imageQuery}
              onChange={(event) => {
                setImageQuery(event.target.value);
                setSel(new Set());
              }}
            />
            <select
              className="inp sm"
              aria-label="参考图状态"
              value={status}
              onChange={(event) => {
                setStatus(event.target.value);
                if (event.target.value === "removed") setShowRemoved(true);
                setSel(new Set());
              }}
            >
              <option value="all">全部状态</option>
              <option value="missing">原图丢失</option>
              <option value="cooling">冷却中</option>
              <option value="disabled">已停用</option>
              <option value="removed">已移出</option>
            </select>
            <button type="button" className="btn xs" onClick={() => void refresh()}>
              重新读取
            </button>
            <button
              type="button"
              className="btn xs"
              disabled={visibleImages.length === 0}
              onClick={() => setSel(new Set(visibleImages.map((item) => item.id)))}
            >
              选择筛选结果 · {visibleImages.length} 张
            </button>
          </div>
        )}

        {failedImport && (
          <div className="fbar">
            <span className="fs12 t2">{failedImport.paths.length} 张图片未导入</span>
            <button
              type="button"
              className="btn xs"
              onClick={() => void copyText(failedImport.report, "失败清单")}
            >
              复制失败清单
            </button>
            <button
              type="button"
              className="btn xs"
              onClick={() => void runImport(failedImport.paths, failedImport.category)}
            >
              仅重试失败项
            </button>
            <button type="button" className="btn xs gho" onClick={() => setFailedImport(null)}>
              关闭反馈
            </button>
          </div>
        )}
        {sel.size > 0 && (
          <div className="rlbar">
            <span className="fs12 fw5">已选 {sel.size} 张</span>
            <button
              type="button"
              className="btn sm"
              onClick={() => void exportOriginals(selected.map((item) => item.filePath))}
            >
              导出原件
            </button>
            <button type="button" className="btn sm" onClick={() => void addToGeneration(selected)}>
              加入本次生成
            </button>
            {selected.some((item) => item.missing && item.linked) && (
              <button
                type="button"
                className="btn sm"
                onClick={() =>
                  setShowRelink(selected.filter((item) => item.missing && item.linked))
                }
              >
                批量替换目录前缀…
              </button>
            )}
            {selected.length === 1 && selected[0]?.missing && selected[0].linked && (
              <button
                type="button"
                className="btn sm"
                onClick={async () => {
                  const item = selected[0];
                  if (!item) return;
                  try {
                    const paths = await unwrap(commands.pickImageFiles());
                    if (paths.length === 0) return;
                    if (paths.length !== 1) {
                      toast.error("请选择一个原图文件");
                      return;
                    }
                    await unwrap(commands.relinkMissingLibraryImage(item.id, paths[0] ?? ""));
                    toast.success("已重新定位原图");
                    await refresh();
                  } catch (error) {
                    toast.error(message(error));
                  }
                }}
              >
                重新定位原文件
              </button>
            )}
            <select
              aria-label="将所选参考图移到分类"
              className="inp sm"
              value=""
              onChange={(e) => {
                const cid = Number(e.target.value);
                if (cid) void patch({ categoryId: cid }, "已改分类");
              }}
            >
              <option value="">移到分类…</option>
              {categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="btn sm"
              onClick={() => patch({ enabled: false }, "已停用")}
            >
              停用
            </button>
            <button
              type="button"
              className="btn sm"
              onClick={() => patch({ enabled: true }, "已启用")}
            >
              启用
            </button>
            {selected.some((i) => i.removed) && (
              <button
                type="button"
                className="btn sm"
                onClick={() => patch({ removed: false }, "已放回")}
              >
                放回图库
              </button>
            )}
            {selected.some((i) => !i.removed) && (
              <button
                type="button"
                className="btn sm gho dng"
                onClick={() => setConfirmRemove(true)}
              >
                移出图库
              </button>
            )}
            <div className="f1" />
            <button type="button" className="btn sm gho" onClick={() => setSel(new Set())}>
              取消选择
            </button>
          </div>
        )}

        <div className="pbody" ref={scrollRef}>
          {!style ? (
            <div className="bigempty">
              <div className="fs13 fw5 t2">选择或新建一个款式</div>
            </div>
          ) : visibleImages.length === 0 ? (
            <div className="bigempty">
              <div className="fs13 fw5 t2">
                {imageQuery || status !== "all"
                  ? "当前筛选无匹配参考图"
                  : `${style.name}${category ? ` · ${category.name}` : ""} 还没有图片`}
              </div>
              <div className="fs12 t3">
                {imageQuery || status !== "all" ? (
                  <button
                    type="button"
                    className="btn xs"
                    onClick={() => {
                      setImageQuery("");
                      setStatus("all");
                    }}
                  >
                    清除筛选
                  </button>
                ) : (
                  "点右上角「导入图片」选择本地原图。"
                )}
              </div>
            </div>
          ) : (
            <div className="rlvlist" style={{ height: virt.getTotalSize() }}>
              {virt.getVirtualItems().map((row) => {
                const from = row.index * cols;
                return (
                  <div
                    key={row.index}
                    className="rlrow"
                    style={{
                      height: rowH,
                      transform: `translateY(${row.start}px)`,
                      gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
                    }}
                  >
                    {visibleImages.slice(from, from + cols).map((img) => (
                      <LibraryCard
                        key={img.id}
                        img={img}
                        selected={sel.has(img.id)}
                        showCategory={categoryId === null}
                        onToggle={toggle}
                        onPreview={previewImage}
                        onUsage={setUsageImage}
                        onGenerate={addToGeneration}
                        onRelink={setShowRelink}
                        exportSelection={
                          sel.has(img.id) && sel.size > 1
                            ? selected.map((item) => item.filePath)
                            : undefined
                        }
                      />
                    ))}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

      {(styleEdit === "new" || editedStyle) && (
        <StyleModal
          key={styleEdit}
          style={editedStyle}
          onDelete={deleteStyle}
          onClose={() => setStyleEdit(null)}
          onSaved={async (id) => {
            setStyleEdit(null);
            setStyleId(id);
            await refreshOverview();
          }}
          onImport={async (id) => {
            setStyleEdit(null);
            setStyleId(id);
            try {
              const paths = await unwrap(commands.pickImageFiles());
              if (paths.length) setImportAsk(paths);
            } catch (error) {
              toast.error(message(error));
            }
          }}
          onArchive={async (item) => {
            await unwrap(commands.setLibraryStyleArchived(item.id, !item.archived));
            toast.success(item.archived ? "款式已恢复" : "款式已归档");
            setStyleEdit(null);
            if (!item.archived && !viewArchived && styleId === item.id)
              setStyleId(
                overview?.styles.find((style) => !style.archived && style.id !== item.id)?.id ??
                  null,
              );
            await refreshOverview();
          }}
        />
      )}
      {deletedStylesOpen && (
        <Modal title="已删除款式" width="w640" onClose={() => setDeletedStylesOpen(false)}>
          {deletedStyles.length === 0 ? (
            <p className="fs12 t3">暂无已删除款式。</p>
          ) : (
            <div className="col gap8">
              {deletedStyles.map((item) => (
                <div key={item.id} className="fx ac gap10">
                  <div className="f1">
                    <div className="fs13 fw5">
                      {item.name}
                      {item.product && ` · ${item.product}`}
                    </div>
                    <div className="fs11 t3">
                      {item.total} 张在库图片
                      {item.removedCount > 0 && ` · ${item.removedCount} 张此前已单独移出`}
                      {item.deletedAt !== null &&
                        ` · ${new Date(item.deletedAt * 1000).toLocaleString()} 删除`}
                    </div>
                  </div>
                  <button
                    type="button"
                    className="btn sm"
                    disabled={restoringStyleId !== null}
                    aria-label={`恢复款式 ${item.name}`}
                    onClick={() => void restoreStyle(item)}
                  >
                    {restoringStyleId === item.id ? "正在恢复…" : "恢复款式"}
                  </button>
                </div>
              ))}
            </div>
          )}
        </Modal>
      )}
      {catsOpen && limits && (
        <CategoriesModal
          categories={categories}
          limits={limits}
          onClose={() => setCatsOpen(false)}
          onChanged={refresh}
        />
      )}
      {importAsk && style && (
        <LibraryImportDialog
          key={importAsk.join("\n")}
          paths={importAsk}
          style={style}
          categories={categories}
          initialCategory={categoryId}
          onClose={() => setImportAsk(null)}
          onImport={(paths, cid) => {
            setImportAsk(null);
            void runImport(paths, cid);
          }}
        />
      )}
      {usageImage && (
        <LibraryUsageDialog
          image={usageImage}
          onClose={() => setUsageImage(null)}
          onSaved={refresh}
        />
      )}
      {showRelink && (
        <RelinkDirectoryDialog
          images={showRelink}
          onClose={() => setShowRelink(null)}
          onApplied={refresh}
        />
      )}
      {pickResult && (
        <Modal
          title={`试抽 · ${pickResult.style} · ${pickResult.category}`}
          onClose={() => setPickResult(null)}
          footer={
            <>
              <button
                type="button"
                className="btn sm"
                disabled={picking}
                onClick={() => void tryPick()}
              >
                再抽一次
              </button>
              <button
                type="button"
                className="btn sm"
                disabled={!pickResult.picks.length}
                onClick={() => {
                  setSel(new Set(pickResult.picks.map((item) => item.image.id)));
                  setPickResult(null);
                }}
              >
                选中这些图
              </button>
              <button
                type="button"
                className="btn sm pri"
                disabled={!pickResult.picks.length}
                onClick={() => {
                  void addToGeneration(pickResult.picks.map((item) => item.image));
                }}
              >
                按此顺序加入生成
              </button>
            </>
          }
        >
          <p className="fs12 t2">
            {pickResult.message}。试抽不计使用。可用 {pickResult.freshCount}/{pickResult.poolSize}{" "}
            张。
          </p>
          <ol>
            {pickResult.picks.map((item, index) => (
              <li key={`${item.image.id}-${index}`} className="fx ac gap8 mt10">
                <span className="mono fs11">
                  Image {index + 1} · #{item.image.id}
                </span>
                <button
                  type="button"
                  className="btn xs"
                  onClick={() => void previewImage(item.image)}
                >
                  {item.image.name}
                </button>
                <span className="fs11 t3">
                  {item.tier === "fresh"
                    ? "可用"
                    : item.tier === "repeat"
                      ? "本次重复"
                      : `冷却回退 · ${item.cooldownDays} 天`}
                </span>
              </li>
            ))}
          </ol>
        </Modal>
      )}
      {confirmRemove && (
        <ConfirmModal
          title="移出图库"
          desc={`移出 ${selected.filter((i) => !i.removed).length} 张图：之后不再被工单抽到，文件保留，可在「显示已移出」里放回。`}
          confirmLabel="移出"
          danger
          onClose={() => setConfirmRemove(false)}
          onConfirm={() => {
            setConfirmRemove(false);
            void patch({ removed: true }, "已移出");
          }}
        />
      )}
      {preview && (
        <OriginalPreview
          title={preview.name}
          path={preview.path}
          onClose={() => setPreview(null)}
        />
      )}
      {importing && <RefImportOverlay state={importing} title="导入参考图库" />}
    </PageScaffold>
  );
}

/**
 * 一张图库卡片。memo：只有这一张的数据或选中态变了才重绘。
 * 高度构成与 `CARD_META_H` 是一对（`.rlmeta` 20 + `.rlsub` 14 + 两个 5 的间距）。
 */
const LibraryCard = memo(function LibraryCard({
  img,
  selected,
  showCategory,
  onToggle,
  onPreview,
  onUsage,
  onGenerate,
  onRelink,
  exportSelection,
}: {
  img: LibraryImageView;
  selected: boolean;
  showCategory: boolean;
  onToggle: (id: number, shift: boolean) => void;
  onPreview: (img: LibraryImageView) => Promise<void>;
  onUsage: (img: LibraryImageView) => void;
  onGenerate: (items: LibraryImageView[]) => Promise<void>;
  onRelink: (items: LibraryImageView[]) => void;
  exportSelection?: string[] | undefined;
}) {
  const b = statusBadge(img);
  const src = assetSrc(img.thumbPath);
  return (
    <div
      className={cn(
        "rlcard",
        selected && "sel",
        (!img.enabled || img.removed || img.missing) && "off",
      )}
      onClick={(event) => onToggle(img.id, event.shiftKey)}
      onDoubleClick={() => void onPreview(img)}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget || (e.key !== " " && e.key !== "Enter")) return;
        e.preventDefault();
        if (e.key === "Enter" || !e.shiftKey) void onPreview(img);
        else onToggle(img.id, true);
      }}
      role="button"
      aria-pressed={selected}
      tabIndex={0}
      title={img.missing ? `原图不在了：${img.filePath}` : img.filePath}
    >
      <MediaActions
        path={img.filePath}
        paths={exportSelection}
        label={img.name}
        onPreview={() => void onPreview(img)}
        actions={[
          { label: "用量记录与备注", run: () => onUsage(img) },
          { label: "复制素材编号", run: () => void copyText(String(img.id), "素材编号") },
          ...(img.missing && img.linked
            ? [{ label: "替换失效目录…", run: () => onRelink([img]) }]
            : []),
          ...(!img.missing && !img.removed
            ? [{ label: "加入本次生成", run: () => void onGenerate([img]) }]
            : []),
        ]}
      >
        <div className="rlimg ph">
          {src ? (
            <img className="thumbimg" src={src} alt="" decoding="async" draggable={false} />
          ) : (
            <div className="thumbfill" />
          )}
        </div>
      </MediaActions>
      <div className="rlmeta">
        <span className={cn("bdg", b.tone)}>{b.text}</span>
        <span className="f1" />
        <span className="fs10 t3 mono">#{img.id}</span>
      </div>
      <button
        type="button"
        className="rlsub fs10 t3 nowrap ohide"
        onClick={(event) => {
          event.stopPropagation();
          onUsage(img);
        }}
        title={
          img.coolingUntil
            ? `冷却至 ${new Date(img.coolingUntil * 1000).toLocaleString()}`
            : "用量记录与备注"
        }
      >
        {showCategory ? `${img.category} · ` : ""}
        {img.useCount > 0
          ? `用过 ${img.useCount} 次 · 上次 ${shortDate(img.lastUsedAt ?? 0)}`
          : "还没用过"}
      </button>
    </div>
  );
});

function StyleModal({
  style,
  onClose,
  onSaved,
  onDelete,
  onImport,
  onArchive,
}: {
  style: LibraryStyleView | null;
  onClose: () => void;
  onSaved: (id: number) => void;
  onDelete: (style: LibraryStyleView) => Promise<void>;
  onImport: (id: number) => void;
  onArchive: (style: LibraryStyleView) => Promise<void>;
}) {
  const [name, setName] = useState(style?.name ?? "");
  const [product, setProduct] = useState(style?.product ?? "");
  const [note, setNote] = useState(style?.note ?? "");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);

  async function save() {
    if (style && (name.trim() !== style.name || product.trim() !== style.product)) {
      const ok = window.confirm(
        `该款式当前有 ${style.total} 张在库图片。改名后，后续工单需要使用新的款式名和产品；历史作品及工单仍保留原归属。继续保存？`,
      );
      if (!ok) return;
    }
    try {
      onSaved(await unwrap(commands.saveLibraryStyle(style?.id ?? null, name, product, note)));
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function remove() {
    if (!style || deleting) return;
    setDeleting(true);
    try {
      await onDelete(style);
    } finally {
      setDeleting(false);
    }
  }

  return (
    <>
      <Modal
        title={style ? "修改款式" : "新建款式"}
        onClose={onClose}
        footer={
          <>
            {style && (
              <button
                type="button"
                className="btn sm gho"
                onClick={() => void copyText(style.name, "款式名")}
              >
                复制款式名
              </button>
            )}
            {style && (
              <button
                type="button"
                className="btn sm gho dng"
                disabled={deleting}
                onClick={() => setConfirmDelete(true)}
              >
                {deleting ? "正在删除…" : "删除款式…"}
              </button>
            )}
            <div className="f1" />
            <button type="button" className="btn sm gho" onClick={onClose}>
              取消
            </button>
            <button
              type="button"
              className="btn sm pri"
              disabled={!name.trim() || deleting}
              onClick={save}
            >
              保存
            </button>
          </>
        }
      >
        {style && (
          <div className="fx ac gap6 wrap mt6">
            <button
              type="button"
              className="btn xs"
              onClick={() => void copyText(String(style.id), "款式编号")}
            >
              复制编号 #{style.id}
            </button>
            <button
              type="button"
              className="btn xs"
              onClick={() => useUiStore.getState().openStyleWorks(style.name, style.product)}
            >
              查看关联作品
            </button>
            {!style.archived && (
              <button type="button" className="btn xs" onClick={() => onImport(style.id)}>
                导入此款式…
              </button>
            )}
            <button
              type="button"
              className="btn xs"
              onClick={() => {
                if (
                  !style.archived &&
                  !window.confirm(
                    `归档「${style.name}」及其 ${style.total} 张在库图片？归档期间工单不会从这里抽图，可随时恢复。`,
                  )
                )
                  return;
                void onArchive(style).catch((error) => toast.error(message(error)));
              }}
            >
              {style.archived ? "恢复款式" : "归档款式"}
            </button>
          </div>
        )}
        <div className="rlform mt10">
          <span className="fs12 t2">款式名</span>
          <input
            className="inp"
            value={name}
            placeholder="与工单 organization.style 一致，如 王一博"
            onChange={(e) => setName(e.target.value)}
          />
          <span className="fs12 t2">产品</span>
          <input
            className="inp"
            value={product}
            placeholder="可空；同名款式属于不同产品时用来区分"
            onChange={(e) => setProduct(e.target.value)}
          />
          <span className="fs12 t2">备注</span>
          <input className="inp" value={note} onChange={(e) => setNote(e.target.value)} />
        </div>
      </Modal>
      {style && confirmDelete && (
        <ConfirmModal
          title={`删除「${style.name}」？`}
          desc={`将从参考图库移除${style.product ? `「${style.product}」下的` : "此"}款式和 ${style.total} 张在库图片${style.removedCount > 0 ? `，另有 ${style.removedCount} 张此前单独移出的图片随款式保留` : ""}。新工单将无法从此款式取图。原图保留在原位置，已建任务和使用记录仍可查看。可在「已删除款式」中恢复。`}
          confirmLabel="删除款式"
          danger
          onConfirm={() => void remove()}
          onClose={() => setConfirmDelete(false)}
        />
      )}
    </>
  );
}

/** 一行可编辑的分类。 */
interface CatDraft {
  id: number | null;
  name: string;
  cooldownDays: number;
  imageCount: number;
}

function CategoriesModal({
  categories,
  limits,
  onClose,
  onChanged,
}: {
  categories: LibraryCategoryView[];
  limits: LibraryLimits;
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [rows, setRows] = useState<CatDraft[]>(() =>
    categories.map((c) => ({
      id: c.id,
      name: c.name,
      cooldownDays: c.cooldownDays,
      imageCount: c.imageCount,
    })),
  );

  const edit = (i: number, p: Partial<CatDraft>) =>
    setRows((r) => r.map((row, k) => (k === i ? { ...row, ...p } : row)));

  async function save() {
    try {
      for (const r of rows) {
        const old = categories.find((c) => c.id === r.id);
        if (old && old.name === r.name.trim() && old.cooldownDays === r.cooldownDays) continue;
        if (!r.id && !r.name.trim()) continue;
        await unwrap(commands.saveLibraryCategory(r.id, r.name, r.cooldownDays, null));
      }
      await onChanged();
      onClose();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function remove(row: CatDraft) {
    if (!row.id) {
      setRows((r) => r.filter((x) => x !== row));
      return;
    }
    try {
      await unwrap(commands.deleteLibraryCategory(row.id));
      setRows((r) => r.filter((x) => x !== row));
      await onChanged();
    } catch (e) {
      toast.error(message(e));
    }
  }

  return (
    <Modal
      title="分类与冷却"
      width="w640"
      onClose={onClose}
      footer={
        <>
          <button
            type="button"
            className="btn sm gho"
            onClick={() =>
              setRows((r) => [
                ...r,
                { id: null, name: "", cooldownDays: limits.defaultCooldownDays, imageCount: 0 },
              ])
            }
          >
            <Plus className="ic12" />
            新分类
          </button>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm pri" onClick={save}>
            保存
          </button>
        </>
      }
    >
      <div className="fs12 t3">
        冷却天数：一张图被工单用过后，这么多天内抽图时优先跳过它；可用图不够时仍会退而用最久没用的那张。填
        0 表示这一类不冷却，可以随时重复用。
      </div>
      <div className="rlcats mt10">
        <span className="fs11 fw6 t3">分类</span>
        <span className="fs11 fw6 t3">冷却天数</span>
        <span className="fs11 fw6 t3">图片</span>
        <span />
        {rows.map((r, i) => (
          <CatRow
            key={r.id ?? `new-${i}`}
            row={r}
            maxCooldown={limits.maxCooldownDays}
            onEdit={(p) => edit(i, p)}
            onRemove={() => remove(r)}
          />
        ))}
      </div>
    </Modal>
  );
}

function CatRow({
  row,
  maxCooldown,
  onEdit,
  onRemove,
}: {
  row: CatDraft;
  maxCooldown: number;
  onEdit: (p: Partial<CatDraft>) => void;
  onRemove: () => void;
}) {
  return (
    <>
      <input
        className="inp sm"
        value={row.name}
        placeholder="如 礼盒图"
        onChange={(e) => onEdit({ name: e.target.value })}
      />
      <input
        className="inp sm rlnum"
        type="number"
        min={0}
        max={maxCooldown}
        value={row.cooldownDays}
        onChange={(e) =>
          onEdit({
            cooldownDays: Math.max(
              0,
              Math.min(maxCooldown, Math.round(Number(e.target.value) || 0)),
            ),
          })
        }
      />
      <span className="fs12 t2 mono">{row.imageCount}</span>
      <button
        type="button"
        className="btn xs gho dng"
        disabled={row.imageCount > 0}
        title={row.imageCount > 0 ? "分类下还有图片，不能删除" : "删除分类"}
        onClick={onRemove}
      >
        删除
      </button>
    </>
  );
}
