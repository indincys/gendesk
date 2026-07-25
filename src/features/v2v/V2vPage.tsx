import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { PageScaffold } from "@/features/_shared/PageScaffold";
import { assetSrc } from "@/lib/img";
import {
  type ClipView,
  type ModelInfo,
  type SkuView,
  type StageCounts,
  commands,
  subscribeV2v,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import {
  Check,
  Clapperboard,
  FolderOpen,
  Layers,
  RefreshCw,
  RotateCcw,
  Send,
  Terminal,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 视频流水线看板。
 *
 * 一页回答「这批视频做到哪了」——这正是拆成「作品库导出 + 包内 ledger」之后
 * 谁都答不上来的那个问题。五列即五个阶段，从左到右就是时间方向。
 */
const COLUMNS = [
  {
    stage: "rewrite",
    label: "待改写",
    hint: "验收通过自动入队。去 Claude Code / Codex 跑改写 skill，无需在此操作",
  },
  {
    stage: "ready",
    label: "待提交",
    hint: "唯一需要你过目的一列：扫一眼改写结果，勾选后提交到即梦",
  },
  { stage: "run", label: "已提交", hint: "本机轮询中，出片后自动落盘（关掉应用也不会丢）" },
  { stage: "rev", label: "待验收", hint: "对照首帧图看片；不通过默认重跑同提示词" },
  { stage: "pass", label: "成片", hint: "可入资产库做视频型素材包" },
] as const;

/** 失败与未通过不单独占列（会把看板撑成七列没人看得完），并入「待验收」列尾部。 */
const SIDE_STAGES = ["fail", "rej"] as const;

export function V2vPage() {
  const [clips, setClips] = useState<ClipView[]>([]);
  const [counts, setCounts] = useState<StageCounts | null>(null);
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [detail, setDetail] = useState<ClipView | null>(null);
  const [cmdPreview, setCmdPreview] = useState<string[] | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [assetPick, setAssetPick] = useState(false);
  const [busy, setBusy] = useState(false);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [handoffDir, setHandoffDir] = useState("");
  /** 已提交条目的即梦状态原文（事件推送，不翻译成自造中文态）。 */
  const [progress, setProgress] = useState<Record<number, string>>({});
  // 重入锁用 ref 而非 state：useState 要等下一次渲染才生效，挡不住同一帧内的连点
  // （v0.14.0 上传重复入库就是这么来的）。
  const busyRef = useRef(false);

  const load = useCallback(async () => {
    try {
      const [rows, c] = await Promise.all([
        unwrap(commands.listV2vClips([])),
        unwrap(commands.v2vCounts()),
      ]);
      setClips(rows);
      setCounts(c);
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
  }, []);

  useEffect(() => {
    void load();
    void unwrap(commands.v2vModels())
      .then(setModels)
      .catch(() => setModels([]));
    void unwrap(commands.getV2vSettings())
      .then((s) => setHandoffDir(s.handoffRoot ?? ""))
      .catch(() => {});
  }, [load]);

  // 事件驱动刷新，不轮询（架构铁律 4）。
  useEffect(() => {
    let un: (() => void) | undefined;
    void subscribeV2v({
      onChanged: () => void load(),
      onProgress: (e) => setProgress((cur) => ({ ...cur, [e.clipId]: e.genStatus })),
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [load]);

  const byStage = useMemo(() => {
    const m: Record<string, ClipView[]> = {};
    for (const c of clips) {
      const bucket = m[c.stage];
      if (bucket) bucket.push(c);
      else m[c.stage] = [c];
    }
    return m;
  }, [clips]);

  const selected = useMemo(() => clips.filter((c) => sel.has(c.id)), [clips, sel]);
  const selectedStages = useMemo(() => new Set(selected.map((c) => c.stage)), [selected]);
  const onlyStage = selectedStages.size === 1 ? [...selectedStages][0] : null;

  const guard = async (fn: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await fn();
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const toggle = (id: number) =>
    setSel((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  const selectColumn = (stage: string) =>
    setSel((s) => {
      const n = new Set(s);
      for (const c of byStage[stage] ?? []) n.add(c.id);
      return n;
    });
  const clearSel = () => setSel(new Set());

  const submit = () =>
    guard(async () => {
      const ids = selected.filter((c) => c.stage === "ready").map((c) => c.id);
      if (ids.length === 0) return;
      try {
        const sum = await unwrap(commands.submitV2vClips(ids));
        if (sum.submitted > 0) toast.success(`已提交 ${sum.submitted} 条到即梦`);
        if (sum.failed > 0) toast.error(`${sum.failed} 条提交失败：${sum.firstError ?? ""}`);
        setCmdPreview(null);
        clearSel();
        void load();
      } catch (e) {
        if (e instanceof Error) toast.error(e.message);
      }
    });

  /** 提交确认：把**即将执行的真实命令行**摆在点确认之前。 */
  const openSubmitConfirm = () =>
    guard(async () => {
      const ids = selected.filter((c) => c.stage === "ready").map((c) => c.id);
      if (ids.length === 0) {
        toast("请先在「待提交」列勾选条目");
        return;
      }
      try {
        setCmdPreview(await unwrap(commands.previewV2vCommands(ids)));
      } catch (e) {
        if (e instanceof Error) toast.error(e.message);
      }
    });

  const review = (pass: boolean) =>
    guard(async () => {
      const ids = selected.filter((c) => c.stage === "rev").map((c) => c.id);
      if (ids.length === 0) return;
      const n = await unwrap(commands.reviewV2vClips(ids, pass)).catch((e) => {
        toast.error(String(e));
        return 0;
      });
      toast(pass ? `已通过 ${n} 条` : `已不通过 ${n} 条（成片进废纸篓）`);
      clearSel();
      void load();
    });

  const requeue = (mode: "run" | "rewrite") =>
    guard(async () => {
      const ids = selected.map((c) => c.id);
      if (ids.length === 0) return;
      const n = await unwrap(commands.requeueV2vClips(ids, mode)).catch((e) => {
        toast.error(String(e));
        return 0;
      });
      toast(mode === "run" ? `已重排 ${n} 条待提交` : `已退回改写 ${n} 条`);
      clearSel();
      void load();
    });

  const remove = () =>
    guard(async () => {
      const n = await unwrap(commands.removeV2vClips(selected.map((c) => c.id))).catch((e) => {
        toast.error(String(e));
        return 0;
      });
      toast(`已从流水线移除 ${n} 条（作品本体不受影响）`);
      setConfirmRemove(false);
      clearSel();
      void load();
    });

  const ingest = () =>
    guard(async () => {
      try {
        const sum = await unwrap(commands.ingestV2vRewrites());
        if (sum.applied > 0) toast.success(`已收录 ${sum.applied} 条改写结果`);
        else if (sum.unmatched > 0 || sum.stale > 0)
          toast(`未收录：认不出 ${sum.unmatched} 条、已越过待提交 ${sum.stale} 条`);
        else toast("交接目录里没有新的改写结果");
        void load();
      } catch (e) {
        if (e instanceof Error) toast.error(e.message);
      }
    });

  const pollNow = () =>
    guard(async () => {
      try {
        const n = await unwrap(commands.pollV2vNow());
        toast(n > 0 ? `取回 ${n} 条成片` : "暂无出片，仍在生成中");
        void load();
      } catch (e) {
        if (e instanceof Error) toast.error(e.message);
      }
    });

  const total = clips.length;

  return (
    <PageScaffold
      title="视频流水线"
      caption={
        counts
          ? `待改写 ${counts.rewrite} · 待提交 ${counts.ready} · 已提交 ${counts.run} · 待验收 ${counts.rev}`
          : ""
      }
    >
      <div className="fbar">
        {sel.size > 0 ? (
          <>
            <span className="fs12 t2 nowrap">已选 {sel.size}</span>
            {onlyStage === "ready" && (
              <button
                type="button"
                className="btn sm pri"
                disabled={busy}
                onClick={openSubmitConfirm}
              >
                <Send className="ic12" />
                提交到即梦
              </button>
            )}
            {onlyStage === "rev" && (
              <>
                <button
                  type="button"
                  className="btn sm pri"
                  disabled={busy}
                  onClick={() => review(true)}
                >
                  <Check className="ic12" />
                  通过
                </button>
                <button
                  type="button"
                  className="btn sm gho dng"
                  disabled={busy}
                  onClick={() => review(false)}
                >
                  <X className="ic12" />
                  不通过
                </button>
              </>
            )}
            {onlyStage === "pass" && (
              <button
                type="button"
                className="btn sm pri"
                disabled={busy}
                onClick={() => setAssetPick(true)}
                title="打包为视频型素材包（1 视频 + 封面）入资产库，接上发布链"
              >
                <Layers className="ic12" />
                入资产库
              </button>
            )}
            {/* 重跑放在最显眼处：视频不通过多半是没抽中，不是提示词不对。 */}
            <button
              type="button"
              className="btn sm"
              disabled={busy}
              onClick={() => requeue("run")}
              title="用同一条视频提示词再抽一次（视频不通过多半是没抽中）"
            >
              <RotateCcw className="ic12" />
              重跑
            </button>
            <button
              type="button"
              className="btn sm gho"
              disabled={busy}
              onClick={() => requeue("rewrite")}
              title="清掉视频提示词，退回待改写让 skill 重写"
            >
              退回改写
            </button>
            <button
              type="button"
              className="btn sm gho dng"
              disabled={busy}
              onClick={() => setConfirmRemove(true)}
            >
              <Trash2 className="ic12" />
              移出流水线
            </button>
            <div className="f1" />
            <button type="button" className="btn sm gho" onClick={clearSel}>
              取消选择
            </button>
          </>
        ) : (
          <>
            <button type="button" className="btn sm" disabled={busy} onClick={ingest}>
              <RefreshCw className="ic12" />
              收录改写结果
            </button>
            <button type="button" className="btn sm gho" disabled={busy} onClick={pollNow}>
              查一次进度
            </button>
            <button
              type="button"
              className="btn sm gho"
              onClick={() =>
                void unwrap(commands.openHandoffDir()).catch((e) => toast.error(String(e)))
              }
              title={handoffDir}
            >
              <FolderOpen className="ic12" />
              打开交接目录
            </button>
            <div className="f1" />
            <span className="fs11 t3 nowrap">共 {total} 条在流水线</span>
          </>
        )}
      </div>

      {total === 0 ? (
        <div className="bigempty">
          <Clapperboard className="ic" style={{ width: 26, height: 26, opacity: 0.5 }} />
          <div className="fs13 fw5 t2">流水线是空的</div>
          <div className="fs12 t3" style={{ maxWidth: 460, lineHeight: 1.7 }}>
            给提示词组标上用途「图生视频」（导入 txt 时就能选），
            <br />
            该组的图**验收通过即自动入队**，不需要回作品库找出来再点导出。
          </div>
        </div>
      ) : (
        <div className="pbody">
          <div className="vkb">
            {COLUMNS.map((col) => {
              const rows = byStage[col.stage] ?? [];
              const extra = col.stage === "rev" ? SIDE_STAGES.flatMap((s) => byStage[s] ?? []) : [];
              return (
                <div key={col.stage} className="vcol">
                  <div className="vcolh">
                    <span>{col.label}</span>
                    <span className="n">{rows.length + extra.length}</span>
                    <span className="f1" />
                    {rows.length > 0 && (
                      <button
                        type="button"
                        className="btn xs gho"
                        onClick={() => selectColumn(col.stage)}
                      >
                        全选
                      </button>
                    )}
                  </div>
                  <div className="fs10 t3" style={{ padding: "0 5px 2px", lineHeight: 1.5 }}>
                    {col.hint}
                  </div>
                  {rows.length + extra.length === 0 && <div className="vempty">—</div>}
                  {[...rows, ...extra].map((c) => (
                    <ClipCard
                      key={c.id}
                      c={c}
                      selected={sel.has(c.id)}
                      status={progress[c.id] ?? ""}
                      onToggle={() => toggle(c.id)}
                      onOpen={() => setDetail(c)}
                    />
                  ))}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {detail && (
        <ClipDetail
          clip={detail}
          models={models}
          onClose={() => setDetail(null)}
          onSaved={() => {
            setDetail(null);
            void load();
          }}
        />
      )}

      {cmdPreview && (
        <Modal
          title="确认提交到即梦"
          width="w700"
          onClose={() => setCmdPreview(null)}
          headerExtra={<span className="chip">{cmdPreview.length} 条</span>}
          footer={
            <>
              <span className="fs11 t3">提交即消耗额度，且无法撤回</span>
              <div className="f1" />
              <button type="button" className="btn sm gho" onClick={() => setCmdPreview(null)}>
                取消
              </button>
              <button type="button" className="btn sm pri" disabled={busy} onClick={submit}>
                <Send className="ic12" />
                确认提交
              </button>
            </>
          }
        >
          <div style={{ padding: 4 }}>
            <div className="fs12 t2 mb8" style={{ lineHeight: 1.7 }}>
              下面是**即将执行的完整命令行**（与真正 exec 的参数同源）。
              「我设了却没生效」这类怀疑只能靠把真实请求摆在确认之前来消除。
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {cmdPreview.map((line, i) => (
                <div key={`${i}-${line.slice(0, 24)}`} className="cmdwell">
                  {line}
                </div>
              ))}
            </div>
          </div>
        </Modal>
      )}

      {assetPick && (
        <ClipToAssetModal
          count={selected.length}
          onClose={() => setAssetPick(false)}
          onPick={async (skuId) => {
            // 逐条建包：视频型素材包就是「1 视频 + 封面」，多选即多包。
            let ok = 0;
            for (const c of selected) {
              try {
                if (await unwrap(commands.packFromClip(skuId, c.id))) ok += 1;
              } catch (e) {
                if (e instanceof Error) toast.error(e.message);
              }
            }
            setAssetPick(false);
            clearSel();
            if (ok > 0) toast.success(`已入资产库 ${ok} 个视频素材包`);
          }}
        />
      )}

      {confirmRemove && (
        <ConfirmModal
          title={`从流水线移除 ${sel.size} 条`}
          desc="只移除视频流水线里的条目；对应的作品、成片文件与已入资产库的素材包都不受影响。之后仍可在作品库手动重新加入。"
          confirmLabel="移除"
          danger
          onConfirm={remove}
          onClose={() => setConfirmRemove(false)}
        />
      )}
    </PageScaffold>
  );
}

function ClipCard({
  c,
  selected,
  status,
  onToggle,
  onOpen,
}: {
  c: ClipView;
  selected: boolean;
  status: string;
  onToggle: () => void;
  onOpen: () => void;
}) {
  const thumb = assetSrc(c.posterPath ?? c.thumbPath);
  const badge = stageBadge(c);
  return (
    <div
      className={cn("vcard", selected && "sel")}
      onClick={(e) => {
        // 点缩略图/正文 = 打开详情；点空白 = 勾选。两个动作都常用，
        // 挤到一个点击里必然有一个要绕路。
        if ((e.target as HTMLElement).closest("[data-open]")) onOpen();
        else onToggle();
      }}
    >
      {thumb ? (
        <img src={thumb} alt="" className="vth" data-open />
      ) : (
        <div className="vth" data-open />
      )}
      <div className="vbody">
        <div className="fx ac gap6">
          <span className="pid">{c.promptCode}</span>
          {badge && <span className={cn("bdg", badge.cls)}>{badge.text}</span>}
        </div>
        <div className="fs10 t3 nowrap ohide">{c.groupName}</div>
        <div className="vptxt" data-open>
          {c.videoPrompt ?? c.variablePart ?? c.sourcePrompt}
        </div>
        {status && <div className="fs10 t3">即梦：{status}</div>}
        {c.errorMessage && (
          <div className="fs10" style={{ color: "var(--er)", lineHeight: 1.5 }}>
            {c.errorMessage.slice(0, 120)}
          </div>
        )}
      </div>
    </div>
  );
}

function stageBadge(c: ClipView): { text: string; cls: string } | null {
  if (c.stage === "fail") return { text: "失败", cls: "b-red" };
  if (c.stage === "rej") return { text: "未通过", cls: "b-gray" };
  if (c.stage === "rev") return { text: "待验收", cls: "b-amber" };
  if (c.stage === "pass") return { text: "成片", cls: "b-green" };
  if (c.attempt > 1) return { text: `第 ${c.attempt} 次`, cls: "b-gray" };
  return null;
}

/** 条目详情：待提交列可编辑提示词与参数；待验收列播放成片对照首帧。 */
function ClipDetail({
  clip,
  models,
  onClose,
  onSaved,
}: {
  clip: ClipView;
  models: ModelInfo[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const [prompt, setPrompt] = useState(clip.videoPrompt ?? "");
  const [model, setModel] = useState(clip.modelVersion ?? "");
  const [duration, setDuration] = useState<string>(clip.duration?.toString() ?? "");
  const [res, setRes] = useState(clip.videoResolution ?? "");
  const [saving, setSaving] = useState(false);
  const editable = clip.stage === "ready" || clip.stage === "rewrite";
  const picked = models.find((m) => m.modelVersion === model);

  const save = async () => {
    setSaving(true);
    try {
      await unwrap(
        commands.updateV2vClip(
          clip.id,
          prompt,
          model.trim() === "" ? null : model.trim(),
          duration.trim() === "" ? null : Number(duration),
          res.trim() === "" ? null : res.trim(),
        ),
      );
      toast.success("已保存");
      onSaved();
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    } finally {
      setSaving(false);
    }
  };

  const videoSrc = assetSrc(clip.videoPath);
  const firstFrame = assetSrc(clip.thumbPath);

  return (
    <Modal
      title={clip.promptCode}
      width="w700"
      onClose={onClose}
      headerExtra={
        <>
          <span className="chip">{clip.groupName}</span>
          {clip.batchId != null && <span className="chip">批次 {clip.batchId}</span>}
          {clip.creditCount != null && <span className="chip">{clip.creditCount} 额度</span>}
        </>
      }
      footer={
        <>
          <span className="fs11 t3">
            {editable ? "改完保存即留在待提交列，勾选后统一提交" : "已提交的条目不可再改参数"}
          </span>
          <div className="f1" />
          {editable && (
            <button
              type="button"
              className="btn sm pri"
              disabled={saving || prompt.trim() === ""}
              onClick={save}
            >
              保存
            </button>
          )}
          <button type="button" className="btn sm" onClick={onClose}>
            关闭
          </button>
        </>
      }
    >
      <div className="fx gap14">
        <div style={{ width: 260, flex: "none" }}>
          {videoSrc ? (
            <>
              {/* 对照：先看成片，再看首帧原图 —— 验收要判的正是「动起来之后还对不对」。 */}
              <video className="vplayer" src={videoSrc} controls loop autoPlay muted />
              <div className="fs10 t3 mt6">
                {clip.width}×{clip.height}
                {clip.durationSec != null && ` · ${clip.durationSec.toFixed(1)}s`}
                {clip.fps != null && ` · ${Math.round(clip.fps)}fps`}
              </div>
            </>
          ) : (
            <div className="fs11 t3">尚无成片</div>
          )}
          {firstFrame && (
            <>
              <div className="fs11 fw6 t3 mt10">首帧原图</div>
              <img
                src={firstFrame}
                alt=""
                className="mt6"
                style={{
                  width: "100%",
                  borderRadius: 10,
                  border: "1px solid var(--line)",
                  display: "block",
                }}
              />
            </>
          )}
        </div>
        <div className="f1" style={{ minWidth: 0 }}>
          <div className="fs11 fw6 t3">视频提示词</div>
          {editable ? (
            <textarea
              className="inp mt6"
              style={{ width: "100%", height: 150, padding: 9, lineHeight: 1.6 }}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder="等 Claude Code 侧的改写 skill 写回，或在此手写"
            />
          ) : (
            <div className="ptext mt6" style={{ maxHeight: 150, overflow: "auto" }}>
              {clip.videoPrompt ?? "（无）"}
            </div>
          )}

          {editable && (
            <div className="fx ac gap8 mt10 wrap">
              <select
                className="inp sm"
                value={model}
                onChange={(e) => {
                  setModel(e.target.value);
                  // 换模型即清掉时长与分辨率：留着上一个模型的值必然撞它的约束，
                  // 而报错要发生在花钱之前，不该等到提交。
                  setDuration("");
                  setRes("");
                }}
              >
                <option value="">默认（不发高级参数）</option>
                {models.map((m) => (
                  <option key={m.modelVersion} value={m.modelVersion}>
                    {m.modelVersion}
                  </option>
                ))}
              </select>
              {picked && (
                <>
                  <input
                    className="inp sm"
                    style={{ width: 92 }}
                    type="number"
                    min={picked.minDuration}
                    max={picked.maxDuration}
                    placeholder={`${picked.minDuration}–${picked.maxDuration}s`}
                    value={duration}
                    onChange={(e) => setDuration(e.target.value)}
                  />
                  <select className="inp sm" value={res} onChange={(e) => setRes(e.target.value)}>
                    <option value="">{picked.resolutions[0]}</option>
                    {picked.resolutions.map((r) => (
                      <option key={r} value={r}>
                        {r}
                      </option>
                    ))}
                  </select>
                </>
              )}
            </div>
          )}

          <div className="fs11 fw6 t3 mt14">生图提示词（可变部分）</div>
          <div className="ptext mt6" style={{ maxHeight: 120, overflow: "auto" }}>
            {clip.variablePart || clip.sourcePrompt}
          </div>
          {clip.submitId && (
            <div className="fx ac gap6 mt10">
              <Terminal className="ic12 t3" />
              <span className="chip">{clip.submitId}</span>
            </div>
          )}
          {clip.errorMessage && (
            <div className="fs11 mt10" style={{ color: "var(--er)", lineHeight: 1.6 }}>
              {clip.errorMessage}
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}

/** 成片 → 视频型素材包的 SKU 选择弹窗。 */
function ClipToAssetModal({
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
    void unwrap(commands.listSkus({ tier: null, warnOnly: null, status: null, query: null }))
      .then(setSkus)
      .catch(() => setSkus([]));
  }, []);
  return (
    <Modal
      title="入资产库 · 选择目标 SKU"
      onClose={onClose}
      headerExtra={<span className="chip">{count} 条成片</span>}
      footer={
        <>
          <span className="fs11 t3">每条成片建一个视频型素材包（视频 + 封面），原成片保留</span>
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
          .map((s) => (
            <div key={s.id} className="pickrow" onClick={() => void onPick(s.id)}>
              <span className="pid">{s.code}</span>
              <span className="fw5 fs12 f1 nowrap ohide">{s.styleName}</span>
            </div>
          ))}
        {skus.length === 0 && (
          <div className="fs12 t3" style={{ padding: 12 }}>
            尚无 SKU，请先在资产库创建
          </div>
        )}
      </div>
    </Modal>
  );
}
