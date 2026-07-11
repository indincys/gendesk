import { ConfirmModal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type TrashItemView, commands, unwrap } from "@/lib/ipc";
import { cn, promptLabel } from "@/lib/utils";
import { Check, Eye, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function TrashPage() {
  const [rows, setRows] = useState<TrashItemView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [confirm, setConfirm] = useState<null | { ids: number[]; all: boolean }>(null);
  // E02：查看未通过原图（清理前原图仍暂存）。
  const [viewImage, setViewImage] = useState<string | null>(null);

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

  const purge = async (ids: number[], all: boolean) => {
    const n = all
      ? await unwrap(commands.purgeAllTrash())
      : await unwrap(commands.purgeTrashItems(ids));
    toast(`已彻底删除 ${n} 项 · 编号已回收`);
    void load();
  };

  const toggle = (id: number) =>
    setSel((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  return (
    <PageScaffold title="废纸篓" caption="清理后彻底删除 · 编号回收 · 不可恢复">
      <div className="phd" style={{ borderBottom: "none", minHeight: 0, paddingTop: 8 }}>
        <span className="cnt">{rows.length} 项</span>
        <div className="f1" />
        {sel.size > 0 && (
          <button
            type="button"
            className="btn sm dng"
            onClick={() => setConfirm({ ids: [...sel], all: false })}
          >
            彻底删除所选 · {sel.size}
          </button>
        )}
        {rows.length > 0 && (
          <button
            type="button"
            className="btn sm gho dng"
            onClick={() => setConfirm({ ids: [], all: true })}
          >
            清空废纸篓
          </button>
        )}
      </div>

      {rows.length === 0 ? (
        <div className="bigempty">
          <div className="fs13 fw5 t2">废纸篓是空的</div>
          <div className="fs12 t3">验收未通过与手动删除的内容会先进入这里等待清理</div>
        </div>
      ) : (
        <div className="pbody">
          {rows.map((x) => (
            <div
              key={x.id}
              className={cn("xrow", sel.has(x.id) && "sel")}
              onClick={() => toggle(x.id)}
            >
              <span className={cn("ckb", sel.has(x.id) && "on")}>
                <Check className="ic12" />
              </span>
              <span className="ph xthumb" style={bg(x.thumbPath)} />
              {x.code && <span className="pid noshrink">{promptLabel(x.code, x.title)}</span>}
              <span className="mono fs11 t3 f1 nowrap ohide">
                {x.promptText ?? entityLabel(x.entityType)}
              </span>
              <span className="bdg b-gray noshrink">{x.sourceLabel}</span>
              <span className="fs11 t3 noshrink" style={{ width: 76, textAlign: "right" }}>
                {fmtTime(x.deletedAt)}
              </span>
              {x.imagePath && (
                <button
                  type="button"
                  className="icb"
                  title="查看原图"
                  onClick={(e) => {
                    e.stopPropagation();
                    setViewImage(x.imagePath ?? null);
                  }}
                >
                  <Eye className="ic12" />
                </button>
              )}
              <button
                type="button"
                className="icb"
                title="彻底删除"
                onClick={(e) => {
                  e.stopPropagation();
                  setConfirm({ ids: [x.id], all: false });
                }}
              >
                <Trash2 className="ic12" />
              </button>
            </div>
          ))}
        </div>
      )}

      {confirm && (
        <ConfirmModal
          title={confirm.all ? "清空废纸篓" : "彻底删除所选"}
          desc="将物理删除文件（含未通过原图）、级联清除记录并回收编号，此操作不可恢复。"
          confirmLabel="彻底删除"
          danger
          onConfirm={() => purge(confirm.ids, confirm.all)}
          onClose={() => setConfirm(null)}
        />
      )}

      {viewImage && (
        <div className="ovl" onClick={() => setViewImage(null)}>
          <button
            type="button"
            className="icb"
            style={{ position: "fixed", top: 16, right: 16, zIndex: 1 }}
            title="关闭"
            onClick={() => setViewImage(null)}
          >
            <X className="ic12" />
          </button>
          {assetSrc(viewImage) ? (
            <img
              src={assetSrc(viewImage)}
              alt="未通过原图"
              style={{ maxWidth: "90vw", maxHeight: "90vh", objectFit: "contain" }}
              onClick={(e) => e.stopPropagation()}
            />
          ) : (
            <div className="fs12 t2">无法预览（原图可能已被清理）</div>
          )}
        </div>
      )}
    </PageScaffold>
  );
}

function entityLabel(t: string): string {
  return (
    { task: "验收未通过的结果", work: "已删除作品", prompt: "已删除提示词", ref: "已删除参考图" }[
      t
    ] ?? t
  );
}
function fmtTime(unix: number): string {
  const d = new Date(unix * 1000);
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}
function bg(path?: string | null): React.CSSProperties {
  const src = assetSrc(path);
  return src
    ? { backgroundImage: `url(${src})`, backgroundSize: "cover", backgroundPosition: "center" }
    : {};
}
