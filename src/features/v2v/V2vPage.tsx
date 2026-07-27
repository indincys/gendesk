import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { V2vInspector } from "@/features/v2v/V2vInspector";
import { V2vLogPanel } from "@/features/v2v/V2vLogPanel";
import { V2vParamsPanel } from "@/features/v2v/V2vParamsPanel";
import { V2vQueuePanel } from "@/features/v2v/V2vQueuePanel";
import { V2vReviewFlow } from "@/features/v2v/V2vReviewFlow";
import {
  type Row,
  SIGNAL_CHIPS,
  SORTS,
  STAGE_CHIPS,
  STAGE_META,
  type Section,
  type SignalKey,
  type SortKey,
  type Stage,
  type StageFilter,
  buildSections,
  deriveRows,
  fmtAgo,
  fmtDur,
  fmtSpan,
  matchQuery,
  matchStage,
  sortRows,
} from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import {
  type AwayDigest,
  type ClipView,
  type CreditStats,
  type EffectiveParams,
  type HandoffStatus,
  type ModelInfo,
  type QueueStats,
  type SkuView,
  type SubmitPreview,
  type V2vTick,
  type V2vUndoEntry,
  commands,
  subscribeV2v,
  unwrap,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import {
  Activity,
  Clapperboard,
  FolderOpen,
  RefreshCw,
  ScrollText,
  Send,
  SlidersHorizontal,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 视频流水线。
 *
 * ## 为什么不是看板（五列卡片）了
 *
 * 五列看板把「阶段」当成了唯一的轴，而实测下来阶段恰恰是**最不缺**的信息 —— 它就写在
 * 那条的脸上。真正答不上来的三个问题都是跨阶段的：
 *
 * 1. **这一批做到哪了。** 卡片按阶段散落在五列里，一批 30 条要横着扫五遍才拼得出来。
 *    故改成按**批次**分节的表格，一行一条，分段条一眼给出这一批的阶段混合。
 * 2. **有没有出事。** 18 条幽灵单和 18 条正常排队在「已提交」列里长得一模一样，
 *    而处置完全相反（一个免费重跑，一个必须继续等）。故加「信号」这条正交的筛选轴，
 *    并让每行都说出**判断依据**而不只是状态。
 * 3. **这一条花没花钱。** 卡片放不下，只能开弹窗一条一条看。故常驻详情栏。
 *
 * ## 键盘
 *
 * J/K 移动 · 空格 通过 · X 不通过 · R 重跑 · E 退回改写 · W 继续等待 · A 入资产库 ·
 * U 撤销 · F 对照首帧 · ⏎ 全屏看片 · ⌘⏎ 确认提交 · ⌥\ 详情栏 · ⌥1/2/3 观测/日志/参数。
 */
export function V2vPage() {
  // ── 数据 ─────────────────────────────────────────────
  const [clips, setClips] = useState<ClipView[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [eff, setEff] = useState<EffectiveParams | null>(null);
  const [credit, setCredit] = useState<CreditStats | null>(null);
  const [queue, setQueue] = useState<QueueStats | null>(null);
  const [handoff, setHandoff] = useState<HandoffStatus | null>(null);
  const [digest, setDigest] = useState<AwayDigest | null>(null);
  const [tick, setTick] = useState<V2vTick | null>(null);
  const [progress, setProgress] = useState<Record<number, string>>({});
  /** 「几秒前」要自己走字，否则一个静止的「12 秒前」比没有还误导。 */
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  // ── 筛选与选择 ───────────────────────────────────────
  const [stage, setStage] = useState<StageFilter>("need");
  const [signals, setSignals] = useState<SignalKey[]>([]);
  const [sort, setSort] = useState<SortKey>("batch");
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cur, setCur] = useState<number | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [doneOpen, setDoneOpen] = useState<Set<string>>(new Set());

  // ── 界面态 ───────────────────────────────────────────
  const [inspector, setInspector] = useState(true);
  const [screen, setScreen] = useState<"list" | "review">("list");
  const [showFrame, setShowFrame] = useState(false);
  const [bannerOpen, setBannerOpen] = useState(false);
  const [showLog, setShowLog] = useState(false);
  const [showParams, setShowParams] = useState(false);
  const [showObserve, setShowObserve] = useState(false);
  const [cmdPreview, setCmdPreview] = useState<{ ids: number[]; data: SubmitPreview } | null>(null);
  const [assetPick, setAssetPick] = useState<number[] | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [busy, setBusy] = useState(false);
  /** 撤销令牌由 Rust 造，前端只当信封（见 `V2vAction` 的注释）。 */
  const [undo, setUndo] = useState<{ label: string; entries: V2vUndoEntry[] } | null>(null);
  /** 看片流里按 A 直投的目标 SKU：选过一次就记住，否则每条都要弹一次选择器。 */
  const [lastSku, setLastSku] = useState<number | null>(null);
  /** 本轮（进入看片流后）判了多少 —— 顶部那句「已过 N · 已毙 M」。 */
  const [tally, setTally] = useState({ passed: 0, killed: 0 });

  // 重入锁用 ref 而非 state：useState 要等下一次渲染才生效，挡不住同一帧内的连点。
  const busyRef = useRef(false);

  const load = useCallback(async () => {
    try {
      setClips(await unwrap(commands.listV2vClips([])));
    } catch (e) {
      if (e instanceof Error) toast.error(e.message);
    }
    void unwrap(commands.v2vQueueStats())
      .then(setQueue)
      .catch(() => {});
  }, []);

  useEffect(() => {
    void load();
    void unwrap(commands.v2vModels())
      .then(setModels)
      .catch(() => setModels([]));
    void unwrap(commands.v2vEffectiveParams())
      .then(setEff)
      .catch(() => {});
    void unwrap(commands.v2vHandoffStatus())
      .then(setHandoff)
      .catch(() => {});
    // 余额要跑一次 CLI（秒级），与页面主体并行加载，拉不到就少显示一段。
    void unwrap(commands.v2vCreditStats())
      .then(setCredit)
      .catch(() => {});
  }, [load]);

  // 开屏战报：只在**确实离开过**且**确实发生过事**时才出现。
  // 拿全部历史冒充昨夜的战果，会让这条横幅从第二天起就没人信。
  useEffect(() => {
    void unwrap(commands.v2vAwayDigest())
      .then((d) => {
        setDigest(d);
        const happened = d.finished + d.failed + d.credits > 0;
        if (d.awaySecs >= 1800 && happened) setBannerOpen(true);
        // 看过就记 —— 否则同一份战报会在每次切页时重放。
        void unwrap(commands.v2vMarkSeen()).catch(() => {});
      })
      .catch(() => {});
  }, []);

  // 事件驱动刷新，不轮询（架构铁律 4）。
  useEffect(() => {
    let un: (() => void) | undefined;
    void subscribeV2v({
      onChanged: () => void load(),
      onProgress: (e) => setProgress((c) => ({ ...c, [e.clipId]: e.genStatus })),
      onTick: setTick,
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [load]);

  // 这个秒表只驱动「x 秒前 / 已等 x」这些文案，不去后端要数据 —— 它不是轮询，
  // 而是让已经收到的时间戳继续走字。
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(t);
  }, []);

  // ── 派生 ─────────────────────────────────────────────
  const rows = useMemo(() => deriveRows(clips, models, eff, now), [clips, models, eff, now]);
  const byId = useMemo(() => new Map(rows.map((r) => [r.clip.id, r])), [rows]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = rows.filter(
      (r) =>
        matchStage(r.stage, stage) && signals.every((s) => r.signals.has(s)) && matchQuery(r, q),
    );
    return sortRows(filtered, sort);
  }, [rows, stage, signals, sort, query]);

  const sections = useMemo(() => buildSections(rows, visible), [rows, visible]);
  const active = useMemo(() => sections.filter((s) => !s.done), [sections]);
  const settled = useMemo(() => sections.filter((s) => s.done), [sections]);

  const stageCount = useCallback(
    (k: StageFilter) => rows.filter((r) => matchStage(r.stage, k)).length,
    [rows],
  );
  const signalCount = useCallback(
    (k: SignalKey) => rows.filter((r) => r.signals.has(k)).length,
    [rows],
  );

  /** 待验收序列（受当前筛选影响）—— 看片流走的就是它。 */
  const revList = useMemo(() => visible.filter((r) => r.stage === "rev"), [visible]);

  const curRow = (cur == null ? null : byId.get(cur)) ?? visible[0] ?? null;
  const curId = curRow?.clip.id ?? null;
  const revIndex = curId == null ? -1 : revList.findIndex((r) => r.clip.id === curId);

  // 默认落在第一条待验收上：一进页面就该看到「等你判定」那条，而不是一个空播放器。
  useEffect(() => {
    if (cur != null) return;
    const first = visible.find((r) => r.stage === "rev") ?? visible[0];
    if (first) setCur(first.clip.id);
  }, [cur, visible]);

  // 看片流里光标必须落在待验收序列内：否则画面放的是 list[0]、按钮判的是光标那一条，
  // 两者不是同一个片子 —— 而这里每一次按键都在花钱或毙片。
  useEffect(() => {
    if (screen !== "review" || revList.length === 0) return;
    if (revIndex >= 0) return;
    const first = revList[0];
    if (first) setCur(first.clip.id);
  }, [screen, revList, revIndex]);

  const selected = useMemo(() => rows.filter((r) => sel.has(r.clip.id)), [rows, sel]);
  const selStages = useMemo(() => new Set(selected.map((r) => r.stage)), [selected]);
  const onlyStage: Stage | null = selStages.size === 1 ? ([...selStages][0] ?? null) : null;

  // ── 动作 ─────────────────────────────────────────────
  const guard = useCallback(async (fn: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  /**
   * 判完自动跳下一条 —— 看片流一秒一条，手动再点一次「下一条」等于把节奏折半。
   *
   * 走**哪一条序列**取决于当前在哪个屏：看片流里只该在待验收之间走，跳到一条
   * 「已提交」上会让大播放器当场变成空画面；看板上则按当前筛选的顺序走。
   * 判完的那条会离开原序列，故取原序列里它后面的一条（没有就退回前一条）。
   */
  const advance = useCallback(
    (fromId: number) => {
      const list = screen === "review" ? revList : visible;
      const at = list.findIndex((r) => r.clip.id === fromId);
      if (at < 0) return;
      const next = list[at + 1] ?? list[at - 1] ?? null;
      if (next) setCur(next.clip.id);
    },
    [screen, revList, visible],
  );

  const review = useCallback(
    (ids: number[], pass: boolean, advanceFrom?: number) =>
      guard(async () => {
        if (ids.length === 0) return;
        const res = await unwrap(commands.reviewV2vClips(ids, pass));
        if (res.changed === 0) {
          toast("这些条目不在待验收阶段");
          return;
        }
        setUndo({ label: res.label, entries: res.undo });
        setTally((t) =>
          pass
            ? { ...t, passed: t.passed + res.changed }
            : { ...t, killed: t.killed + res.changed },
        );
        toast(pass ? res.label : `${res.label}（成片进废纸篓）`);
        if (advanceFrom != null) advance(advanceFrom);
        else setSel(new Set());
        await load();
      }),
    [guard, advance, load],
  );

  const requeue = useCallback(
    (ids: number[], mode: "run" | "rewrite" | "wait", advanceFrom?: number) =>
      guard(async () => {
        if (ids.length === 0) return;
        const res = await unwrap(commands.requeueV2vClips(ids, mode));
        if (res.changed === 0) {
          toast(
            mode === "wait"
              ? "没有可继续等待的条目（只有判了超时且提交单还在的才行）"
              : "这些条目当前阶段不允许该操作",
          );
          return;
        }
        setUndo({ label: res.label, entries: res.undo });
        toast(
          mode === "wait"
            ? `${res.label}（沿用原提交单，不再扣额度）`
            : mode === "run"
              ? `${res.label}（需再次确认提交，会重新扣费）`
              : res.label,
        );
        if (advanceFrom != null) advance(advanceFrom);
        else setSel(new Set());
        await load();
      }),
    [guard, advance, load],
  );

  const doUndo = useCallback(() => {
    const u = undo;
    if (!u || u.entries.length === 0) return;
    void guard(async () => {
      const n = await unwrap(commands.undoV2v(u.entries));
      setUndo(null);
      setTally({ passed: 0, killed: 0 });
      toast(n > 0 ? `已撤销 ${n} 条` : "已无法撤销（这些条目之后又被改动过）");
      await load();
    });
  }, [undo, guard, load]);

  /** 提交确认：把**即将执行的真实命令行**与这一下要花的额度摆在点确认之前。 */
  const openSubmit = useCallback(
    (ids: number[]) =>
      guard(async () => {
        const ready = ids.filter((id) => byId.get(id)?.stage === "ready");
        if (ready.length === 0) {
          toast("请先选中「待提交」阶段的条目");
          return;
        }
        setCmdPreview({ ids: ready, data: await unwrap(commands.previewV2vCommands(ready)) });
      }),
    [guard, byId],
  );

  const doSubmit = useCallback(() => {
    const p = cmdPreview;
    if (!p) return;
    void guard(async () => {
      const sum = await unwrap(commands.submitV2vClips(p.ids));
      if (sum.submitted > 0) toast.success(`已提交 ${sum.submitted} 条到即梦`);
      if (sum.failed > 0) toast.error(`${sum.failed} 条提交失败：${sum.firstError ?? ""}`);
      setCmdPreview(null);
      setSel(new Set());
      await load();
    });
  }, [cmdPreview, guard, load]);

  /** 成片 → 视频型素材包。选过一次 SKU 就记住，看片流里按 A 才不必每条都弹窗。 */
  const packInto = useCallback(
    (ids: number[], skuId: number, advanceFrom?: number) =>
      guard(async () => {
        let ok = 0;
        for (const id of ids) {
          if (await unwrap(commands.packFromClip(skuId, id))) ok += 1;
        }
        setLastSku(skuId);
        setAssetPick(null);
        if (ok > 0) toast.success(`已入资产库 ${ok} 个视频素材包`);
        else toast("没有可打包的条目（只有验收通过且有成片文件的才行）");
        if (advanceFrom != null) advance(advanceFrom);
        else setSel(new Set());
        await load();
      }),
    [guard, advance, load],
  );

  const packOrPick = useCallback(
    (ids: number[], advanceFrom?: number) => {
      const packable = ids.filter((id) => {
        const r = byId.get(id);
        return r?.stage === "pass" && !r.clip.inAssetLib;
      });
      if (packable.length === 0) {
        toast("只有验收通过且尚未入库的成片可以入资产库");
        return;
      }
      if (lastSku != null) void packInto(packable, lastSku, advanceFrom);
      else setAssetPick(packable);
    },
    [byId, lastSku, packInto],
  );

  const remove = useCallback(
    () =>
      guard(async () => {
        const n = await unwrap(commands.removeV2vClips([...sel]));
        toast(`已从流水线移除 ${n} 条（作品本体不受影响）`);
        setConfirmRemove(false);
        setSel(new Set());
        await load();
      }),
    [guard, sel, load],
  );

  const ingest = useCallback(
    () =>
      guard(async () => {
        const sum = await unwrap(commands.ingestV2vRewrites());
        if (sum.applied > 0) toast.success(`已收录 ${sum.applied} 条改写结果`);
        else if (sum.unmatched > 0 || sum.stale > 0)
          toast(`未收录：认不出 ${sum.unmatched} 条、已越过待提交 ${sum.stale} 条`);
        else toast("交接目录里没有新的改写结果");
        setHandoff(await unwrap(commands.v2vHandoffStatus()));
        await load();
      }),
    [guard, load],
  );

  const pollNow = useCallback(
    () =>
      guard(async () => {
        const n = await unwrap(commands.pollV2vNow());
        toast(n > 0 ? `取回 ${n} 条成片` : "暂无出片，仍在生成中");
        await load();
      }),
    [guard, load],
  );

  // ── 看片流的单条判定（键盘与按钮共用） ────────────────
  const judgeCurrent = useCallback(
    (kind: "pass" | "rej" | "rerun" | "rewrite" | "wait" | "pack") => {
      const r = curRow;
      if (!r) return;
      const id = r.clip.id;
      if (kind === "pass") {
        if (r.stage !== "rev") return;
        void review([id], true, id);
      } else if (kind === "rej") {
        if (r.stage !== "rev") return;
        void review([id], false, id);
      } else if (kind === "rerun") {
        void requeue([id], "run", id);
      } else if (kind === "rewrite") {
        void requeue([id], "rewrite", id);
      } else if (kind === "wait") {
        void requeue([id], "wait", id);
      } else {
        packOrPick([id], id);
      }
    },
    [curRow, review, requeue, packOrPick],
  );

  /** 「通过并入资产库」= 先定案再打包。顺序反了会撞 `pack_from_clip` 的 pass 门禁。 */
  const passAndPack = useCallback(() => {
    const r = curRow;
    if (!r || r.stage !== "rev") return;
    const id = r.clip.id;
    void guard(async () => {
      const res = await unwrap(commands.reviewV2vClips([id], true));
      if (res.changed === 0) return;
      setUndo({ label: res.label, entries: res.undo });
      setTally((t) => ({ ...t, passed: t.passed + 1 }));
      await load();
      if (lastSku != null) {
        if (await unwrap(commands.packFromClip(lastSku, id))) toast.success("已通过并入资产库");
        advance(id);
        await load();
      } else {
        // 还没选过目标 SKU：先弹一次选择器，选完由 packInto 接着走。
        setAssetPick([id]);
      }
    });
  }, [curRow, guard, load, lastSku, advance]);

  const move = useCallback(
    (d: 1 | -1) => {
      if (visible.length === 0) return;
      const at = curId == null ? -1 : visible.findIndex((r) => r.clip.id === curId);
      const next = visible[Math.max(0, Math.min(visible.length - 1, (at < 0 ? 0 : at) + d))];
      if (next) setCur(next.clip.id);
    },
    [visible, curId],
  );

  const modalOpen =
    cmdPreview != null ||
    assetPick != null ||
    confirmRemove ||
    showLog ||
    showParams ||
    showObserve;

  // ── 键盘 ─────────────────────────────────────────────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      // 命令面板 / 速查面板打开时整页让路：不然在 ⌘K 里打字会顺手判掉一条视频。
      const ui = useUiStore.getState();
      if (ui.paletteOpen || ui.helpOpen) return;
      if (e.metaKey || e.ctrlKey) {
        // ⌘⏎ 确认提交：选中的待提交条目，或（没选时）当前光标那一条。
        if (e.key === "Enter") {
          e.preventDefault();
          const ids = sel.size > 0 ? [...sel] : curId != null ? [curId] : [];
          void openSubmit(ids);
        }
        return;
      }
      // ⌥1/2/3 与 ⌥\ 用 e.code：macOS 上 Alt 会把 key 改写成 ¡ / « 之类的符号。
      if (e.altKey) {
        if (e.code === "Digit1") {
          e.preventDefault();
          setShowObserve(true);
        } else if (e.code === "Digit2") {
          e.preventDefault();
          setShowLog(true);
        } else if (e.code === "Digit3") {
          e.preventDefault();
          setShowParams(true);
        } else if (e.code === "Backslash") {
          e.preventDefault();
          setInspector((v) => !v);
        }
        return;
      }
      if (modalOpen) return;

      if (e.key === "Escape") {
        if (screen === "review") {
          e.preventDefault();
          setScreen("list");
        }
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        if (screen === "list" && curRow?.clip.videoPath) {
          setTally({ passed: 0, killed: 0 });
          setScreen("review");
        }
        return;
      }
      if (e.key === "j" || e.key === "J" || e.key === "ArrowDown") {
        e.preventDefault();
        move(1);
        return;
      }
      if (e.key === "k" || e.key === "K" || e.key === "ArrowUp") {
        e.preventDefault();
        move(-1);
        return;
      }
      if (e.key === " ") {
        e.preventDefault();
        judgeCurrent("pass");
        return;
      }
      if (e.key === "x" || e.key === "X") {
        judgeCurrent("rej");
        return;
      }
      if (e.key === "r" || e.key === "R") {
        judgeCurrent("rerun");
        return;
      }
      if (e.key === "e" || e.key === "E") {
        judgeCurrent("rewrite");
        return;
      }
      if (e.key === "w" || e.key === "W") {
        judgeCurrent("wait");
        return;
      }
      if (e.key === "a" || e.key === "A") {
        if (screen === "review") passAndPack();
        else judgeCurrent("pack");
        return;
      }
      if (e.key === "u" || e.key === "U") {
        doUndo();
        return;
      }
      if (e.key === "f" || e.key === "F") {
        setShowFrame((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [screen, modalOpen, move, judgeCurrent, passAndPack, doUndo, curRow, curId, sel, openSubmit]);

  // ── 页头统计 ─────────────────────────────────────────
  const passN = rows.filter((r) => r.stage === "pass").length;
  const rejN = rows.filter((r) => r.stage === "rej").length;
  const passRate = passN + rejN === 0 ? null : Math.round((passN / (passN + rejN)) * 100);
  const staleSecs = queue?.sinceLastFinish ?? null;
  const stale = (queue?.running ?? 0) > 0 && staleSecs != null && staleSecs > 2 * 3600;

  if (clips.length === 0) {
    return (
      <div className="col f1 ohide">
        <V2vHeader
          tick={tick}
          now={now}
          balance={credit?.balance ?? null}
          spentDay={credit?.spentDay ?? null}
          passRate={passRate}
          stale={false}
          staleSecs={null}
          onObserve={() => setShowObserve(true)}
          onLog={() => setShowLog(true)}
          onParams={() => setShowParams(true)}
        />
        <div className="bigempty">
          <Clapperboard className="ic" style={{ width: 26, height: 26, opacity: 0.5 }} />
          <div className="fs13 fw5 t2">流水线是空的</div>
          <div className="fs12 t3" style={{ maxWidth: 460, lineHeight: 1.7 }}>
            给提示词组标上用途「图生视频」（导入 txt 时就能选），该组的图
            <b>验收通过即自动入队</b>，不需要回作品库找出来再点导出。
          </div>
        </div>
        {showLog && <V2vLogPanel onClose={() => setShowLog(false)} />}
        {showParams && (
          <V2vParamsPanel
            models={models}
            selectedReady={[]}
            onClose={() => setShowParams(false)}
            onApplied={() => setShowParams(false)}
          />
        )}
        {showObserve && (
          <ObserveModal
            tick={tick}
            now={now}
            credit={credit}
            onClose={() => setShowObserve(false)}
          />
        )}
      </div>
    );
  }

  return (
    <div className="col f1 ohide" style={{ position: "relative" }}>
      <V2vHeader
        tick={tick}
        now={now}
        balance={credit?.balance ?? null}
        spentDay={credit?.spentDay ?? null}
        passRate={passRate}
        stale={stale}
        staleSecs={staleSecs}
        onObserve={() => setShowObserve(true)}
        onLog={() => setShowLog(true)}
        onParams={() => setShowParams(true)}
      />

      {bannerOpen && digest && (
        <div className="vbanner">
          <span className="ttl">你离开的 {fmtSpan(digest.awaySecs)}</span>
          <span className="it">
            出片 <b style={{ color: "var(--ok2)" }}>{digest.finished} 条</b>
          </span>
          <span className="it">
            失败{" "}
            <b style={{ color: "var(--er)" }}>
              {digest.failed} 条{digest.phantom > 0 && `（幽灵单 ${digest.phantom} · 未计费）`}
            </b>
          </span>
          <span className="it">
            扣费 <b>{digest.credits} 额度</b>
          </span>
          <span className="it">
            待验收现有 <b style={{ color: "var(--wr2)" }}>{digest.revNow} 条</b>
          </span>
          <div className="f1" />
          {digest.revNow > 0 && (
            <button
              type="button"
              className="btn xs pri"
              onClick={() => {
                setStage("rev");
                setBannerOpen(false);
                const first = rows.find((r) => r.stage === "rev");
                if (first) setCur(first.clip.id);
                setTally({ passed: 0, killed: 0 });
                setScreen("review");
              }}
            >
              进看片流
            </button>
          )}
          <button type="button" className="btn xs gho" onClick={() => setBannerOpen(false)}>
            知道了
          </button>
        </div>
      )}

      {/* 阶段筛选 */}
      <div className="vfilt">
        <input
          className="inp sm"
          style={{ width: 168 }}
          placeholder="编号 / 组 / 提示词…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {STAGE_CHIPS.map((c) => {
          const on = stage === c.key;
          const dot = c.key === "need" || c.key === "all" ? null : STAGE_META[c.key as Stage].seg;
          return (
            <button
              key={c.key}
              type="button"
              className={cn("vchip", on && "on")}
              onClick={() => setStage(c.key)}
            >
              {dot && <span className="d" style={{ background: dot }} />}
              {c.label}
              <span className="n">{stageCount(c.key)}</span>
            </button>
          );
        })}
      </div>

      {/* 信号筛选 —— 与阶段正交的例外轴 */}
      <div className="vfilt sig">
        <span className="lb">信号</span>
        {SIGNAL_CHIPS.map((c) => {
          const on = signals.includes(c.key);
          const n = signalCount(c.key);
          return (
            <button
              key={c.key}
              type="button"
              className={cn("vsig", on && "on", n === 0 && "zero")}
              title={c.title}
              onClick={() =>
                setSignals((s) =>
                  s.includes(c.key) ? s.filter((x) => x !== c.key) : [...s, c.key],
                )
              }
            >
              {c.label} <span className="n">{n}</span>
            </button>
          );
        })}
        <div className="f1" />
        <span className="fs10 t3 nowrap" title={handoff?.pendingDir}>
          交接：{handoff?.items ?? 0} 条已物化
          {handoff?.lastIngestAt != null && ` · ${fmtAgo(now - handoff.lastIngestAt)}收录`}
          {handoff?.error && ` · 物化失败：${handoff.error}`}
        </span>
        <button type="button" className="btn xs" disabled={busy} onClick={ingest}>
          <RefreshCw className="ic12" />
          收录改写
        </button>
        <button
          type="button"
          className="btn xs gho"
          onClick={() =>
            void unwrap(commands.openHandoffDir()).catch((e) => toast.error(String(e)))
          }
          title={handoff?.pendingDir}
        >
          <FolderOpen className="ic12" />
        </button>
        <button
          type="button"
          className="btn xs"
          onClick={() => {
            const ks = Object.keys(SORTS) as SortKey[];
            setSort((s) => ks[(ks.indexOf(s) + 1) % ks.length] ?? "batch");
          }}
        >
          排序：{SORTS[sort]} ▾
        </button>
        <button
          type="button"
          className={cn("btn xs", inspector && "pri")}
          onClick={() => setInspector((v) => !v)}
        >
          详情栏 ⌥\
        </button>
      </div>

      {/* 表格 */}
      <div className="f1 fx" style={{ minHeight: 0 }}>
        <div className="col f1" style={{ minWidth: 0 }}>
          <div className="vrowh">
            <span />
            <span />
            <span>编号</span>
            <span>阶段</span>
            <span>模型型号</span>
            <span>即梦</span>
            <span>已等 · 上次查询</span>
            <span style={{ textAlign: "right" }}>额度</span>
            <span>情况 · 判断依据</span>
          </div>

          <div className="sc f1" style={{ minHeight: 0 }}>
            {active.map((s) => (
              <SectionBlock
                key={s.key}
                s={s}
                open={!collapsed.has(s.key)}
                curId={curId}
                sel={sel}
                progress={progress}
                busy={busy}
                onToggle={() =>
                  setCollapsed((c) => {
                    const n = new Set(c);
                    if (n.has(s.key)) n.delete(s.key);
                    else n.add(s.key);
                    return n;
                  })
                }
                onPick={setCur}
                onCheck={(id) =>
                  setSel((old) => {
                    const n = new Set(old);
                    if (n.has(id)) n.delete(id);
                    else n.add(id);
                    return n;
                  })
                }
                onSelectAll={(ids) =>
                  setSel((old) => {
                    const n = new Set(old);
                    const allIn = ids.every((i) => n.has(i));
                    for (const i of ids) {
                      if (allIn) n.delete(i);
                      else n.add(i);
                    }
                    return n;
                  })
                }
                onSubmitBatch={(ids) => void openSubmit(ids)}
                onReviewBatch={(firstId) => {
                  setCur(firstId);
                  setTally({ passed: 0, killed: 0 });
                  setScreen("review");
                }}
                onRerunBatch={(ids) => void requeue(ids, "run")}
                onRewriteGroup={(ids) => void requeue(ids, "rewrite")}
              />
            ))}
            {active.length === 0 && (
              <div className="fs11 t3" style={{ padding: "18px 14px" }}>
                当前筛选下没有进行中的批次。
              </div>
            )}

            {settled.length > 0 && <div className="vdonehd">已定案的批次 · 自动收起，不再占位</div>}
            {settled.map((s) => {
              const open = doneOpen.has(s.key);
              const passed = s.all.filter((r) => r.stage === "pass").length;
              const inLib = s.all.filter((r) => r.clip.inAssetLib).length;
              const toggleDone = () =>
                setDoneOpen((c) => {
                  const n = new Set(c);
                  if (n.has(s.key)) n.delete(s.key);
                  else n.add(s.key);
                  return n;
                });
              return (
                <div key={s.key}>
                  <div
                    className="vdone"
                    onClick={toggleDone}
                    onKeyDown={(e) => e.key === "Enter" && toggleDone()}
                    role="button"
                    tabIndex={0}
                  >
                    <span className="cr">{open ? "▾" : "▸"}</span>
                    <span className="pid">{s.batchId == null ? "历史" : `#${s.batchId}`}</span>
                    <span className="nm">{s.title}</span>
                    <span className="d" />
                    <span className="tl">
                      {s.all.length}/{s.all.length} 已定案 · {passed} 成片 · {inLib} 已入资产库
                    </span>
                  </div>
                  {open &&
                    s.all.map((r) => (
                      <ClipRow
                        key={r.clip.id}
                        r={r}
                        cur={r.clip.id === curId}
                        checked={sel.has(r.clip.id)}
                        status={progress[r.clip.id] ?? r.clip.genStatus ?? ""}
                        onPick={() => setCur(r.clip.id)}
                        onCheck={() =>
                          setSel((old) => {
                            const n = new Set(old);
                            if (n.has(r.clip.id)) n.delete(r.clip.id);
                            else n.add(r.clip.id);
                            return n;
                          })
                        }
                      />
                    ))}
                </div>
              );
            })}
            <div style={{ height: 12 }} />
          </div>

          {/* 底栏：有选择时是批量动作，没选择时是一句提示 + 撤销 */}
          <div className="vfoot">
            {sel.size > 0 ? (
              <>
                <button type="button" className="btn xs" onClick={() => setSel(new Set())}>
                  已选 <b>{sel.size}</b> ✕
                </button>
                <span className="fs11 t3 nowrap">
                  {onlyStage ? `均为${STAGE_META[onlyStage].label}` : "跨阶段"}
                </span>
                {onlyStage === "rev" && (
                  <>
                    <button
                      type="button"
                      className="btn xs okb"
                      disabled={busy}
                      onClick={() => void review([...sel], true)}
                    >
                      通过 <span className="kh">空格</span>
                    </button>
                    <button
                      type="button"
                      className="btn xs dngo"
                      disabled={busy}
                      onClick={() => void review([...sel], false)}
                    >
                      不通过 <span className="kh">X</span>
                    </button>
                  </>
                )}
                {onlyStage === "ready" && (
                  <button
                    type="button"
                    className="btn xs pri"
                    disabled={busy}
                    onClick={() => void openSubmit([...sel])}
                  >
                    <Send className="ic12" />
                    提交 {sel.size} 条
                    {estimateOf(selected) != null && ` · 约 ${estimateOf(selected)} 额度`}{" "}
                    <span className="kh">⌘⏎</span>
                  </button>
                )}
                {onlyStage === "pass" && (
                  <button
                    type="button"
                    className="btn xs pri"
                    disabled={busy}
                    onClick={() => packOrPick([...sel])}
                  >
                    入资产库 <span className="kh">A</span>
                  </button>
                )}
                {selected.some((r) => r.signals.has("timeout")) && (
                  <button
                    type="button"
                    className="btn xs pri"
                    disabled={busy}
                    onClick={() =>
                      void requeue(
                        selected.filter((r) => r.signals.has("timeout")).map((r) => r.clip.id),
                        "wait",
                      )
                    }
                    title="沿用原提交单放回轮询，不重新提交、不再扣额度"
                  >
                    继续等待 {selected.filter((r) => r.signals.has("timeout")).length} 条{" "}
                    <span className="kh">W</span>
                  </button>
                )}
                <button
                  type="button"
                  className="btn xs"
                  disabled={busy}
                  onClick={() => void requeue([...sel], "run")}
                  title="用同一条视频提示词再抽一次（回到待提交，确认后重新扣额度）"
                >
                  重跑 <span className="kh">R</span>
                </button>
                <button
                  type="button"
                  className="btn xs gho"
                  disabled={busy}
                  onClick={() => void requeue([...sel], "rewrite")}
                  title="清掉视频提示词，退回待改写让 skill 重写"
                >
                  退回改写 <span className="kh">E</span>
                </button>
                <button
                  type="button"
                  className="btn xs gho dng"
                  disabled={busy}
                  onClick={() => setConfirmRemove(true)}
                >
                  移出流水线
                </button>
                {firstSignal(selected) && (
                  <button
                    type="button"
                    className="btn xs gho"
                    onClick={() => {
                      const s = firstSignal(selected);
                      if (s) {
                        setSignals([s]);
                        setStage("all");
                      }
                    }}
                  >
                    同类 {signalCount(firstSignal(selected) as SignalKey)} 条
                  </button>
                )}
              </>
            ) : (
              <>
                <span className="fs11 t3 nowrap ohide">
                  {visible.length} 条符合当前筛选 · J/K 移动 · 空格 通过 · X 不通过 · ⏎ 全屏看片
                </span>
                <button type="button" className="btn xs gho" disabled={busy} onClick={pollNow}>
                  查一次进度
                </button>
              </>
            )}
            <div className="f1" />
            {undo && (
              <span className="vundo">
                <span className="ohide">{undo.label}</span>
                <button type="button" onClick={doUndo}>
                  撤销 U
                </button>
              </span>
            )}
          </div>
        </div>

        {inspector && (
          <V2vInspector
            row={curRow}
            posText={revIndex >= 0 ? `${revIndex + 1} / ${revList.length}` : ""}
            showFirstFrame={showFrame}
            busy={busy}
            onToggleFrame={() => setShowFrame((v) => !v)}
            onEnterReview={() => {
              setTally({ passed: 0, killed: 0 });
              setScreen("review");
            }}
            onPass={() => judgeCurrent("pass")}
            onReject={() => judgeCurrent("rej")}
            onRerun={() => judgeCurrent("rerun")}
            onRewrite={() => judgeCurrent("rewrite")}
            onResume={() => judgeCurrent("wait")}
            onPack={() => judgeCurrent("pack")}
          />
        )}
      </div>

      {screen === "review" && (
        <V2vReviewFlow
          list={revList}
          index={revIndex < 0 ? 0 : revIndex}
          passedCount={tally.passed}
          killedCount={tally.killed}
          undoLabel={undo?.label ?? null}
          busy={busy}
          onSeek={setCur}
          onPass={() => judgeCurrent("pass")}
          onReject={() => judgeCurrent("rej")}
          onRerun={() => judgeCurrent("rerun")}
          onRewrite={() => judgeCurrent("rewrite")}
          onPackPass={passAndPack}
          onUndo={doUndo}
          onExit={() => setScreen("list")}
        />
      )}

      {cmdPreview && (
        <SubmitConfirm
          preview={cmdPreview.data}
          busy={busy}
          onClose={() => setCmdPreview(null)}
          onConfirm={doSubmit}
        />
      )}

      {assetPick && (
        <SkuPickModal
          count={assetPick.length}
          onClose={() => setAssetPick(null)}
          onPick={(skuId) => void packInto(assetPick, skuId, assetPick[0])}
        />
      )}

      {showLog && <V2vLogPanel onClose={() => setShowLog(false)} />}
      {showParams && (
        <V2vParamsPanel
          models={models}
          // 只把还没花钱的两列交给批量覆盖：已提交的条目改参数不会重新生效，
          // 却会让详情页显示的参数与那条视频实际用的对不上。
          selectedReady={selected
            .filter((r) => r.stage === "ready" || r.stage === "rewrite")
            .map((r) => r.clip.id)}
          onClose={() => {
            setShowParams(false);
            void unwrap(commands.v2vEffectiveParams())
              .then(setEff)
              .catch(() => {});
          }}
          onApplied={() => {
            setShowParams(false);
            setSel(new Set());
            void load();
          }}
        />
      )}
      {showObserve && (
        <ObserveModal tick={tick} now={now} credit={credit} onClose={() => setShowObserve(false)} />
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
    </div>
  );
}

/** 选中条目的预估额度合计；有一条查不到单价就返回 null（不给半真半假的数）。 */
function estimateOf(rows: Row[]): number | null {
  let sum = 0;
  for (const r of rows) {
    if (r.estimate == null) return null;
    sum += r.estimate;
  }
  return sum;
}

function firstSignal(rows: Row[]): SignalKey | null {
  for (const r of rows) {
    const first = [...r.signals][0];
    if (first) return first;
  }
  return null;
}

/**
 * 页头。心跳、余额、通过率、三个面板入口。
 *
 * 心跳必须在**什么都没发生**时也答得出「轮询器还活着吗」—— 一个静默的界面和一个
 * 卡死的轮询器长得一模一样。
 */
function V2vHeader({
  tick,
  now,
  balance,
  spentDay,
  passRate,
  stale,
  staleSecs,
  onObserve,
  onLog,
  onParams,
}: {
  tick: V2vTick | null;
  now: number;
  balance: number | null;
  spentDay: number | null;
  passRate: number | null;
  stale: boolean;
  staleSecs: number | null;
  onObserve: () => void;
  onLog: () => void;
  onParams: () => void;
}) {
  const ago = tick == null ? null : Math.max(0, now - tick.at);
  // 心跳每 6 秒一次；超过 30 秒没心跳说明循环卡住或应用被挂起了。
  const bad = tick != null && (tick.error != null || (ago ?? 0) > 30);
  return (
    <div className="vhd">
      <span className="ptt">视频流水线</span>
      <span
        className={cn("pollpill", tick && !tick.enabled && "off", bad && "bad")}
        title={
          tick?.error
            ? `上一轮出错：${tick.error}`
            : "后台按已等时长退避轮询（10s→10min），心跳每 6 秒一次；关掉应用不影响已扣额度的任务"
        }
      >
        <span className="dot" />
        {tick == null
          ? "轮询 · 等待首轮"
          : `${tick.enabled ? "轮询中" : "轮询已关"} · ${tick.running} 在跑 · ${fmtAgo(ago ?? 0)}`}
      </span>
      {stale && staleSecs != null && (
        <span
          className="vstalepill"
          title="超 2 小时没有新片落盘 —— 任务不会丢（额度已扣、即梦照跑），但值得看一眼执行日志"
        >
          {fmtDur(staleSecs)} 未出片
        </span>
      )}
      <span className="fs11 t3 nowrap">
        余额 <b className="mono t1">{balance ?? "—"}</b> · 今日{" "}
        <b className="mono t1">{spentDay ?? 0}</b> · 通过率{" "}
        <b className="mono t1">{passRate == null ? "—" : `${passRate}%`}</b>
      </span>
      <div className="f1" />
      <button type="button" className="btn xs gho" onClick={onObserve}>
        <Activity className="ic12" />
        观测 ⌥1
      </button>
      <button type="button" className="btn xs gho" onClick={onLog}>
        <ScrollText className="ic12" />
        日志 ⌥2
      </button>
      <button type="button" className="btn xs gho" onClick={onParams}>
        <SlidersHorizontal className="ic12" />
        参数 ⌥3
      </button>
    </div>
  );
}

/** 一个批次一节。分段条 + 待办摘要 + 就地的批次级动作。 */
function SectionBlock({
  s,
  open,
  curId,
  sel,
  progress,
  busy,
  onToggle,
  onPick,
  onCheck,
  onSelectAll,
  onSubmitBatch,
  onReviewBatch,
  onRerunBatch,
  onRewriteGroup,
}: {
  s: Section;
  open: boolean;
  curId: number | null;
  sel: Set<number>;
  progress: Record<number, string>;
  busy: boolean;
  onToggle: () => void;
  onPick: (id: number) => void;
  onCheck: (id: number) => void;
  onSelectAll: (ids: number[]) => void;
  onSubmitBatch: (ids: number[]) => void;
  onReviewBatch: (firstId: number) => void;
  onRerunBatch: (ids: number[]) => void;
  onRewriteGroup: (ids: number[]) => void;
}) {
  const ready = s.rows.filter((r) => r.stage === "ready");
  const rev = s.rows.filter((r) => r.stage === "rev");
  const fails = s.rows.filter((r) => r.stage === "fail");
  const phantoms = fails.filter((r) => r.signals.has("phantom"));
  const cost = estimateOf(ready);

  // 例外放最前面 —— 与「情况」列同一条规则：被截断时先没的必须是常态，不是例外。
  const parts: string[] = [];
  if (phantoms.length > 0) parts.push(`幽灵 ${phantoms.length}`);
  else if (fails.length > 0) parts.push(`异常 ${fails.length}`);
  if (ready.length > 0) parts.push(`待放行 ${ready.length}`);
  if (rev.length > 0) parts.push(`待验收 ${rev.length}`);

  // 连续毙掉三条以上多半不是「没抽中」，而是这一组的提示词本身有问题 ——
  // 那时该做的是退回改写整组，而不是一条条重跑（每重跑一次都要再花一份钱）。
  const badGroup = worstGroup(s.all);

  return (
    <div>
      <div
        className="vsect"
        onClick={onToggle}
        onKeyDown={(e) => e.key === "Enter" && onToggle()}
        role="button"
        tabIndex={0}
      >
        <span className="cr">{open ? "▾" : "▸"}</span>
        <span className="pid">{s.batchId == null ? "历史" : `#${s.batchId}`}</span>
        <span className="nm" title={s.title}>
          {s.title}
        </span>
        <div className="seg" title={s.legend}>
          {s.seg.map((g) => (
            <div
              key={g.stage}
              style={{ width: `${g.pct}%`, background: STAGE_META[g.stage].seg }}
            />
          ))}
        </div>
        <span className="tl">
          {parts.length > 0 ? parts.join(" · ") : `${s.all.length} 条 · 无待办`}
        </span>
        {s.rows.length > 0 && (
          <button
            type="button"
            className="btn xs gho"
            onClick={(e) => {
              e.stopPropagation();
              onSelectAll(s.rows.map((r) => r.clip.id));
            }}
          >
            全选本节
          </button>
        )}
        {ready.length > 0 && (
          <button
            type="button"
            className="btn xs pri"
            disabled={busy}
            onClick={(e) => {
              e.stopPropagation();
              onSubmitBatch(ready.map((r) => r.clip.id));
            }}
          >
            确认提交 {ready.length} 条{cost != null && ` · 预估 ${cost} 额度`}
          </button>
        )}
        {rev.length > 0 && (
          <button
            type="button"
            className="btn xs"
            onClick={(e) => {
              e.stopPropagation();
              const first = rev[0];
              if (first) onReviewBatch(first.clip.id);
            }}
          >
            看片流 {rev.length}
          </button>
        )}
        {rev.length === 0 && phantoms.length > 0 && (
          <button
            type="button"
            className="btn xs"
            disabled={busy}
            onClick={(e) => {
              e.stopPropagation();
              onRerunBatch(phantoms.map((r) => r.clip.id));
            }}
            title="幽灵单从未计费，重跑不花钱"
          >
            全部重跑 {phantoms.length} 条
          </button>
        )}
      </div>

      {open && badGroup && (
        <div className="vnote">
          <span>
            「{badGroup.name}」这一组已有 {badGroup.rejected} 条不通过 ——
            多半不是没抽中，而是提示词本身有问题。
          </span>
          <div className="f1" />
          <button
            type="button"
            className="btn xs"
            disabled={busy}
            onClick={() => onRewriteGroup(badGroup.ids)}
          >
            退回改写整组
          </button>
        </div>
      )}

      {open &&
        s.rows.map((r) => (
          <ClipRow
            key={r.clip.id}
            r={r}
            cur={r.clip.id === curId}
            checked={sel.has(r.clip.id)}
            status={progress[r.clip.id] ?? r.clip.genStatus ?? ""}
            onPick={() => onPick(r.clip.id)}
            onCheck={() => onCheck(r.clip.id)}
          />
        ))}
      {open && s.rows.length === 0 && <div className="vsempty">当前筛选下这一批没有条目</div>}
    </div>
  );
}

/** 本批里毙得最狠的那个组（≥3 条不通过才报）。返回可退回改写的条目 id。 */
function worstGroup(rows: Row[]): { name: string; rejected: number; ids: number[] } | null {
  const byGroup = new Map<string, Row[]>();
  for (const r of rows) {
    const k = r.clip.groupName || "未分组";
    const b = byGroup.get(k);
    if (b) b.push(r);
    else byGroup.set(k, [r]);
  }
  let best: { name: string; rejected: number; ids: number[] } | null = null;
  for (const [name, list] of byGroup) {
    const rejected = list.filter((r) => r.stage === "rej").length;
    if (rejected < 3) continue;
    if (best && best.rejected >= rejected) continue;
    best = {
      name,
      rejected,
      // pass 的不动（`requeue_for_rewrite` 也拒），已经在改写队列里的重发一次无害。
      ids: list.filter((r) => r.stage !== "pass").map((r) => r.clip.id),
    };
  }
  return best;
}

function ClipRow({
  r,
  cur,
  checked,
  status,
  onPick,
  onCheck,
}: {
  r: Row;
  cur: boolean;
  checked: boolean;
  status: string;
  onPick: () => void;
  onCheck: () => void;
}) {
  const c = r.clip;
  const meta = STAGE_META[r.stage];
  const thumb = assetSrc(c.posterPath ?? c.thumbPath);
  const jimeng =
    r.stage === "run"
      ? status || "等待首次查询"
      : r.stage === "rev" || r.stage === "pass" || r.stage === "rej"
        ? status || "Finish"
        : r.stage === "fail"
          ? (c.genStatus ?? "—")
          : "—";
  return (
    <div
      className={cn("vrow", cur && "cur", checked && "sel")}
      onClick={onPick}
      onKeyDown={(e) => e.key === "Enter" && onPick()}
      role="button"
      tabIndex={-1}
    >
      <span
        onClick={(e) => {
          e.stopPropagation();
          onCheck();
        }}
        onKeyDown={(e) => {
          if (e.key === " ") {
            e.stopPropagation();
            onCheck();
          }
        }}
        role="checkbox"
        aria-checked={checked}
        tabIndex={-1}
      >
        <span className={cn("vbox", checked && "on")}>{checked ? "✓" : ""}</span>
      </span>
      {thumb ? <img className="vrth" src={thumb} alt="" /> : <span className="vrth" />}
      <span className="pid nowrap">
        {c.promptCode}
        {c.attempt > 1 && <span style={{ color: "var(--wr2)" }}> ·{c.attempt}</span>}
      </span>
      <span className="vstagec" style={{ color: meta.fg }}>
        <span className="d" style={{ background: meta.fg }} />
        {meta.label}
      </span>
      <span
        className={cn("vmodel", r.vip && "vip")}
        title={
          r.modelFull == null
            ? "跟随 CLI 默认（设置里没指定模型）"
            : `${r.modelFull}${r.estimate == null ? " · 单价未实测" : ` · 约 ${r.estimate} 额度/条`}`
        }
      >
        {r.modelShort}
      </span>
      <span className="mono fs10 t2 nowrap ohide">{jimeng}</span>
      <span className={cn("mono fs10 nowrap ohide", r.slow || r.stage === "fail" ? "wr2" : "t2")}>
        {r.waitSecs === 0
          ? "—"
          : `${fmtDur(r.waitSecs)}${r.polledAgo != null ? ` · ${fmtDur(r.polledAgo)}前` : ""}`}
      </span>
      <span
        className={cn("mono fs10", r.vip ? "wr2" : "t2")}
        style={{ textAlign: "right", opacity: r.creditEstimated ? 0.6 : 1 }}
        title={r.creditEstimated ? "预估值（还没收到扣费回执）" : undefined}
      >
        {r.credit == null ? "—" : r.credit}
      </span>
      <span className={cn("fs11 nowrap ohide", toneClass(r.situationTone))}>{r.situation}</span>
    </div>
  );
}

function toneClass(t: Row["situationTone"]): string {
  return t === "er" ? "terr" : t === "wr" ? "wr2" : t === "acc" ? "acc2" : "t3";
}

/** 提交确认卡：真实命令行 + 这一下要花多少额度，全摆在按钮之前。 */
function SubmitConfirm({
  preview,
  busy,
  onClose,
  onConfirm,
}: {
  preview: SubmitPreview;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const short = preview.estimatedCredits;
  return (
    <Modal
      title="确认提交到即梦"
      width="w700"
      onClose={onClose}
      headerExtra={<span className="chip">{preview.commands.length} 条</span>}
      footer={
        <>
          <span className="fs12" style={{ fontWeight: 600 }}>
            预计消耗 {preview.unpriced.length > 0 ? "≥ " : ""}
            {short} 额度
          </span>
          <span className="fs11 t3">提交即扣费，无法撤回</span>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn sm pri" disabled={busy} onClick={onConfirm}>
            <Send className="ic12" />
            确认提交
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        <div className="costbar mb8">
          <div className="fs12">
            <b>{preview.commands.length}</b> 条 · 预计消耗{" "}
            <b>
              {preview.unpriced.length > 0 ? "≥ " : ""}
              {short}
            </b>{" "}
            额度
            {preview.balance !== null && (
              <>
                ｜余额 <b>{preview.balance}</b> → 提交后约 <b>{preview.balance - short}</b>
              </>
            )}
          </div>
          {preview.unpriced.length > 0 && (
            <div className="twarn">
              {preview.unpriced.join("、")} 没实测过单价，未计入 —— 实际只会更高。
            </div>
          )}
          {preview.balance !== null && preview.balance < short && (
            <div className="terr">
              余额不足：即梦逐条扣费，会提交到一半开始报错，而前面扣掉的退不回来。
            </div>
          )}
        </div>
        <div className="fs12 t2 mb8" style={{ lineHeight: 1.7 }}>
          下面是<b>即将执行的完整命令行</b>（与真正 exec 的参数同源）。
          「我设了却没生效」这类怀疑只能靠把真实请求摆在确认之前来消除。
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
          {preview.commands.map((line, i) => (
            <div key={`${i}-${line.slice(0, 24)}`} className="cmdwell">
              {line}
            </div>
          ))}
        </div>
      </div>
    </Modal>
  );
}

/** 观测面板（⌥1）：队列进度 + 心跳 + 额度分账。 */
function ObserveModal({
  tick,
  now,
  credit,
  onClose,
}: {
  tick: V2vTick | null;
  now: number;
  credit: CreditStats | null;
  onClose: () => void;
}) {
  return (
    <Modal
      title="队列观测"
      width="w700"
      onClose={onClose}
      headerExtra={
        <span className="chip">
          {tick == null
            ? "等待首轮心跳"
            : `${tick.running} 在跑 · ${fmtAgo(Math.max(0, now - tick.at))}`}
        </span>
      }
      footer={
        <>
          <span className="fs11 t3">
            即梦不回传排队位次，故这里给的是我们自己测得准的两件事：出片间隔与逐小时趋势。
          </span>
          <div className="f1" />
          <button type="button" className="btn sm pri" onClick={onClose}>
            完成
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        <V2vQueuePanel tick={tick} now={now} always />
        <div className="statgrid mt10">
          <Stat label="账户余额" value={credit?.balance == null ? "—" : String(credit.balance)} />
          <Stat label="累计已用" value={String(credit?.spentTotal ?? 0)} />
          <Stat label="近 7 天" value={String(credit?.spentWeek ?? 0)} />
          <Stat label="近 24 小时" value={String(credit?.spentDay ?? 0)} />
        </div>
        <div className="statgrid mt8">
          <Stat label="成片（值回票价）" value={String(credit?.spentPass ?? 0)} tone="ok" />
          <Stat label="未通过（白花的）" value={String(credit?.spentRej ?? 0)} tone="er" />
          <Stat label="待验收（未定论）" value={String(credit?.spentPending ?? 0)} />
          <Stat label="计入条数" value={String(credit?.countedClips ?? 0)} />
        </div>
        {credit?.balanceError && (
          <div className="fs11 mt8" style={{ color: "var(--wr)", lineHeight: 1.7 }}>
            查不到余额：{credit.balanceError}
          </div>
        )}
      </div>
    </Modal>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "ok" | "er" }) {
  return (
    <div className="statcell">
      <div className="fs10 t3 nowrap ohide">{label}</div>
      <div
        className="fs16 fw6"
        style={{ color: tone === "ok" ? "var(--ok)" : tone === "er" ? "var(--er)" : undefined }}
      >
        {value}
      </div>
    </div>
  );
}

/** 成片 → 视频型素材包的 SKU 选择。 */
function SkuPickModal({
  count,
  onClose,
  onPick,
}: {
  count: number;
  onClose: () => void;
  onPick: (skuId: number) => void;
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
            <div
              key={s.id}
              className="pickrow"
              onClick={() => onPick(s.id)}
              onKeyDown={(e) => e.key === "Enter" && onPick(s.id)}
              role="button"
              tabIndex={0}
            >
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
