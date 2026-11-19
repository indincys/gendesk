import { Modal } from "@/components/ui/Modal";
import { MediaActions, copyText } from "@/features/_shared/MediaActions";
import { assetSrc } from "@/lib/img";
import { type DeliveryDraft, type DraftImage, commands, unwrap } from "@/lib/ipc";
import { ArrowDown, ArrowUp, GripVertical } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

export function PosterDraftDialog({
  batchId,
  onClose,
  onChange,
  enabled,
}: {
  batchId: string;
  enabled: boolean;
  onClose: () => void;
  onChange: () => void;
}) {
  const [draft, setDraft] = useState<DeliveryDraft | null>(null);
  const [images, setImages] = useState<DraftImage[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [staticOnly, setStaticOnly] = useState(false);
  const drag = useRef<number | null>(null);

  const load = useCallback(async () => {
    try {
      const value = await unwrap(commands.getPosterDraft(batchId));
      setDraft(value);
      setImages(value.images);
      setDirty(false);
      setError("");
    } catch (error) {
      setError(String(error));
    }
  }, [batchId]);

  useEffect(() => {
    void load();
  }, [load]);

  function move(from: number, to: number) {
    if (!draft?.editable || busy || from === to || to < 0 || to >= images.length) return;
    setImages((current) => {
      const next = [...current];
      const [item] = next.splice(from, 1);
      if (item) next.splice(to, 0, item);
      return next;
    });
    setDirty(true);
  }

  async function save(confirm: boolean) {
    if (!draft || busy) return;
    setBusy(true);
    setError("");
    try {
      await unwrap(
        commands.savePosterDraft(
          batchId,
          draft.revision,
          images.map(({ uid, excluded }) => ({ uid, excluded })),
          confirm,
        ),
      );
      toast.success(confirm ? "已确认交付，正在交给 Poster" : "草稿已保存，等待你确认交付");
      onChange();
      if (confirm) onClose();
      else await load();
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function exportDraft() {
    if (!draft || busy || dirty) return;
    setBusy(true);
    setError("");
    try {
      const dest = await unwrap(commands.pickOutputDir());
      if (!dest) return;
      const result = await unwrap(
        commands.exportPosterDraft(batchId, dest, staticOnly, draft.revision),
      );
      toast(`已导出 ${result.exported} 张 · ${result.failures.length} 张未导出`, {
        action: {
          label: "在文件管理器中显示",
          onClick: () => {
            void unwrap(commands.openPathInFolder(result.directory)).catch((error) =>
              toast.error(String(error)),
            );
          },
        },
      });
      if (result.failures.length) setError(result.failures.join("\n"));
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }

  const included = images.filter((item) => !item.excluded);
  const cover = included[0]?.uid;
  const waiting = draft?.waitingIntake
    ? "还在收图"
    : draft?.waitingReview
      ? `等 ${draft.waitingReview} 张图审完`
      : included.some((item) => item.wantsLive && (!item.liveImage || !item.liveVideo))
        ? "有实况 JPG 或 MOV 未就绪"
        : "";
  const close = () => {
    if (busy) return;
    if (dirty && !window.confirm("草稿还有未保存的调整，要关闭吗？")) return;
    onClose();
  };

  return (
    <Modal
      title="Poster 交付清单"
      width="w700"
      onClose={close}
      footer={
        <>
          <button type="button" className="btn sm gho" disabled={busy} onClick={close}>
            关闭
          </button>
          <div className="f1" />
          <button
            type="button"
            className="btn sm gho"
            disabled={busy || dirty}
            onClick={() => void load()}
          >
            刷新状态
          </button>
          <button
            type="button"
            className="btn sm"
            disabled={busy || dirty || included.length === 0}
            title={dirty ? "先保存调整，再导出此顺序" : undefined}
            onClick={() => void exportDraft()}
          >
            导出到文件夹
          </button>
          {draft?.editable && (
            <>
              <button
                type="button"
                className="btn sm"
                disabled={busy || !dirty}
                onClick={() => void save(false)}
              >
                保存草稿
              </button>
              <button
                type="button"
                className="btn sm pri"
                disabled={busy || !enabled || Boolean(waiting) || included.length === 0}
                onClick={() => void save(true)}
              >
                确认交付 · {included.length} 张
              </button>
            </>
          )}
        </>
      }
    >
      {draft ? (
        <>
          <div className="fx ac gap8 wrap">
            <span className="chip">
              {draft.state === "done" ? "已导入" : draft.editable ? "草稿" : "已冻结"}
            </span>
            <span className="fs12">
              {included.length} 张参与交付 · {images.length - included.length} 张已移出
            </span>
            {waiting && <span className="bdg b-amber">{waiting}</span>}
            {!enabled && draft.editable && <span className="bdg b-amber">Poster 交付已关闭</span>}
            {draft.materialId && (
              <button
                type="button"
                className="btn xs"
                onClick={() => void copyText(draft.materialId ?? "", "Poster 素材 ID")}
              >
                复制 Poster 素材 ID
              </button>
            )}
          </div>
          {draft.lastError && (
            <p role="status" className="fs12 t2">
              {draft.lastError}
            </p>
          )}
          {included.some((item) => item.wantsLive) && (
            <label className="fx ac gap6 fs12 mt10">
              <input
                type="checkbox"
                checked={staticOnly}
                onChange={(e) => setStaticOnly(e.target.checked)}
              />
              手工导出只取静态图片
            </label>
          )}
          <ol className="poster-draft-list">
            {images.map((item, index) => {
              const sequence = included.findIndex(({ uid }) => uid === item.uid) + 1;
              return (
                <li
                  key={item.uid}
                  className="poster-draft-row"
                  onDragOver={(event) => {
                    if (drag.current !== null) event.preventDefault();
                  }}
                  onDrop={(event) => {
                    if (drag.current === null) return;
                    event.preventDefault();
                    move(drag.current, index);
                    drag.current = null;
                  }}
                >
                  {draft.editable && (
                    <span
                      draggable={!busy}
                      className="poster-drag-grip"
                      title="拖动排序，也可用上移和下移按钮"
                      onDragStart={(event) => {
                        drag.current = index;
                        event.dataTransfer.setData("text/plain", item.uid);
                        event.dataTransfer.effectAllowed = "move";
                      }}
                      onDragEnd={() => {
                        drag.current = null;
                      }}
                    >
                      <GripVertical className="ic12" />
                    </span>
                  )}
                  <img src={assetSrc(item.thumbPath ?? item.imagePath) ?? undefined} alt="" />
                  <div className="f1 ohide">
                    <div className="fs12 fw5 nowrap ohide" title={item.itemName}>
                      {item.itemName}
                    </div>
                    <div className="fs10 t3">
                      {item.excluded
                        ? "已移出"
                        : `${String(sequence).padStart(2, "0")}${item.uid === cover ? " · 封面" : ""}`}
                      {item.style ? ` · ${item.style}` : ""}
                      {item.productName ? ` · ${item.productName}` : " · 商品待匹配"}
                      {item.taskId == null ? "" : ` · 任务 #${item.taskId}`}
                      {item.wantsLive
                        ? item.liveImage && item.liveVideo
                          ? " · 实况已配对"
                          : " · 等待实况配对"
                        : ""}
                    </div>
                  </div>
                  <MediaActions path={item.imagePath} label={item.itemName} />
                  {draft.editable && (
                    <>
                      <button
                        type="button"
                        className="btn xs"
                        aria-label={`上移 ${item.itemName}`}
                        disabled={busy || index === 0}
                        onClick={() => move(index, index - 1)}
                      >
                        <ArrowUp className="ic12" />
                      </button>
                      <button
                        type="button"
                        className="btn xs"
                        aria-label={`下移 ${item.itemName}`}
                        disabled={busy || index === images.length - 1}
                        onClick={() => move(index, index + 1)}
                      >
                        <ArrowDown className="ic12" />
                      </button>
                      {!item.excluded && item.uid !== cover && (
                        <button
                          type="button"
                          className="btn xs"
                          disabled={busy}
                          onClick={() => move(index, 0)}
                        >
                          设封面
                        </button>
                      )}
                      <button
                        type="button"
                        className="btn xs gho"
                        disabled={busy}
                        aria-label={`${item.excluded ? "放回" : "从本次交付移除"} ${item.itemName}`}
                        onClick={() => {
                          setImages((current) =>
                            current.map((entry) =>
                              entry.uid === item.uid
                                ? { ...entry, excluded: !entry.excluded }
                                : entry,
                            ),
                          );
                          setDirty(true);
                        }}
                      >
                        {item.excluded ? "放回" : "移出本次"}
                      </button>
                    </>
                  )}
                </li>
              );
            })}
          </ol>
        </>
      ) : (
        !error && (
          <p role="status" className="fs12">
            正在读取交付清单…
          </p>
        )
      )}
      {error && (
        <div role="alert" className="vhint er">
          <p style={{ whiteSpace: "pre-wrap" }}>{error}</p>
          <button
            type="button"
            className="btn xs"
            disabled={busy}
            onClick={() => {
              if (!dirty || window.confirm("重新读取会丢掉未保存的调整，继续吗？")) void load();
            }}
          >
            重新读取
          </button>
        </div>
      )}
    </Modal>
  );
}
