import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import { type TrashItemView, commands, unwrap } from "@/lib/ipc";
import { cn, promptLabel } from "@/lib/utils";
import { Check, Eye, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

export function TrashPage() {
  const [rows, setRows] = useState<TrashItemView[]>([]);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [confirm, setConfirm] = useState<null | { ids: number[]; all: boolean }>(null);
  // 任务4：点击查看被丢弃的大图 + 提示词原文（清理前仍暂存）。
  const [detail, setDetail] = useState<TrashItemView | null>(null);

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
              <button
                type="button"
                className="ph xthumb"
                title="查看大图与提示词"
                style={{ ...bg(x.thumbPath), cursor: "pointer", padding: 0 }}
                onClick={(e) => {
                  e.stopPropagation();
                  setDetail(x);
                }}
              />
              {x.code && <span className="pid noshrink">{promptLabel(x.code, x.title)}</span>}
              <span className="mono fs11 t3 f1 nowrap ohide">
                {x.promptText ?? entityLabel(x.entityType)}
              </span>
              <span className="bdg b-gray noshrink">{x.sourceLabel}</span>
              <span className="fs11 t3 noshrink" style={{ width: 76, textAlign: "right" }}>
                {fmtTime(x.deletedAt)}
              </span>
              <button
                type="button"
                className="icb"
                title="查看大图与提示词"
                onClick={(e) => {
                  e.stopPropagation();
                  setDetail(x);
                }}
              >
                <Eye className="ic12" />
              </button>
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

      {detail && (
        <Modal
          title={detail.code ? promptLabel(detail.code, detail.title) : entityLabel(detail.entityType)}
          width="w700"
          onClose={() => setDetail(null)}
          headerExtra={<span className="bdg b-gray">{detail.sourceLabel}</span>}
          footer={
            <>
              <span className="fs11 t3">清理后彻底删除并回收编号，不可恢复</span>
              <div className="f1" />
              <button
                type="button"
                className="btn sm gho dng"
                onClick={() => {
                  const id = detail.id;
                  setDetail(null);
                  setConfirm({ ids: [id], all: false });
                }}
              >
                彻底删除
              </button>
              <button type="button" className="btn sm" onClick={() => setDetail(null)}>
                关闭
              </button>
            </>
          }
        >
          <div className="fx gap14">
            <div style={{ width: 320, flex: "none" }}>
              {assetSrc(detail.imagePath) ? (
                <img
                  src={assetSrc(detail.imagePath)}
                  alt="被丢弃的图片"
                  style={{
                    width: "100%",
                    borderRadius: 10,
                    border: "1px solid var(--line)",
                    display: "block",
                  }}
                />
              ) : assetSrc(detail.thumbPath) ? (
                <img
                  src={assetSrc(detail.thumbPath)}
                  alt="缩略图"
                  style={{
                    width: "100%",
                    borderRadius: 10,
                    border: "1px solid var(--line)",
                    display: "block",
                  }}
                />
              ) : (
                <div
                  className="ph"
                  style={{ aspectRatio: 1, borderRadius: 10, border: "1px solid var(--line)" }}
                />
              )}
              <div className="fs11 t3 mt10">删除于 {fmtTime(detail.deletedAt)}</div>
              {!detail.imagePath && (
                <div className="fs11 t3 mt6" style={{ lineHeight: 1.6 }}>
                  原图已随清理删除，仅保留缩略图与提示词记录。
                </div>
              )}
            </div>
            <div className="f1" style={{ minWidth: 0 }}>
              <div className="fs11 fw6 t3" style={{ letterSpacing: ".05em" }}>
                提示词原文
              </div>
              <div className="ptext mt6" style={{ maxHeight: 380, overflow: "auto" }}>
                {detail.promptText ?? "（无提示词记录）"}
              </div>
            </div>
          </div>
        </Modal>
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
