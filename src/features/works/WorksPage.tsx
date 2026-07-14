import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { NatThumb } from "@/features/_shared/NatThumb";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type GroupView, type SkuView, type WorkView, commands, unwrap } from "@/lib/ipc";
import { tierVisual } from "@/lib/status";
import { cn } from "@/lib/utils";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import {
  CheckSquare,
  Copy,
  Download,
  FolderOpen,
  ImageIcon,
  Layers,
  RefreshCw,
  Star,
  Wand2,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

export function WorksPage() {
  const go = useUiStore((s) => s.go);
  const restoreFromBatch = useGenerateStore((s) => s.restoreFromBatch);
  const [works, setWorks] = useState<WorkView[]>([]);
  const [groups, setGroups] = useState<GroupView[]>([]);
  const [filter, setFilter] = useState<"all" | "fav" | number>("all");
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

  const load = useCallback(async () => {
    try {
      setGroups(await unwrap(commands.listPromptGroups()));
      const f = {
        groupId: typeof filter === "number" ? filter : null,
        favoriteOnly: filter === "fav",
      };
      setWorks(await unwrap(commands.listWorks(f, null)));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, [filter]);
  useEffect(() => {
    void load();
  }, [load]);

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
  const batchDelete = async () => {
    const ids = [...sel];
    await unwrap(commands.trashWorks(ids)).catch((e) => toast.error(String(e)));
    toast(`已移入废纸篓 ${ids.length} 张`);
    setConfirmBatchDel(false);
    exitSelect();
    void load();
  };

  return (
    <PageScaffold title="作品库" caption={`${works.length} 张已通过`}>
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        {selectMode ? (
          <>
            <span className="fs12 t2 nowrap">已选 {sel.size}</span>
            <div className="f1" />
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
              className="btn sm"
              disabled={sel.size === 0}
              onClick={batchFavorite}
            >
              <Star className="ic12" />
              收藏
            </button>
            <button
              type="button"
              className="btn sm"
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
            <div className="f1" />
            <div className="seg">
              <span
                className={cn("sgi", filter === "all" && "on")}
                onClick={() => setFilter("all")}
              >
                全部
              </span>
              <span
                className={cn("sgi", filter === "fav" && "on")}
                onClick={() => setFilter("fav")}
              >
                收藏
              </span>
              {groups.map((g) => (
                <span
                  key={g.id}
                  className={cn("sgi", filter === g.id && "on")}
                  onClick={() => setFilter(g.id)}
                >
                  {g.name}
                </span>
              ))}
            </div>
          </>
        )}
      </div>

      {works.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">该筛选下暂无作品</div>
          <div className="fs12 t3">通过验收的图片会归档到这里，并同步输出到本地批次文件夹</div>
        </div>
      ) : (
        <div className="pbody">
          <div className="wgrid">
            {works.map((w, idx) => (
              <div
                key={w.id}
                className={cn("wcard", selectMode && sel.has(w.id) && "sel")}
                onClick={(e) => onCardClick(idx, w.id, e.shiftKey)}
              >
                <NatThumb path={w.thumbPath} className="wcimg wcnat" />
                <div className="rmeta">
                  <span className="pid">{w.promptCode}</span>
                  <span className="fs10 t3 nowrap ohide f1">
                    {w.groupName} · {fmtDate(w.acceptedAt)}
                  </span>
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
              </div>
            ))}
          </div>
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
                  className="btn sm"
                  onClick={() => remix(detail)}
                  title="带此提示词与参考图预填生成页"
                >
                  <Wand2 className="ic12" />
                  用此配置再生成
                </button>
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

function bg(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
