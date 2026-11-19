import { Modal } from "@/components/ui/Modal";
import { Toggle } from "@/components/ui/Stepper";
import { PosterDraftDialog } from "@/features/settings/PosterDraftDialog";
import {
  type LivePhotoStatus,
  type PosterBatch,
  type PosterPreview,
  type PosterSettings,
  commands,
  subscribeLivePhoto,
  subscribePoster,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { usePosterStore } from "@/stores/poster";
import { ArrowRight, FolderOpen, RotateCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * 设置页「审稿后自动化」：审稿通过之后的两步，都默认关闭、都不弹窗。
 *
 * - 自动转实况图：后台用内置助手把通过的图转成 JPG + MOV。
 * - 自动导入 Poster：通过的图按款式进入 Poster 对应商品的图片库存。
 *
 * 两项都开时 Rust 让 Poster 批次等实况配对落盘，再连同视频一起导入。
 * 这里只发开关与处置命令，状态全部回读后端。
 */
export function AutomationSection({ id }: { id: string }) {
  const [live, setLive] = useState<LivePhotoStatus | null>(null);
  const settings = usePosterStore((s) => s.settings);
  const batches = usePosterStore((s) => s.batches);
  const refreshPoster = usePosterStore((s) => s.refresh);
  const [busy, setBusy] = useState(false);
  const [binding, setBinding] = useState<PosterBatch | null>(null);
  const [draft, setDraft] = useState<PosterBatch | null>(null);

  const loadLive = useCallback(() => unwrap(commands.getLivePhotoStatus()).then(setLive), []);

  useEffect(() => {
    const offLive = subscribeLivePhoto(() => void loadLive().catch(() => {}));
    const offPoster = subscribePoster(() => void refreshPoster().catch(() => {}));
    void loadLive().catch(() => {});
    void refreshPoster().catch((e) => toast.error(String(e)));
    return () => {
      void offLive.then((off) => off());
      void offPoster.then((off) => off());
    };
  }, [loadLive, refreshPoster]);

  const run = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    try {
      await action();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      await Promise.all([loadLive(), refreshPoster()]).catch(() => {});
    }
  };

  const savePoster = (patch: Partial<PosterSettings>) =>
    run(async () => {
      if (!settings) return;
      await unwrap(commands.updatePosterSettings({ ...settings, ...patch }));
    });

  const liveOn = !!live?.supported && live.enabled;
  const posterOn = !!settings?.autoImport;
  const failed = batches.filter((b) => b.state === "failed");
  const active = batches.filter(
    (b) => b.state === "queued" || b.state === "preparing" || b.state === "awaiting",
  );
  const recentDone = batches.filter((b) => b.state === "done").slice(0, 3);

  return (
    <section className="sec" id={id}>
      <div className="sechead">
        <span className="fw6 fs13">审稿后自动化</span>
        <span className="pcap">设置实况转换与 Poster 交付方式</span>
      </div>

      <div className="aflow">
        <span className="afstep on">审稿通过</span>
        <ArrowRight className="ic12 t3" />
        <span className={cn("afstep", liveOn && "on")}>{liveOn ? "转实况图" : "不转实况"}</span>
        <ArrowRight className="ic12 t3" />
        <span className={cn("afstep", posterOn && "on")}>
          {posterOn ? "进 Poster 对应款式" : "不进 Poster"}
        </span>
        <div className="f1" />
        <span className="fs11 t3">
          {posterOn && settings?.confirmSets
            ? "图文草稿收齐验收后，由你调整并确认交付"
            : liveOn && posterOn
              ? "先转实况图，配对完成后连同视频一起导入"
              : posterOn
                ? "通过即导入静态图，不弹窗"
                : liveOn
                  ? "只在本机生成实况照片"
                  : "只完成审稿"}
        </span>
      </div>

      {/* ---- 自动转实况图 ---- */}
      <div className="arow">
        <Toggle
          label="自动转实况图"
          on={liveOn}
          onClick={() => {
            if (!live?.supported) return;
            void run(() => unwrap(commands.setLivePhotoEnabled(!live.enabled)));
          }}
        />
        <div className="arowbody">
          <div className="fx ac gap8 wrap">
            <span className="fs12 fw6">自动转实况图</span>
            {live && !live.supported && <span className="bdg b-gray">仅 macOS</span>}
            {!!live?.pending && <span className="bdg b-blue">{live.pending} 张转换中</span>}
            {!!live?.failed && <span className="bdg b-red">{live.failed} 张失败</span>}
            <div className="f1" />
            {!!live?.failed && (
              <button
                type="button"
                className="btn sm"
                disabled={busy}
                onClick={() => void run(() => unwrap(commands.retryLivePhotos()))}
              >
                <RotateCw className="ic12" />
                重试失败
              </button>
            )}
            {live?.supported && (
              <button
                type="button"
                className="btn sm gho"
                onClick={() =>
                  void unwrap(commands.openLivePhotoDir()).catch((e) => toast.error(String(e)))
                }
              >
                <FolderOpen className="ic12" />
                打开目录
              </button>
            )}
          </div>
          <div className="fs11 t3 mt6">
            应用内置转换助手，后台把通过的图转成 JPG + MOV 实况照片，成对放在「成品 / 实况照片」下。
          </div>
          {live?.error && (
            <div className="fs11 mt6" style={{ color: "var(--er)" }}>
              最近一次失败：{live.error}
            </div>
          )}
        </div>
      </div>

      {/* ---- 自动导入 Poster ---- */}
      <div className="arow">
        <Toggle
          label="自动导入 Poster"
          on={posterOn}
          onClick={() => void savePoster({ autoImport: !posterOn })}
        />
        <div className="arowbody">
          <div className="fx ac gap8 wrap">
            <span className="fs12 fw6">自动导入 Poster</span>
            {!!active.length && <span className="bdg b-blue">{active.length} 批进行中</span>}
            {!!failed.length && <span className="bdg b-red">{failed.length} 批需处理</span>}
          </div>
          <div className="fs11 t3 mt6">
            通过的图按款式进入 Poster 对应商品。同一次提交、同一款式、同一需求的图等这一组审完，
            组成一条图文（图片包，第 1 张作封面），标题文案在 Poster 里配；封面图进封面库。Poster
            直接引用作品目录里的成品，导入后请保留原文件。关闭会暂停交付，已排队的记录仍可查看。
          </div>
          {settings && posterOn && (
            <div className="fx ac gap8 mt10 wrap">
              <Toggle
                label="新图文由我确认后交付"
                on={Boolean(settings.confirmSets)}
                onClick={() => void savePoster({ confirmSets: !settings.confirmSets })}
              />
              <span className="fs12">新图文由我确认后交付</span>
              <span className="fs11 t3">只影响新建图文包，已存在的草稿仍需确认。</span>
            </div>
          )}
          {settings && (
            <div className="fx ac gap10 mt10">
              <span className="fs11 t3 nowrap">Poster 应用</span>
              <div className="pathwell f1 ohide nowrap" title={settings.executable}>
                {settings.executable}
              </div>
              <button
                type="button"
                className="btn sm"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    const picked = await unwrap(commands.pickPosterApp());
                    if (picked)
                      await unwrap(
                        commands.updatePosterSettings({ ...settings, executable: picked }),
                      );
                  })
                }
              >
                选择应用
              </button>
            </div>
          )}
          {failed.length > 1 && (
            <button
              type="button"
              className="btn sm mt10"
              disabled={busy || !posterOn}
              onClick={() => void run(() => unwrap(commands.retryPosterImports()))}
            >
              重试全部待交付批次
            </button>
          )}
          {(failed.length > 0 || active.length > 0 || recentDone.length > 0) && (
            <div className="pblist mt10">
              {[...failed, ...active, ...recentDone].map((b) => (
                <BatchRow
                  key={b.id}
                  batch={b}
                  busy={busy}
                  onOpen={() => setDraft(b)}
                  onBind={() => setBinding(b)}
                  onSkip={() => {
                    if (window.confirm("放弃这一批 Poster 交付？作品文件仍可在作品库找到。")) {
                      void run(() => unwrap(commands.skipPosterBatch(b.id)));
                    }
                  }}
                  onRetry={() => void run(() => unwrap(commands.retryPosterBatch(b.id)))}
                />
              ))}
            </div>
          )}
        </div>
      </div>

      {draft && (
        <PosterDraftDialog
          key={draft.id}
          batchId={draft.id}
          enabled={posterOn}
          onClose={() => setDraft(null)}
          onChange={() => void refreshPoster().catch((error) => toast.error(String(error)))}
        />
      )}
      {binding && (
        <BindModal
          batch={binding}
          onClose={() => setBinding(null)}
          onDone={() => {
            setBinding(null);
            void refreshPoster().catch(() => {});
          }}
        />
      )}
    </section>
  );
}

