import { Modal } from "@/components/ui/Modal";
import { type PosterPreview, commands, isTauri, subscribePoster, unwrap } from "@/lib/ipc";
import { usePosterStore } from "@/stores/poster";
import { useUiStore } from "@/stores/ui";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

/** 审稿完成后才询问。所有款式一次确认，Poster 侧没有弹窗，不会创建发布任务。 */
export function PosterImportModal() {
  const batches = usePosterStore((s) => s.batches);
  const enabled = usePosterStore((s) => s.settings?.enabled ?? false);
  const deferred = usePosterStore((s) => s.deferred);
  const refresh = usePosterStore((s) => s.refresh);
  const defer = usePosterStore((s) => s.defer);
  const go = useUiStore((s) => s.go);
  // 先处理最早的待确认，后完成的验收不能顶替用户正在核对的这一批。
  const batch = enabled
    ? batches.filter((b) => b.state === "awaiting" && !deferred.has(b.id)).at(-1)
    : undefined;
  const batchId = batch?.id;
  const [loadedPreview, setPreview] = useState<(PosterPreview & { batchId: string }) | null>(null);
  const preview = loadedPreview?.batchId === batchId ? loadedPreview : null;
  const [choices, setChoices] = useState<Record<string, string>>({});
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const previewRequest = useRef(0);
  const previousStates = useRef<Map<string, string>>(new Map());

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void subscribePoster(() => {
      void refresh().catch((e) => toast.error(String(e)));
    }).then((un) => {
      if (disposed) un();
      else cleanup = un;
      if (!disposed) void refresh().catch((e) => toast.error(String(e)));
    });
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [refresh]);

  useEffect(() => {
    for (const b of batches) {
      const before = previousStates.current.get(b.id);
      if (before && before !== b.state && b.state === "done")
        toast.success(`本批 ${b.count} 张图片已完成 Poster 入库核对`);
      if (before && before !== b.state && b.state === "failed")
        toast.error("图片已通过审稿，Poster 导入待重试，可在设置中查看原因");
    }
    previousStates.current = new Map(batches.map((b) => [b.id, b.state]));
  }, [batches]);

  const load = useCallback(async (id: string) => {
    const request = ++previewRequest.current;
    setLoading(true);
    setError("");
    setPreview(null);
    try {
      const result = await unwrap(commands.previewPosterBatch(id));
      if (request !== previewRequest.current) return;
      setPreview({ ...result, batchId: id });
      setChoices(Object.fromEntries(result.groups.map((g) => [g.key, g.productId ?? ""])));
    } catch (e) {
      if (request === previewRequest.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (request === previewRequest.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (batchId) void load(batchId);
    else {
      previewRequest.current++;
      setPreview(null);
    }
  }, [batchId, load]);

  if (!batch) return null;
  const complete =
    !!preview && preview.groups.length > 0 && preview.groups.every((g) => choices[g.key]);
  const decide = async (importing: boolean) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      await unwrap(
        commands.decidePosterBatch(
          batch.id,
          importing && preview
            ? preview.groups.map((g) => ({ key: g.key, productId: choices[g.key] ?? "" }))
            : null,
        ),
      );
      await refresh();
      if (importing) toast("已确认，正在后台导入 Poster");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  return (
    <Modal
      id="poster-import"
      title={`已通过 ${batch.count} 张图片`}
      width="w640"
      onClose={() => {
        if (!busyRef.current) defer(batch.id);
      }}
      footer={
        <>
          <button
            type="button"
            className="btn sm gho"
            disabled={busy}
            onClick={() => void decide(false)}
          >
            暂不导入
          </button>
          <div className="f1" />
          <button
            type="button"
            className="btn pri sm"
            disabled={busy || loading || !complete}
            onClick={() => void decide(true)}
          >
            {busy ? "正在确认…" : `导入全部 ${batch.count} 张到 Poster`}
          </button>
        </>
      }
    >
      <div className="fs13 fw6">是否将本批图片直接导入 Poster？</div>
      <div className="fs12 t2 mt6" style={{ lineHeight: 1.8 }}>
        按下方款式进入对应商品的图片库。导入后可直接选图使用，不会自动发布。
      </div>
      {loading && <div className="fs12 t2 mt12">正在读取 Poster 商品…</div>}
      {preview?.groups.map((group) => (
        <div className="mt12" key={group.key}>
          <div className="fx ac gap8">
            <span className="fs12 fw6 f1">
              {group.sku || group.style || "未标注款式"} · {group.count} 张
            </span>
            <select
              className="inp"
              aria-label={`${group.sku || group.style || "未标注款式"} 对应的 Poster 商品`}
              disabled={busy}
              value={choices[group.key] ?? ""}
              onChange={(e) => setChoices((s) => ({ ...s, [group.key]: e.target.value }))}
            >
              <option value="">选择对应商品</option>
              {preview.products.map((p) => (
                <option value={p.id} key={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </div>
          {group.warning && <div className="fs11 t2 mt6">{group.warning}</div>}
        </div>
      ))}
      {error && (
        <div className="fs12 mt12" role="alert">
          {error}
        </div>
      )}
      <div className="fx ac gap8 mt12">
        <button
          type="button"
          className="btn sm gho"
          disabled={busy || loading}
          onClick={() => void load(batch.id)}
        >
          重新读取商品
        </button>
        <button
          type="button"
          className="btn sm gho"
          disabled={busy}
          onClick={() => {
            defer(batch.id);
            go("settings");
          }}
        >
          设置接入
        </button>
        <span className="fs11 t3">关闭弹窗会保留待确认记录</span>
      </div>
    </Modal>
  );
}
