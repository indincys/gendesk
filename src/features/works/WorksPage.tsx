import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type GroupView, type WorkView, commands, unwrap } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useGenerateStore } from "@/stores/generate";
import { useUiStore } from "@/stores/ui";
import { Copy, FolderOpen, ImageIcon, RefreshCw, Star, Wand2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
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

  return (
    <PageScaffold title="作品库" caption={`${works.length} 张已通过`}>
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <div className="f1" />
        <div className="seg">
          <span className={cn("sgi", filter === "all" && "on")} onClick={() => setFilter("all")}>
            全部
          </span>
          <span className={cn("sgi", filter === "fav" && "on")} onClick={() => setFilter("fav")}>
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
      </div>

      {works.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">该筛选下暂无作品</div>
          <div className="fs12 t3">通过验收的图片会归档到这里，并同步输出到本地批次文件夹</div>
        </div>
      ) : (
        <div className="pbody">
          <div className="wgrid">
            {works.map((w) => (
              <div key={w.id} className="wcard" onClick={() => setDetail(w)}>
                <div className="ph wcimg" style={bg(w.thumbPath)} />
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
    </PageScaffold>
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