function BatchRow({
  batch,
  busy,
  onOpen,
  onBind,
  onSkip,
  onRetry,
}: {
  batch: PosterBatch;
  busy: boolean;
  onOpen: () => void;
  onBind: () => void;
  onSkip: () => void;
  onRetry: () => void;
}) {
  const when = new Date(batch.createdAt * 1000).toLocaleString(undefined, {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
  const badge =
    batch.state === "failed" ? (
      <span className="bdg b-red">需处理</span>
    ) : batch.state === "done" ? (
      <span className="bdg b-green">已导入</span>
    ) : batch.waitingIntake ? (
      <span className="bdg b-amber">等剩余图片收齐</span>
    ) : batch.waitingReview > 0 ? (
      <span className="bdg b-amber">等本组 {batch.waitingReview} 张审完</span>
    ) : batch.waitingLive > 0 ? (
      <span className="bdg b-amber">等 {batch.waitingLive} 张实况图</span>
    ) : batch.state === "awaiting" ? (
      <span className="bdg b-amber">等待你确认</span>
    ) : (
      <span className="bdg b-blue">{batch.state === "preparing" ? "审稿中" : "导入中"}</span>
    );
  return (
    <div className={cn("pbrow", batch.state === "failed" && "bad")}>
      <div className="fx ac gap8">
        {badge}
        <span className="fs12">
          {batch.isSet ? `图文 · ${batch.count} 张` : `${batch.count} 张`}
        </span>
        <span className="mono fs10 t3">{when}</span>
        <div className="f1" />
        <button type="button" className="btn sm" onClick={onOpen}>
          {batch.state === "awaiting" ? "打开交付草稿" : "查看交付清单"}
        </button>
        {batch.state === "awaiting" && (
          <>
            <button type="button" className="btn sm" disabled={busy} onClick={onBind}>
              指定商品
            </button>
            <button type="button" className="btn sm gho" disabled={busy} onClick={onSkip}>
              不导入
            </button>
          </>
        )}
        {batch.state === "failed" && (
          <>
            <button type="button" className="btn sm" disabled={busy} onClick={onRetry}>
              重试本批
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={onBind}>
              指定商品
            </button>
            <button type="button" className="btn sm gho" disabled={busy} onClick={onSkip}>
              不导入
            </button>
          </>
        )}
      </div>
      {batch.lastError && <div className="fs11 t2 mt6">{batch.lastError}</div>}
    </div>
  );
}

/** 失败批次的一次性商品指认：选定后记住款式绑定，以后同款自动导入。 */
function BindModal({
  batch,
  onClose,
  onDone,
}: { batch: PosterBatch; onClose: () => void; onDone: () => void }) {
  const [preview, setPreview] = useState<PosterPreview | null>(null);
  const [picks, setPicks] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void unwrap(commands.previewPosterBatch(batch.id))
      .then((p) => {
        setPreview(p);
        setPicks(Object.fromEntries(p.groups.map((g) => [g.key, g.productId ?? ""])));
      })
      .catch((e) => {
        toast.error(e instanceof Error ? e.message : String(e));
        onClose();
      });
  }, [batch.id, onClose]);

  const ready = !!preview && preview.groups.every((g) => picks[g.key]);
  const save = async () => {
    if (!preview || !ready) return;
    setBusy(true);
    try {
      await unwrap(
        commands.bindPosterBatch(
          batch.id,
          preview.groups.map((g) => ({ key: g.key, productId: picks[g.key] ?? "" })),
        ),
      );
      toast.success(
        batch.state === "awaiting" ? "已指定商品，等待确认交付" : "已记住款式对应关系，正在导入",
      );
      onDone();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="指定 Poster 商品"
      width="w640"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">只需指定一次，之后同款式沿用</span>
          <div className="f1" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn pri" disabled={!ready || busy} onClick={save}>
            {batch.state === "awaiting" ? "保存对应关系" : "保存并导入"}
          </button>
        </>
      }
    >
      {!preview ? (
        <div className="fs12 t3">正在读取 Poster 商品…</div>
      ) : (
        <div className="col gap8">
          {preview.groups.map((g) => (
            <div className="bindrow" key={g.key}>
              <div className="col gap4 ohide">
                <span className="fs12 fw6 nowrap ohide">{g.style || g.sku || "未标款式"}</span>
                <span className="fs11 t3">
                  {g.count} 张{g.sku && g.style ? ` · ${g.sku}` : ""}
                  {g.warning ? ` · ${g.warning}` : ""}
                </span>
              </div>
              <select
                className="inp"
                value={picks[g.key] ?? ""}
                onChange={(e) => setPicks((cur) => ({ ...cur, [g.key]: e.target.value }))}
              >
                <option value="">选择商品…</option>
                {preview.products.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                    {p.isGeneric ? "（通用）" : ""}
                  </option>
                ))}
              </select>
            </div>
          ))}
        </div>
      )}
    </Modal>
  );
}
