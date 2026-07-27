import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { useDebouncedValue } from "@/features/_shared/useDebouncedValue";
import { assetSrc, bg } from "@/lib/img";
import {
  type GroupView,
  type PurposeView,
  type SkuView,
  type WorkView,
  commands,
  unwrap,
} from "@/lib/ipc";
import { tierVisual } from "@/lib/status";
import { cn } from "@/lib/utils";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import {
  CheckSquare,
  Clapperboard,
  Copy,
  Download,
  FolderOpen,
  ImageIcon,
  Layers,
  RefreshCw,
  Search,
  Star,
  Wand2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/** 分节方式。批次是默认值 —— 工作是一阵一阵的，「近期这批」才是找图的实际起点。 */
type GroupBy = "batch" | "date" | "group";

const GROUP_BY_LABEL: Record<GroupBy, string> = {
  batch: "按批次",
  date: "按日期",
  group: "按分组",
};

/** 一个分节：标题 + 副标题 + 该节的作品。 */
interface Section {
  key: string;
  title: string;
  meta: string;
  works: WorkView[];
}

export function WorksPage() {
  const go = useUiStore((s) => s.go);
  const restoreFromBatch = useGenerateStore((s) => s.restoreFromBatch);
  const [works, setWorks] = useState<WorkView[]>([]);
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [detail, setDetail] = useState<WorkView | null>(null);
  const [confirmDel, setConfirmDel] = useState<WorkView | null>(null);
  // E21：源输出文件是否缺失（懒检测：打开详情时校验）。
  const [sourceMissing, setSourceMissing] = useState(false);
  // E15：多选批量操作态。
  const [selectMode, setSelectMode] = useState(false);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [confirmBatchDel, setConfirmBatchDel] = useState(false);
  const [assetPick, setAssetPick] = useState(false);
  const lastClicked = useRef<number | null>(null);

  // ── 筛选态 ───────────────────────────────────────────────────
  const [groupBy, setGroupBy] = useState<GroupBy>("batch");
  const [rawQuery, setRawQuery] = useState("");
  const query = useDebouncedValue(rawQuery, 220);
  const [favOnly, setFavOnly] = useState(false);
  const [groupFilter, setGroupFilter] = useState<number | null>(null);
  const [purposes, setPurposes] = useState<PurposeView[]>([]);
  const [purposeFilter, setPurposeFilter] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [atEnd, setAtEnd] = useState(false);

  const load = useCallback(async () => {
    try {
      setGroups(await unwrap(commands.listPromptGroups()));
      const f = {
        groupId: groupFilter,
        favoriteOnly: favOnly,
        tag: purposeFilter,
        query: query.trim() === "" ? null : query.trim(),
        batchId: null,
      };
      const rows = await unwrap(commands.listWorks(f, page));
      setWorks(rows);
      // 后端一页 300 条；不足即到底（不额外查 count，省一次全表扫描）。
      setAtEnd(rows.length < 300);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, [groupFilter, favOnly, purposeFilter, query, page]);

  useEffect(() => {
    void unwrap(commands.listPurposes())
      .then(setPurposes)
      .catch(() => setPurposes([]));
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  // 改筛选条件即回到第一页，否则筛完停在第 3 页会显示成「什么都没有」。
  useEffect(() => {
    setPage(0);
  }, [groupFilter, favOnly, purposeFilter, query]);

  // ── 分节 ─────────────────────────────────────────────────────
  // 后端已按「批次倒序 + 批次内生成序」返回，这里只做连续切分，不重排序。
  const sections = useMemo<Section[]>(() => buildSections(works, groupBy), [works, groupBy]);

  const toggleFav = async (w: WorkView) => {
    setWorks((cur) => cur.map((x) => (x.id === w.id ? { ...x, favorite: x.favorite ? 0 : 1 } : x)));
    await unwrap(commands.toggleWorkFavorite(w.id)).catch(() => void load());
  };

  const del = async (w: WorkView) => {
    await unwrap(commands.trashWork(w.id)).catch(() => {});
    setDetail(null);
    void load();
    toast("已移入废纸篓");
  };

  // E21：打开详情时懒检测源输出文件是否仍在。
  useEffect(() => {
    if (!detail) return;
    setSourceMissing(false);
    void unwrap(commands.fileExists(detail.imagePath))
      .then((ok) => setSourceMissing(!ok))
      .catch(() => {});
  }, [detail]);

  // E21：从资产区快照重新导出输出文件。
  const reexport = async (w: WorkView) => {
    try {
      await unwrap(commands.reexportWork(w.id));
      setSourceMissing(false);
      toast.success("已重新导出到原输出路径");
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };

  // E33：复制提示词原文。
  const copyPrompt = async (w: WorkView) => {
    await navigator.clipboard.writeText(w.promptText);
    toast.success("提示词已复制");
  };

  // E33：复制图片到系统剪贴板（jpg → png 经 canvas 转换，兼容剪贴板）。
  const copyImage = async (w: WorkView) => {
    try {
      const src = assetSrc(w.imagePath) ?? assetSrc(w.thumbPath);
      if (!src) throw new Error("图片不可读");
      const img = new Image();
      img.crossOrigin = "anonymous";
      await new Promise<void>((res, rej) => {
        img.onload = () => res();
        img.onerror = () => rej(new Error("图片加载失败"));
        img.src = src;
      });
      const canvas = document.createElement("canvas");
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      canvas.getContext("2d")?.drawImage(img, 0, 0);
      const blob = await new Promise<Blob | null>((res) => canvas.toBlob(res, "image/png"));
      if (!blob) throw new Error("转换失败");
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      toast.success("图片已复制到剪贴板");
    } catch (e) {
      if (e instanceof Error) toast.error(`复制图片失败：${e.message}`);
    }
  };

  // E33：用此作品的提示词 + 参考图预填生成页。
  const remix = (w: WorkView) => {
    if (w.refImageId == null || w.groupId == null) {
      toast.error("该作品的参考图或分组已删除，无法一键再生成");
      return;
    }
    restoreFromBatch([{ refImageId: w.refImageId, promptGroupId: w.groupId }], {});
    setDetail(null);
    go("generate");
  };

  // ── E15 批量操作 ──────────────────────────────────────────────
  const exitSelect = () => {
    setSelectMode(false);
    setSel(new Set());
    lastClicked.current = null;
  };
  const onCardClick = (idx: number, id: number, shift: boolean) => {
    if (!selectMode) {
      setDetail(works[idx] ?? null);
      return;
    }
    if (shift && lastClicked.current !== null) {
      const a = Math.min(lastClicked.current, idx);
      const b = Math.max(lastClicked.current, idx);
      setSel((s) => {
        const n = new Set(s);
        for (let i = a; i <= b; i++) {
          const it = works[i];
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
      lastClicked.current = idx;
    }
  };
  /** 全选本节 —— 「快速多选需要做视频的素材」的实际操作，不必再一张张点。 */
  const selectSection = (s: Section) => {
    setSelectMode(true);
    setSel((cur) => {
      const n = new Set(cur);
      for (const w of s.works) n.add(w.id);
      return n;
    });
  };
  const batchFavorite = async () => {
    const ids = [...sel];
    await unwrap(commands.setWorksFavorite(ids, true)).catch((e) => toast.error(String(e)));
    toast.success(`已收藏 ${ids.length} 张`);
    exitSelect();
    void load();
  };
  const batchExport = async () => {
    const dir = await unwrap(commands.pickOutputDir()).catch(() => null);
    if (!dir) return;
    const ids = [...sel];
    const n = await unwrap(commands.exportWorks(ids, dir)).catch((e) => {
      toast.error(String(e));
      return 0;
    });
    if (n < ids.length) toast(`已导出 ${n}/${ids.length} 张（部分源文件缺失已跳过）`);
    else toast.success(`已导出 ${n} 张到所选文件夹`);
    exitSelect();
  };
  /**
   * 手动加入视频流水线。
   *
   * 正常路径是**验收通过即自动入队**（用途=图生视频的组），这个按钮是逃生口：
   * 用途是筛选默认值不是门禁，堵死了就得改代码。
   */
  const batchToPipeline = async () => {
    const ids = [...sel];
    try {
      const n = await unwrap(commands.enqueueWorksV2v(ids));
      if (n === 0) toast("所选作品都已在视频流水线里");
      else toast.success(`已加入视频流水线 ${n} 条`);
      exitSelect();
      void load();
      if (n > 0) go("v2v");
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  };
  const batchDelete = async () => {
    const ids = [...sel];
    await unwrap(commands.trashWorks(ids)).catch((e) => toast.error(String(e)));
    toast(`已移入废纸篓 ${ids.length} 张`);
    setConfirmBatchDel(false);
    exitSelect();
    void load();
  };

  const activeGroup = groups.find((g) => g.id === groupFilter);

  return (
    <PageScaffold title="作品库" caption={`${works.length} 张已通过`}>
      <div className="fbar">
        {selectMode ? (
          <>
            <span className="fs12 t2 nowrap">已选 {sel.size}</span>
            <div className="f1" />
            <button
              type="button"
              className="btn sm"
              disabled={sel.size === 0}
              onClick={batchToPipeline}
              title="加入视频流水线（正常无需手动：用途=图生视频的组，验收通过即自动入队）"
            >
              <Clapperboard className="ic12" />
              加入视频流水线
            </button>
            <button
              type="button"
              className="btn sm"
              disabled={sel.size === 0}
              onClick={batchExport}
            >
              <Download className="ic12" />
              导出到文件夹
            </button>
            <button
              type="button"
              className="btn sm gho"
              disabled={sel.size === 0}
              onClick={batchFavorite}
            >
              <Star className="ic12" />
              收藏
            </button>
            <button
              type="button"
              className="btn sm gho"
              disabled={sel.size === 0}
              onClick={() => setAssetPick(true)}
              title="打包为图集素材包入资产库"
            >
              <Layers className="ic12" />
              入资产库
            </button>
            <button
              type="button"
              className="btn sm gho dng"
              disabled={sel.size === 0}
              onClick={() => setConfirmBatchDel(true)}
            >
              删除
            </button>
            <button type="button" className="btn sm gho" onClick={exitSelect}>
              退出多选
            </button>
          </>
        ) : (
          <>
            <div className="srch" style={{ width: 240 }}>
              <Search className="ic ic12" />
              <input
                className="inp sm"
                style={{ width: "100%", paddingLeft: 26 }}
                placeholder="搜编号 / 分组 / 参考图 / 提示词"
                value={rawQuery}
                onChange={(e) => setRawQuery(e.target.value)}
              />
            </div>
            {/* 分组降级为可搜索的筛选器：上百个分组平铺成分段控件在物理上就不可用。 */}
            <GroupFilter
              groups={groups}
              active={activeGroup ?? null}
              onPick={(id) => setGroupFilter(id)}
            />
            {purposes.map((p) => (
              <button
                key={p.tag}
                type="button"
                className={cn("btn sm", purposeFilter === p.tag ? "" : "gho")}
                onClick={() => setPurposeFilter(purposeFilter === p.tag ? null : p.tag)}
                title={p.hint}
              >
                {p.tag}
              </button>
            ))}
            <button
              type="button"
              className={cn("btn sm", favOnly ? "" : "gho")}
              onClick={() => setFavOnly((v) => !v)}
            >
              <Star className="ic12" />
              收藏
            </button>
            <div className="f1" />
            <div className="seg">
              {(Object.keys(GROUP_BY_LABEL) as GroupBy[]).map((k) => (
                <span
                  key={k}
                  className={cn("sgi", groupBy === k && "on")}
                  onClick={() => setGroupBy(k)}
                >
                  {GROUP_BY_LABEL[k]}
                </span>
              ))}
            </div>
            {works.length > 0 && (
              <button
                type="button"
                className="btn sm gho"
                onClick={() => setSelectMode(true)}
                title="多选作品做批量操作"
              >
                <CheckSquare className="ic12" />
                多选
              </button>
            )}
          </>
        )}
      </div>

      {works.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">
            {query || groupFilter || purposeFilter || favOnly ? "该筛选下暂无作品" : "还没有作品"}
          </div>
          <div className="fs12 t3">通过验收的图片会归档到这里，并同步输出到本地批次文件夹</div>
        </div>
      ) : (
        <div className="pbody">
          {sections.map((s) => (
            <div key={s.key}>
              <div className="wsec">
                <span className="wst">{s.title}</span>
                <span className="wsm f1">{s.meta}</span>
                <button
                  type="button"
                  className="btn xs gho"
                  onClick={() => selectSection(s)}
                  title="把本节全部作品加入选择"
                >
                  全选本节
                </button>
              </div>
              <div className="wgrid" style={{ paddingTop: 0 }}>
                {s.works.map((w) => {
                  const idx = works.findIndex((x) => x.id === w.id);
                  return (
                    <div
                      key={w.id}
                      className={cn("wcard", selectMode && sel.has(w.id) && "sel")}
                      onClick={(e) => onCardClick(idx, w.id, e.shiftKey)}
                    >
                      <NatThumb path={w.thumbPath} className="wcimg wcnat" />
                      <div className="rmeta">
                        <span className="pid">{w.promptCode}</span>
                        <span className="fs10 t3 nowrap ohide f1">{w.groupName}</span>
                        <button
                          type="button"
                          className={cn("star", w.favorite && "on")}
                          onClick={(e) => {
                            e.stopPropagation();
                            void toggleFav(w);
                          }}
                          title="收藏"
                        >
                          <Star className="ic12" fill={w.favorite ? "currentColor" : "none"} />
                        </button>
                      </div>
                      {(w.isI2V || w.inPipeline) && (
                        <div className="wflags">
                          {w.isI2V && <span className="wfl i2v">图生视频</span>}
                          {w.inPipeline && <span className="wfl inq">已入流水线</span>}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          ))}
          {(page > 0 || !atEnd) && (
            <div className="fx ac jc gap8" style={{ padding: "4px 0 28px" }}>
              <button
                type="button"
                className="btn sm gho"
                disabled={page === 0}
                onClick={() => setPage((p) => Math.max(0, p - 1))}
              >
                上一页
              </button>
              <span className="fs11 t3">第 {page + 1} 页</span>
              <button
                type="button"
                className="btn sm gho"
                disabled={atEnd}
                onClick={() => setPage((p) => p + 1)}
              >
                下一页
              </button>
            </div>
          )}
        </div>
      )}

      {detail && (
        <Modal
          title={detail.promptCode}
          width="w700"
          onClose={() => setDetail(null)}
          headerExtra={
            <>
              <span className="bdg b-green">已通过</span>
              {detail.isI2V && <span className="bdg b-gray">图生视频</span>}
              {sourceMissing && <span className="bdg b-red">源文件缺失</span>}
            </>
          }
          footer={
            <>
              <span className="fs11 t3">作品与提示词、参考图长期关联，可追溯</span>
              <div className="f1" />
              <button
                type="button"
                className="btn sm gho dng"
                onClick={() => setConfirmDel(detail)}
              >
                删除
              </button>
              <button type="button" className="btn sm" onClick={() => setDetail(null)}>
                关闭
              </button>
            </>
          }
        >
          <div className="fx gap14">
            <div style={{ width: 300, flex: "none" }}>
              <div
                className="ph"
                style={{
                  ...bg(detail.thumbPath),
                  aspectRatio: "1",
                  borderRadius: 10,
                  border: "1px solid var(--line)",
                }}
              />
              <div className="fx ac gap6 mt10 wrap">
                <span className="chip">{detail.refName}</span>
                <span className="chip">{fmtDate(detail.acceptedAt)}</span>
                {detail.batchId != null && <span className="chip">批次 {detail.batchId}</span>}
              </div>
              <div className="pathwell mt10" style={{ fontSize: "10.5px" }}>
                {detail.imagePath}
              </div>
              <button
                type="button"
                className="btn sm mt10 w100"
                style={{ justifyContent: "center" }}
                onClick={() =>
                  void unwrap(commands.openPathInFolder(detail.imagePath)).catch(() =>
                    toast.error("打开失败：文件可能已被移动或删除"),
                  )
                }
              >
                <FolderOpen className="ic12" />
                打开所在文件夹
              </button>
              {sourceMissing && (
                <button
                  type="button"
                  className="btn sm mt6 w100"
                  style={{ justifyContent: "center" }}
                  onClick={() => void reexport(detail)}
                >
                  <RefreshCw className="ic12" />
                  重新导出
                </button>
              )}
            </div>
            <div className="f1" style={{ minWidth: 0 }}>
              <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
                对应提示词
              </div>
              <div className="ptext mt6" style={{ maxHeight: 260, overflow: "auto" }}>
                {detail.promptText}
              </div>
              <div className="fx ac gap8 mt14 wrap">
                <button
                  type="button"
                  className="btn sm gho"
                  onClick={() => void copyPrompt(detail)}
                >
                  <Copy className="ic12" />
                  复制提示词
                </button>
                <button type="button" className="btn sm gho" onClick={() => void copyImage(detail)}>
                  <ImageIcon className="ic12" />
                  复制图片
                </button>
                <button
                  type="button"
                  className="btn sm gho"
                  onClick={() => remix(detail)}
                  title="带此提示词与参考图预填生成页"
                >
                  <Wand2 className="ic12" />
                  用此配置再生成
                </button>
                {!detail.inPipeline && (
                  <button
                    type="button"
                    className="btn sm"
                    onClick={async () => {
                      try {
                        await unwrap(commands.enqueueWorksV2v([detail.id]));
                        toast.success("已加入视频流水线");
                        setDetail(null);
                        void load();
                      } catch (e) {
                        if (e instanceof Error) toast.error(e.message);
                      }
                    }}
                  >
                    <Clapperboard className="ic12" />
                    加入视频流水线
                  </button>
                )}
              </div>
            </div>
          </div>
        </Modal>
      )}

      {confirmDel && (
        <ConfirmModal
          title="删除作品"
          desc="删除后作品记录进入废纸篓，清理后不可恢复；已导出到本地文件夹的图片文件不会被删除（需自行清理）。"
          confirmLabel="删除"
          danger
          onConfirm={() => del(confirmDel)}
          onClose={() => setConfirmDel(null)}
        />
      )}

      {confirmBatchDel && (
        <ConfirmModal
          title={`删除 ${sel.size} 张作品`}
          desc="删除后作品记录进入废纸篓，清理后不可恢复；已导出到本地文件夹的图片文件不会被删除。"
          confirmLabel="删除"
          danger
          onConfirm={batchDelete}
          onClose={() => setConfirmBatchDel(false)}
        />
      )}

      {assetPick && (
        <WorksToAssetModal
          count={sel.size}
          onClose={() => setAssetPick(false)}
          onPick={async (skuId) => {
            const pack = await unwrap(commands.packFromWorks(skuId, Array.from(sel)));
            setAssetPick(false);
            exitSelect();
            if (pack) toast.success(`已打包 ${pack.fileCount} 张入资产库`);
            else toast.error("未能入库（所选无有效图片）");
          }}
        />
      )}
    </PageScaffold>
  );
}

/**
 * 把已排好序的作品切成连续分节。
 *
 * **只切不排**：后端已按「批次倒序 + 批次内生成序」返回，前端再排一次只会与后端分叉。
 */
export function buildSections(works: WorkView[], by: GroupBy): Section[] {
  const out: Section[] = [];
  for (const w of works) {
    const key = sectionKey(w, by);
    const last = out[out.length - 1];
    if (last && last.key === key) last.works.push(w);
    else out.push({ key, title: "", meta: "", works: [w] });
  }
  for (const s of out) {
    const first = s.works[0];
    if (!first) continue;
    s.key = sectionKey(first, by);
    const names = [...new Set(s.works.map((w) => w.groupName).filter(Boolean))];
    // 组名列出前三个：一个批次混几十个组时，铺满整行的组名反而什么都读不出来。
    const groupsText =
      names.length === 0
        ? ""
        : names.length <= 3
          ? names.join(" · ")
          : `${names.slice(0, 3).join(" · ")} 等 ${names.length} 组`;
    const i2v = s.works.filter((w) => w.isI2V).length;
    const parts = [`${s.works.length} 张`];
    if (groupsText) parts.push(groupsText);
    if (i2v > 0) parts.push(`图生视频 ${i2v}`);
    s.meta = parts.join(" · ");
    switch (by) {
      case "batch":
        s.title = first.batchId == null ? "无批次（历史作品）" : `批次 #${first.batchId}`;
        break;
      case "date":
        s.title = fmtDate(first.acceptedAt);
        break;
      default:
        s.title = first.groupName || "未分组";
    }
    if (by === "batch") s.meta = `${fmtDate(first.acceptedAt)} · ${s.meta}`;
  }
  return out;
}

function sectionKey(w: WorkView, by: GroupBy): string {
  switch (by) {
    case "batch":
      return `b${w.batchId ?? "none"}`;
    case "date":
      return `d${fmtDate(w.acceptedAt)}`;
    default:
      return `g${w.groupId ?? "none"}`;
  }
}

/** 可搜索的分组筛选。取代平铺上百个分段控件 —— 那个控件在 100 个组时物理上不可用。 */
function GroupFilter({
  groups,
  active,
  onPick,
}: {
  groups: GroupView[];
  active: GroupView | null;
  onPick: (id: number | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDoc);
    return () => window.removeEventListener("mousedown", onDoc);
  }, [open]);
  const hits = useMemo(() => {
    const s = q.trim().toLowerCase();
    const list = s === "" ? groups : groups.filter((g) => g.name.toLowerCase().includes(s));
    return list.slice(0, 60);
  }, [groups, q]);
  return (
    <div className="gwrap" ref={ref} style={{ position: "relative" }}>
      <button
        type="button"
        className={cn("btn sm", active ? "" : "gho")}
        onClick={() => setOpen((v) => !v)}
      >
        {active ? `分组：${active.name}` : "分组"}
      </button>
      {open && (
        <div className="gmenu gpick" style={{ left: 0, right: "auto" }}>
          <input
            className="inp sm"
            style={{ width: "100%", marginBottom: 4 }}
            placeholder={`搜索 ${groups.length} 个分组`}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          <button
            type="button"
            className="gmi"
            onClick={() => {
              onPick(null);
              setOpen(false);
            }}
          >
            {active ? "　" : "✓ "}全部分组
          </button>
          {hits.map((g) => (
            <button
              key={g.id}
              type="button"
              className="gmi"
              onClick={() => {
                onPick(g.id);
                setOpen(false);
              }}
            >
              {active?.id === g.id ? "✓ " : "　"}
              {g.name}
              <span className="f1" />
              <span className="fs10 t3">{g.count}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** 「入资产库」SKU 选择弹窗（作品库联动 → 图集素材包）。 */
function WorksToAssetModal({
  count,
  onClose,
  onPick,
}: {
  count: number;
  onClose: () => void;
  onPick: (skuId: number) => void | Promise<void>;
}) {
  const [skus, setSkus] = useState<SkuView[]>([]);
  useEffect(() => {
    void unwrap(commands.listSkus({ tier: null, warnOnly: null, status: null, query: null })).then(
      setSkus,
    );
  }, []);
  return (
    <Modal
      title="入资产库 · 选择目标 SKU"
      onClose={onClose}
      headerExtra={<span className="chip">{count} 张</span>}
      footer={
        <>
          <span className="fs11 t3">选中的输出图复制为一个图集素材包（原作品保留）</span>
          <div className="f1" />
          <button type="button" className="btn sm" onClick={onClose}>
            取消
          </button>
        </>
      }
    >
      <div style={{ padding: 8 }}>
        {skus
          .filter((s) => !s.isGeneral)
          .map((s) => {
            const t = tierVisual(s.tier);
            return (
              <div key={s.id} className="pickrow" onClick={() => void onPick(s.id)}>
                <span className="pid">{s.code}</span>
                <span className="fw5 fs12 f1 nowrap ohide">{s.styleName}</span>
                <span className={cn("bdg", t.badgeClass)}>{t.label}</span>
              </div>
            );
          })}
        {skus.length === 0 && (
          <div className="fs12 t3" style={{ padding: 12 }}>
            尚无 SKU，请先在资产库创建
          </div>
        )}
      </div>
    </Modal>
  );
}

function fmtDate(unix: number): string {
  const d = new Date(unix * 1000);
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}
