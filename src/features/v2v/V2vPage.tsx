import { ConfirmModal, Modal } from "@/components/ui/Modal";
import { V2vInspector } from "@/features/v2v/V2vInspector";
import { V2vLogPanel } from "@/features/v2v/V2vLogPanel";
import { type Params, V2vParamPicker } from "@/features/v2v/V2vParamPicker";
import { V2vParamsPanel } from "@/features/v2v/V2vParamsPanel";
import { V2vQueuePanel } from "@/features/v2v/V2vQueuePanel";
import { V2vCreditDaily, V2vQueueTrend } from "@/features/v2v/V2vQueueTrend";
import { V2vReviewFlow } from "@/features/v2v/V2vReviewFlow";
import {
  ACTION_CHIPS,
  ACTION_META,
  type ActionFilter,
  type Row,
  SIGNAL_CHIPS,
  SORTS,
  STAGE_META,
  type Section,
  type SignalKey,
  type SortKey,
  type Stage,
  buildSections,
  deriveRows,
  fmtAgo,
  fmtDur,
  fmtSpan,
  isLive,
  matchAction,
  matchQuery,
  sortRows,
} from "@/features/v2v/model";
import { assetSrc } from "@/lib/img";
import {
  type AutofillStatus,
  type AwayDigest,
  type ClipView,
  type CreditStats,
  type EffectiveParams,
  type HandoffStatus,
  type ModelInfo,
  type QueueStats,
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
  Film,
  FolderOpen,
  PenLine,
  RefreshCw,
  ScrollText,
  Send,
  SlidersHorizontal,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

/**
 * 视频流水线工作台。
 *
 * ## 主轴是「下一步动作」，不是阶段（v0.22.0）
 *
 * v0.19.0 把看板换成分节表格是对的，但轴选错了：它按**阶段**组织，而阶段恰恰是
 * 最不缺的信息 —— 它就写在每一条脸上。于是 21 条待改写会同时显示
 * 「需要我 0」「待改写 21」「无待办」「等 skill 写回 · 交接已物化」四句互相矛盾的话，
 * 而真相是那 21 条**正卡在人身上**：工单早已物化好，在等人去 Claude Code 跑改写。
 * 全流水线最大的一处阻塞，界面上说的是「无待办」。
 *
 * 现在筛选片直接就是动作：处理异常 / 去改写 / 待放行 / 待验收 / 等即梦。派生在
 * `model.ts` 的 `nextAction`，故筛选、节头摘要、行内色点三者同源。顺带修掉一个真 bug：
 * 幽灵单只存在于 `run`，而旧的「需要我」不含 `run` —— 唯一该**免费**重跑的那一类
 * 被默认筛选整个藏了起来。
 *
 * ## 另外两条仍然成立的判断
 *
 * - **信号是与动作正交的例外轴**：18 条幽灵单和 18 条正常排队长得一模一样，
 *   而处置完全相反（一个免费重跑，一个必须继续等否则重复扣费）。
 * - **详情栏常驻**：「这一条花没花钱」放不进一行，而开弹窗一条条看太慢。
 *
 * ## 键盘
 *
 * ←/→（或 ↑/↓、J/K）移动 · 空格 通过 · X 不通过 · R 重跑 · E 退回改写 · W 继续等待 ·
 * U 撤销 · F 对照首帧 · ⏎ 全屏看片 · ⌘⏎ 确认提交 · ⌥\ 详情栏 · ⌥1/2/3 观测/日志/参数。
 */
/** 轮询事件带回来的实时进度（`v2v://progress` 的载荷，去掉 clipId）。 */
type LiveProgress = { genStatus: string; queueIdx: number | null; polledAt: number };

export function V2vPage() {
  // ── 数据 ─────────────────────────────────────────────
  const [clips, setClips] = useState<ClipView[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [eff, setEff] = useState<EffectiveParams | null>(null);
  const [credit, setCredit] = useState<CreditStats | null>(null);
  const [queue, setQueue] = useState<QueueStats | null>(null);
  const [handoff, setHandoff] = useState<HandoffStatus | null>(null);
  const [auto, setAuto] = useState<AutofillStatus | null>(null);
  const [digest, setDigest] = useState<AwayDigest | null>(null);
  const [tick, setTick] = useState<V2vTick | null>(null);
  /** 轮询刚问到的实时进度，按 clip id。库里那份要等下一次 `listV2vClips` 才更新。 */
  const [progress, setProgress] = useState<Record<number, LiveProgress>>({});
  /** 「几秒前」要自己走字，否则一个静止的「12 秒前」比没有还误导。 */
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  // ── 筛选与选择 ───────────────────────────────────────
  const [action, setAction] = useState<ActionFilter>("mine");
  const [signals, setSignals] = useState<SignalKey[]>([]);
  const [sort, setSort] = useState<SortKey>("batch");
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<Set<number>>(new Set());
  const [cur, setCur] = useState<number | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  // ── 界面态 ───────────────────────────────────────────
  const [inspector, setInspector] = useState(true);
  const [screen, setScreen] = useState<"list" | "review">("list");
  const [showFrame, setShowFrame] = useState(false);
  const [bannerOpen, setBannerOpen] = useState(false);
  const [showLog, setShowLog] = useState(false);
  const [showParams, setShowParams] = useState(false);
  const [showObserve, setShowObserve] = useState(false);
  const [cmdPreview, setCmdPreview] = useState<{ ids: number[]; data: SubmitPreview } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [bulk, setBulk] = useState<Params>({
    modelVersion: "",
    duration: null,
    videoResolution: "",
  });
  const [busy, setBusy] = useState(false);
  /** 撤销令牌由 Rust 造，前端只当信封（见 `V2vAction` 的注释）。 */
  const [undo, setUndo] = useState<{ label: string; entries: V2vUndoEntry[] } | null>(null);
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
    // 常驻队列的状态跟着每次事件刷新：它会在无人操作时自己变（补单、被日限挡住）。
    void unwrap(commands.v2vAutofillStatus())
      .then(setAuto)
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
      // 位次一起收下。原来这里只取 `genStatus`，把 `queueIdx` 整个丢掉了 ——
      // 于是轮询刚问到的新位次要等下一次 `listV2vClips` 才看得见，
      // 而这两件事之间隔着整整一轮（非 VIP 600 秒）。
      onProgress: (e) =>
        setProgress((c) => ({
          ...c,
          [e.clipId]: { genStatus: e.genStatus, queueIdx: e.queueIdx, polledAt: e.polledAt },
        })),
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
  // 喂给 `deriveRows` 的秒表**量化到 30 秒**（照 V2vClipsPage 的先例）。
  // 它要遍历全表、逐行重算判据与情况文案，原样喂 1 秒秒表等于每秒重算一整页；
  // 而它用到的时间全是「已等多久 / 多久前查过」这类以分钟为单位读的值。
  // 顶部那个心跳 pill 与倒计时仍读未量化的 `now`，那才是真需要每秒走字的地方。
  const coarseNow = Math.floor(now / 30) * 30;
  const rows = useMemo(
    () => deriveRows(clips, models, eff, coarseNow, queue?.inFlightLimit ?? 1),
    [clips, models, eff, coarseNow],
  );
  const byId = useMemo(() => new Map(rows.map((r) => [r.clip.id, r])), [rows]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = rows.filter(
      (r) => matchAction(r, action) && signals.every((s) => r.signals.has(s)) && matchQuery(r, q),
    );
    return sortRows(filtered, sort);
  }, [rows, action, signals, sort, query]);

  const sections = useMemo(() => buildSections(rows, visible), [rows, visible]);

  const actionCount = useCallback(
    (k: ActionFilter) => rows.filter((r) => matchAction(r, k)).length,
    [rows],
  );
  /** 待改写条数 —— 「去改写」召唤横幅与交接对账都读它。 */
  const rewriteN = useMemo(() => rows.filter((r) => r.action === "rewrite").length, [rows]);

  // 交接状态跟着**待改写条数**刷新。
  //
  // 它原来只在 mount 与手动「收录改写」之后取一次，而 watcher 会在无人操作时自己收录
  // —— 于是横幅上那个「21 条」在改写落地之后还挂着，恰好在它最要紧的时候是错的。
  // 不挂在每个 tick 上：`v2v_handoff_status` 会顺手重写工单，不是只读的。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是「待改写条数变了」这个信号
  useEffect(() => {
    void unwrap(commands.v2vHandoffStatus())
      .then(setHandoff)
      .catch(() => {});
  }, [rewriteN]);
  // 信号只数**在制**的：成片已经去了成片库，把它们算进「重跑过 12 条」这种数字里，
  // 点下去却一条都筛不出来 —— 一个点了没反应的筛选片比没有更糟。
  const signalCount = useCallback(
    (k: SignalKey) => rows.filter((r) => isLive(r.stage) && r.signals.has(k)).length,
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
  // 只有还没花钱的两列改参数才有意义：已提交的改了不会重新生效，
  // 却会让详情栏显示的参数与那条视频实际用的对不上。
  const editableRows = useMemo(
    () => selected.filter((r) => r.stage === "ready" || r.stage === "rewrite"),
    [selected],
  );
  const editableParams = useMemo(() => editableRows.map((r) => r.clip.id), [editableRows]);

  /**
   * 选中项当前**自己写死**的参数是否一致（读 clip 自己的覆写，不读逐级回落后的结果）。
   *
   * 这决定参数条的初值，而初值这件事有代价：`set_v2v_clip_params` 的 `None` 是
   * **清空**不是**保持**。参数条若一律以空值开场，选中一批已经设过 vip/1080p 的条目、
   * 只想改个时长，按下「应用」就会把模型和分辨率一起抹掉。
   */
  const selParams = useMemo(() => {
    const key = (r: Row) =>
      `${r.clip.modelVersion ?? ""}|${r.clip.duration ?? ""}|${r.clip.videoResolution ?? ""}`;
    const first = editableRows[0];
    if (!first) return { mixed: false, value: null as Params | null };
    const uniform = editableRows.every((r) => key(r) === key(first));
    return {
      mixed: !uniform,
      value: uniform
        ? {
            modelVersion: first.clip.modelVersion ?? "",
            duration: first.clip.duration,
            videoResolution: first.clip.videoResolution ?? "",
          }
        : null,
    };
  }, [editableRows]);
  const bulkMixed = selParams.mixed;

  // 选择一变就把参数条重置到选中项的现状（不一致时留空，并在条上标「多条不一致」）。
  // 用 key 而不是对象引用做依赖：selected 每秒都会因为 `now` 走字而重新构造。
  const selKey = editableParams.join(",");
  // biome-ignore lint/correctness/useExhaustiveDependencies: 依赖的是「选了哪几条」，见下
  useEffect(() => {
    setBulk(selParams.value ?? { modelVersion: "", duration: null, videoResolution: "" });
    // selParams 每次渲染都是新对象；真正该触发重置的是「选了哪几条」。
  }, [selKey]);

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
        // 已经放行、正在本地队列里等空位的不算 —— 对它们再点一次确认什么也不会发生，
        // 而确认卡会把它们算进「这一批 N 条 · 预估 M 额度」，两个数字当场就不准了。
        const ready = ids.filter((id) => {
          const c = byId.get(id);
          return c?.clip.stage === "ready" && c.clip.submitQueuedAt == null;
        });
        if (ready.length === 0) {
          toast("请先选中「待放行」的条目（排队中的已经放行过了，不必再点）");
          return;
        }
        setCmdPreview({ ids: ready, data: await unwrap(commands.previewV2vCommands(ready)) });
      }),
    [guard, byId],
  );

  /**
   * 把一套参数写进指定条目。
   *
   * `refreshPreview` 在提交确认卡里为真 —— 改完参数必须**当场重取命令行与预估额度**，
   * 否则那张卡会继续摆着上一套参数算出来的数字，而人正要照着它按下确认。
   */
  const applyParams = useCallback(
    (ids: number[], p: Params, refreshPreview: boolean) =>
      guard(async () => {
        if (ids.length === 0) return;
        const n = await unwrap(
          commands.setV2vClipParams(
            ids,
            p.modelVersion.trim() === "" ? null : p.modelVersion.trim(),
            p.duration,
            p.videoResolution.trim() === "" ? null : p.videoResolution.trim(),
          ),
        );
        toast.success(n > 0 ? `已改写 ${n} 条的生成参数` : "没有可改参数的条目（已提交的改不动）");
        await load();
        if (refreshPreview) {
          setCmdPreview({ ids, data: await unwrap(commands.previewV2vCommands(ids)) });
        }
      }),
    [guard, load],
  );

  const doSubmit = useCallback(() => {
    const p = cmdPreview;
    if (!p) return;
    void guard(async () => {
      const sum = await unwrap(commands.submitV2vClips(p.ids));
      if (sum.submitted > 0) {
        toast.success(
          sum.queued > 0
            ? `已提交 ${sum.submitted} 条到即梦 · 另 ${sum.queued} 条在本地排队，出一条自动补一条`
            : `已提交 ${sum.submitted} 条到即梦`,
        );
      } else if (sum.queued > 0) {
        // 即梦已经跑满时一条都发不出去 —— 这不是失败，但必须说清楚，
        // 否则点了确认什么都没发生。
        toast(`即梦已跑满，${sum.queued} 条已排进本地队列，有空位就自动发`);
      }
      if (sum.failed > 0) toast.error(`${sum.failed} 条提交失败：${sum.firstError ?? ""}`);
      setCmdPreview(null);
      setSel(new Set());
      await load();
    });
  }, [cmdPreview, guard, load]);

  /** 撤回放行：本地队列 → 等你点确认提交。没发出去所以不涉及钱。 */
  const unqueue = useCallback(
    (ids: number[]) =>
      guard(async () => {
        const n = await unwrap(commands.unqueueV2vClips(ids));
        toast(n > 0 ? `已撤回放行 ${n} 条（未产生额度消耗）` : "这些条目已经发出去了，撤不回来");
        setSel(new Set());
        await load();
      }),
    [guard, load],
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
    (kind: "pass" | "rej" | "rerun" | "rewrite" | "wait") => {
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
      } else {
        void requeue([id], "wait", id);
      }
    },
    [curRow, review, requeue],
  );

  const move = useCallback(
    (d: 1 | -1) => {
      if (visible.length === 0) return;
      const at = curId == null ? -1 : visible.findIndex((r) => r.clip.id === curId);
      const next = visible[Math.max(0, Math.min(visible.length - 1, (at < 0 ? 0 : at) + d))];
      if (next) setCur(next.clip.id);
    },
    [visible, curId],
  );

  const modalOpen = cmdPreview != null || confirmRemove || showLog || showParams || showObserve;

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
      // 四个方向键**全部**是「换一条」。
      //
      // 看片流底部那条胶片条是横向的，于是 ←/→ 才是这里最顺手的换片方向 —— 而它们
      // 原先唯一的绑定在播放条那个 `tabIndex={-1}` 的 slider 上（`V2vVideo`），
      // 永远拿不到焦点，等于没绑。逐帧仍留在播放条的按钮上：判形变是停下来慢慢看的事，
      // 与「一秒一条地过片」不是同一种节奏，不该抢同一组键。
      if (e.key === "j" || e.key === "J" || e.key === "ArrowDown" || e.key === "ArrowRight") {
        e.preventDefault();
        move(1);
        return;
      }
      if (e.key === "k" || e.key === "K" || e.key === "ArrowUp" || e.key === "ArrowLeft") {
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
  }, [screen, modalOpen, move, judgeCurrent, doUndo, curRow, curId, sel, openSubmit]);

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
          queue={queue}
          auto={auto}
          passCount={0}
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
          <V2vParamsPanel models={models} queue={queue} onClose={() => setShowParams(false)} />
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
        queue={queue}
        auto={auto}
        passCount={passN}
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
                setAction("review");
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

      {/* 「去改写」召唤 —— 全流水线最大的一处阻塞，它只可能由人推动。
          不可关闭：关得掉的 CTA 会立刻退化回「21 条躺在那儿没人知道该干嘛」。 */}
      {rewriteN > 0 && (
        <RewriteCall
          n={rewriteN}
          handoff={handoff}
          now={now}
          busy={busy}
          onIngest={ingest}
          onOnly={() => setAction("rewrite")}
        />
      )}

      {/* 主轴：下一步动作 */}
      <div className="vfilt">
        <input
          className="inp sm"
          style={{ width: 168 }}
          placeholder="编号 / 组 / 提示词…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {ACTION_CHIPS.map((c) => {
          const dot =
            c.key === "mine" || c.key === "all"
              ? null
              : c.key === "rej"
                ? STAGE_META.rej.seg
                : ACTION_META[c.key].dot;
          return (
            <button
              key={c.key}
              type="button"
              className={cn("vchip", action === c.key && "on")}
              onClick={() => setAction(c.key)}
            >
              {dot && <span className="d" style={{ background: dot }} />}
              {c.label}
              <span className="n">{actionCount(c.key)}</span>
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
        {/* 交接状态与「收录改写」都搬进了上面那条召唤横幅 —— 它们是同一件事的两半，
            而摆在这里既没人看见，又把这一行在 1140px 下挤到换行。 */}
        <div className="f1" />
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
          {/* 表头在滚动容器**内部** sticky。留在外面时那条 10px 的经典滚动条只吃
              表体的宽度，同一套 grid 模板于是算出两组列位 —— 那正是「列对不齐」。 */}
          <div className="vtbody">
            <div className="vgrid th">
              <span />
              <span />
              <span>编号</span>
              <span>模型型号</span>
              <span>已等</span>
              <span style={{ textAlign: "right" }}>额度</span>
              <span>情况 · 下一步</span>
            </div>
            {sections.map((s) => (
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
            {sections.length === 0 && (
              <div className="vclear">
                <div className="fs13 fw5 t2">工作台已清空</div>
                <div className="fs11 t3" style={{ lineHeight: 1.8, maxWidth: 460 }}>
                  当前筛选下没有还在制的批次 —— 全部定案的批次会**整节消失**，不再折叠占位。
                  {passN > 0 && (
                    <>
                      {" "}
                      {passN} 条成片在
                      <button
                        type="button"
                        className="lnk"
                        onClick={() => useUiStore.getState().go("clips")}
                      >
                        视频成片
                      </button>
                      页。
                    </>
                  )}
                </div>
              </div>
            )}
            <div style={{ height: 12 }} />
          </div>

          {/* 参数条：选中还没提交的条目就直接出现，**不再藏在一个「参数 N」开关后面**。
              参数是每一批不一样的东西（模型之间差 5.5 倍），放进全局设置等于每换一批
              都去设置页改一次；藏在开关后面则等于没有。 */}
          {editableParams.length > 0 && (
            <div className="parambar foot">
              <span className="fs11 fw6 t3 nowrap">
                改这 {editableParams.length} 条的模型 / 时长 / 分辨率
              </span>
              {bulkMixed && (
                <span className="fs10 t3 nowrap" title="选中的条目当前参数不一致，留空表示不覆盖">
                  多条不一致
                </span>
              )}
              <V2vParamPicker
                models={models}
                value={bulk}
                onChange={setBulk}
                disabled={busy}
                compact
              />
              <div className="f1" />
              <button
                type="button"
                className="btn xs pri"
                disabled={busy}
                onClick={() => void applyParams(editableParams, bulk, false)}
              >
                应用到这 {editableParams.length} 条
              </button>
            </div>
          )}

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
                {selected.some((r) => r.action === "submit") && (
                  <button
                    type="button"
                    className="btn xs pri"
                    disabled={busy}
                    onClick={() => void openSubmit([...sel])}
                  >
                    <Send className="ic12" />
                    提交 {selected.filter((r) => r.action === "submit").length} 条
                    {estimateOf(selected) != null && ` · 约 ${estimateOf(selected)} 额度`}{" "}
                    <span className="kh">⌘⏎</span>
                  </button>
                )}
                {selected.some((r) => r.action === "queued") && (
                  <button
                    type="button"
                    className="btn xs gho"
                    disabled={busy}
                    onClick={() =>
                      void unqueue(
                        selected.filter((r) => r.action === "queued").map((r) => r.clip.id),
                      )
                    }
                    title="它们还没发出去、一分钱没扣 —— 撤回后退回「等你点确认提交」"
                  >
                    撤回放行 {selected.filter((r) => r.action === "queued").length} 条
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
                        setAction("all");
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
                  {visible.length} 条符合当前筛选 · ←/→ 换条 · 空格 通过 · X 不通过 · ⏎ 全屏看片
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
            models={models}
            onApplyParams={(id, p) => void applyParams([id], p, false)}
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
          onUndo={doUndo}
          onExit={() => setScreen("list")}
        />
      )}

      {cmdPreview && (
        <SubmitConfirm
          preview={cmdPreview.data}
          ids={cmdPreview.ids}
          models={models}
          queue={queue}
          busy={busy}
          onApplyParams={(p) => applyParams(cmdPreview.ids, p, true)}
          onClose={() => setCmdPreview(null)}
          onConfirm={doSubmit}
        />
      )}

      {showLog && <V2vLogPanel onClose={() => setShowLog(false)} />}
      {showParams && (
        <V2vParamsPanel
          models={models}
          queue={queue}
          onClose={() => {
            setShowParams(false);
            void unwrap(commands.v2vEffectiveParams())
              .then(setEff)
              .catch(() => {});
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
          desc="只移除视频流水线里的条目；对应的作品与已交付到输出目录的成片文件都不受影响。之后仍可在作品库手动重新加入。"
          confirmLabel="移除"
          danger
          onConfirm={remove}
          onClose={() => setConfirmRemove(false)}
        />
      )}
    </div>
  );
}

/**
 * 「去改写」召唤。
 *
 * 待改写这一步**只可能由人推动** —— 它在 Claude Code / Codex 里做，而 GenDesk 这边
 * 工单早已物化好、什么都不缺。此前界面上关于它的全部表达是三句黑话
 * （「无待办」「等 skill 写回」「交接已物化」）散落在三个地方，加起来还是答不出
 * 「我该去哪儿干什么」。这里把答案摆成一句话加三个按钮。
 *
 * **不可关闭**：关得掉的 CTA 会立刻退化回「21 条躺在那儿没人知道该干嘛」。
 * 它在 `rewriteN` 归零时自己消失，那才是它该消失的时刻。
 */
function RewriteCall({
  n,
  handoff,
  now,
  busy,
  onIngest,
  onOnly,
}: {
  n: number;
  handoff: HandoffStatus | null;
  now: number;
  busy: boolean;
  onIngest: () => void;
  onOnly: () => void;
}) {
  const err = handoff?.error ?? null;
  // 工单条数与待改写条数对不上，是「收了一半」唯一的可见症状 —— 在此之前没有任何
  // 一处会说这件事，而它的后果是有几条永远不会被改写。
  const mismatch = err == null && handoff != null && handoff.items !== n;
  return (
    <div className={cn("vcall", err && "er")}>
      <span className="ttl">
        <PenLine className="ic12" style={{ verticalAlign: "-2px", marginRight: 4 }} />
        {n} 条等你改写
      </span>
      <div className="ds f1">
        {err ? (
          <>工单没能写出去：{err} —— 先确认交接目录还在、可写。</>
        ) : (
          <>
            工单已写到交接目录
            {handoff && `（${handoff.groups} 组 · ${handoff.items} 条）`}， 去 Claude Code 或 Codex
            里跑 <b>v2v-rewrite</b>，写完回来点「收录改写结果」。
            {mismatch && (
              <span style={{ color: "var(--wr2)" }}>
                {" "}
                工单里 {handoff?.items} 条、流水线里 {n} 条待改写 —— 点收录对一次账。
              </span>
            )}
            {handoff?.lastIngestAt != null && (
              <span className="pth"> · 上次收录 {fmtAgo(now - handoff.lastIngestAt)}</span>
            )}
          </>
        )}
      </div>
      <button
        type="button"
        className="btn xs"
        onClick={() => void unwrap(commands.openHandoffDir()).catch((e) => toast.error(String(e)))}
        title={handoff?.pendingDir}
      >
        <FolderOpen className="ic12" />
        打开交接目录
      </button>
      <button type="button" className="btn xs pri" disabled={busy} onClick={onIngest}>
        <RefreshCw className="ic12" />
        写完了 · 收录改写结果
      </button>
      <button type="button" className="btn xs gho" onClick={onOnly}>
        只看这 {n} 条
      </button>
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
  queue,
  auto,
  passCount,
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
  queue: QueueStats | null;
  auto: AutofillStatus | null;
  passCount: number;
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
            : "后台整表扫描即梦（含 VIP 5 分钟一次、全非 VIP 10 分钟一次），心跳每 6 秒一次；关掉应用不影响已扣额度的任务"
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
      <QueuePill queue={queue} onOpen={onParams} />
      <AutofillPill auto={auto} onOpen={onParams} />
      <span className="fs11 t3 nowrap">
        余额 <b className="mono t1">{balance ?? "—"}</b> · 今日{" "}
        <b className="mono t1">{spentDay ?? 0}</b> · 通过率{" "}
        <b className="mono t1">{passRate == null ? "—" : `${passRate}%`}</b>
      </span>
      <div className="f1" />
      {passCount > 0 && (
        <button
          type="button"
          className="btn xs gho"
          onClick={() => useUiStore.getState().go("clips")}
          title="验收通过的视频已经不是流水线的事了，它们在成片库那一页"
        >
          <Film className="ic12" />
          成片 {passCount}
        </button>
      )}
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

/**
 * 在跑上限与本地待发队列的 pill（0028）。
 *
 * 它回答的是这一版最核心的那个问题：**「我放行了 9 条，为什么只跑 1 条」**。
 * 在此之前界面上没有任何一处提到过并发上限的存在，于是 9 条一起砸向即梦、
 * 8 条被 `ExceedConcurrencyLimit` 弹回来判死，而人只看到「8 个错误」。
 */
function QueuePill({ queue, onOpen }: { queue: QueueStats | null; onOpen: () => void }) {
  if (!queue) return null;
  const { running, inFlightLimit, queued, observedLimit } = queue;
  if (running === 0 && queued === 0) return null;
  return (
    <button
      type="button"
      className={cn("autopill", queued > 0 && "idle")}
      onClick={onOpen}
      title={[
        `即梦同一时间只跑得下 ${inFlightLimit} 条（账户级并发上限）。`,
        observedLimit != null
          ? "这个数是本次运行实测出来的：再多发即梦会以 ExceedConcurrencyLimit 拒收。"
          : "可在参数面板里调整。",
        queued > 0
          ? `另有 ${queued} 条已放行、正排在本地等空位，出一条自动补一条 —— 不必再点提交。`
          : "",
      ].join("")}
    >
      <span className="dot" />
      即梦 {running}/{inFlightLimit}
      {queued > 0 && ` · 本地排队 ${queued}`}
    </button>
  );
}

/**
 * 常驻队列的 pill。
 *
 * 它要答的是**「开着」与「在跑」的差别** —— 没料了、日限满了、余额不够都会让这条
 * 队列安静地停下来，而一条停摆的常驻队列与一条正常运转的在界面上长得一模一样。
 * 所以停因必须写在脸上，而不是等人去翻日志。
 */
function AutofillPill({ auto, onOpen }: { auto: AutofillStatus | null; onOpen: () => void }) {
  if (!auto?.enabled) return null;
  const bad = auto.error != null;
  const idle = auto.running < auto.depth;
  return (
    <button
      type="button"
      className={cn("autopill", bad && "bad", !bad && idle && "idle")}
      onClick={onOpen}
      title={
        auto.error ??
        `常驻的非 VIP 队列：保持 ${auto.depth} 条在跑，完成一条补一条。` +
          `位子与你手动放行的那些共用（即梦的并发上限是账户级的），所以「在跑 ${auto.running}」数的是全部。` +
          `今日已提交 ${auto.spentToday}${auto.dailyCredits > 0 ? `/${auto.dailyCredits}` : ""} 额度。`
      }
    >
      <span className="dot" />
      常驻补单 {auto.running}/{auto.depth}
      {bad ? " · 配置有误" : auto.blocked ? ` · ${auto.blocked}` : ` · 存量 ${auto.stock}`}
    </button>
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
  progress: Record<number, LiveProgress>;
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
  // 「待放行」与「已放行、在本地排队」都是 `ready` 阶段，但按钮只该管前者 ——
  // 对已经放过行的再点一次「确认提交」什么也不会发生，而按钮上写着 9 条。
  const ready = s.rows.filter((r) => r.action === "submit");
  const queuedRows = s.rows.filter((r) => r.action === "queued");
  const rev = s.rows.filter((r) => r.stage === "rev");
  const fails = s.rows.filter((r) => r.stage === "fail");
  const phantoms = fails.filter((r) => r.signals.has("phantom"));
  const cost = estimateOf(ready);
  const rewrite = s.rows.filter((r) => r.stage === "rewrite");

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
        {/* 一句人话，取代原来那条无图例的分段条。 */}
        <span className={cn("tl", s.headlineTone !== "t3" && s.headlineTone)}>{s.headline}</span>
        {rewrite.length > 0 && (
          <button
            type="button"
            className="btn xs gho"
            onClick={(e) => {
              e.stopPropagation();
              void unwrap(commands.openHandoffDir()).catch((err) => toast.error(String(err)));
            }}
            title="工单已写好，去 Claude Code / Codex 里跑 v2v-rewrite"
          >
            <FolderOpen className="ic12" />
            改写 {rewrite.length}
          </button>
        )}
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
        {queuedRows.length > 0 && (
          <span className="fs11 t3 nowrap" title="已放行、正排在本地等即梦的空位，出一条自动补一条">
            排队中 {queuedRows.length}
          </span>
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
            status={progress[r.clip.id]?.genStatus ?? r.clip.genStatus ?? ""}
            onPick={() => onPick(r.clip.id)}
            onCheck={() => onCheck(r.clip.id)}
          />
        ))}
      {/* 「当前筛选下这一批没有条目」那句提示没了，因为这一节现在根本不会出现 ——
          `buildSections` 里空节整节消失。留个空壳节头不回答任何问题，只会把真正
          命中的那一节挤下去（几十批之后就是整屏的空壳）。 */}
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
  const act = ACTION_META[r.action];
  const thumb = assetSrc(c.posterPath ?? c.thumbPath);
  // 即梦原文没有自己的列了（对非 run 行恒为 —，对 run 行的同一事实已经在「情况」里），
  // 但它仍是排障时唯一的一手证据 —— 挂到色点的 title 上，不占轨道宽度。
  const jimeng = r.stage === "run" ? status || "等待首次查询" : (c.genStatus ?? "");
  return (
    <div
      className={cn("vgrid tr", cur && "cur", checked && "sel")}
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
      <span
        className={cn("mono fs10 nowrap ohide", r.slow || r.stage === "fail" ? "wr2" : "t2")}
        title={r.polledAgo == null ? undefined : `上次查询 ${fmtDur(r.polledAgo)}前`}
      >
        {r.waitSecs === 0 ? "—" : fmtDur(r.waitSecs)}
      </span>
      <span
        className={cn("mono fs10", r.vip ? "wr2" : "t2")}
        style={{ textAlign: "right", opacity: r.creditEstimated ? 0.6 : 1 }}
        title={r.creditEstimated ? "预估值（还没收到扣费回执）" : undefined}
      >
        {r.credit == null ? "—" : r.credit}
      </span>
      {/* 阶段色点吸进「情况」这一格：它是同一句话的两半，而独占一列要 72px，
          恰恰是「情况」列被裁掉的那部分宽度。 */}
      <span
        className={cn("vsit", toneClass(r.situationTone))}
        title={`${meta.label} → ${act.label}${jimeng ? ` · 即梦 ${jimeng}` : ""}`}
      >
        <span className="d" style={{ background: act.dot }} />
        <span className="nowrap ohide">{r.situation}</span>
      </span>
    </div>
  );
}

function toneClass(t: Row["situationTone"]): string {
  return t === "er" ? "terr" : t === "wr" ? "wr2" : t === "acc" ? "acc2" : "t3";
}

/**
 * 提交确认卡：真实命令行 + 这一下要花多少额度 + **就地改参数**，全摆在按钮之前。
 *
 * 参数编辑放在这里而不是设置页，是因为「提交前」正是唯一一个人一定会看的时刻，
 * 也是唯一一个改了还来得及的时刻 —— 提交即扣费，之后再改只影响下一批。
 * 改完当场重算这一批要花多少：模型之间差 5.5 倍，那个数字必须随选择一起变，
 * 否则「改了参数」与「这一下花多少钱」还是两件对不上的事。
 *
 * ## 这张卡上的两处「别让人干等」
 *
 * 1. **余额自己去取**（`v2vBalance`）。它原来长在 `previewV2vCommands` 里，于是点
 *    「提交」之后整个界面要等一次 CLI 网络往返才看得到这张卡 —— 期间一声不吭，
 *    人以为没点上。现在卡先出来，余额那一格显示「读取中…」。
 * 2. **提交过程有进度**。后端本来就在逐条记 `submit` 阶段的执行日志（「提交中 i/total」），
 *    但那些字只有开执行日志才看得到，而这张卡此刻正挡着它。所以直接订阅同一条流，
 *    把最新一句摆在按钮旁边 —— 不新增事件，也就不会与日志分叉。
 */
function SubmitConfirm({
  preview,
  ids,
  models,
  queue,
  busy,
  onApplyParams,
  onClose,
  onConfirm,
}: {
  preview: SubmitPreview;
  ids: number[];
  models: ModelInfo[];
  /** 在跑上限与当前占用 —— 这一批里有几条会当场发出去，由它决定。 */
  queue: QueueStats | null;
  busy: boolean;
  /** 把参数写进这一批条目并重取预览。 */
  onApplyParams: (p: Params) => void;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const short = preview.estimatedCredits;
  /** `undefined` = 还在读；`null` = 读不到（掉线/未登录，不拦人）。 */
  const [balance, setBalance] = useState<number | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    void unwrap(commands.v2vBalance())
      .then((b) => alive && setBalance(b))
      .catch(() => alive && setBalance(null));
    return () => {
      alive = false;
    };
  }, []);

  // 提交进度：订阅执行日志里 `submit` 阶段的那条流。
  const [step, setStep] = useState<string | null>(null);
  useEffect(() => {
    if (!busy) return;
    let un: (() => void) | undefined;
    void subscribeV2v({
      onActivity: (e) => {
        if (e.entry.phase === "submit") setStep(e.entry.message);
      },
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, [busy]);

  // 秒表：这里要回答的是「它还在动吗」，而后端两条日志之间可能隔着一整次网络往返。
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (!busy) {
      setElapsed(0);
      return;
    }
    const t = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(t);
  }, [busy]);
  // 即梦同时只跑得下这么多条，其余留在本地排队自动接上。**这个数必须出现在按下确认
  // 之前**：这一版之前，选 9 条点确认得到的是「已提交 9 条」，而实际只有 1 条入队、
  // 8 条被即梦以 ExceedConcurrencyLimit 弹回来判死 —— 界面从头到尾没提过有这个上限。
  const limit = queue?.inFlightLimit ?? 1;
  const goesNow = Math.max(0, Math.min(preview.commands.length, limit - (queue?.running ?? 0)));
  const waits = preview.commands.length - goesNow;
  const [p, setP] = useState<Params>({ modelVersion: "", duration: null, videoResolution: "" });
  const [dirty, setDirty] = useState(false);
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
          <span className="fs11 t3">
            {waits > 0 ? `先发 ${goesNow} 条 · 其余排队时才扣费` : "提交即扣费，无法撤回"}
          </span>
          <div className="f1" />
          {/* 提交进行中：这一行是「它还在动吗」的全部答案。取消按钮此刻让位给它 ——
              发出去的那一条撤不回来，一个点了没用的「取消」只会让人更慌。 */}
          {busy && (
            <span className="subprog">
              <RefreshCw className="ic12 spin" />
              <span className="ohide nowrap">{step ?? "正在提交…"}</span>
              <span className="mono nowrap">{elapsed}s</span>
            </span>
          )}
          {!busy && (
            <button type="button" className="btn sm gho" onClick={onClose}>
              取消
            </button>
          )}
          <button
            type="button"
            className="btn sm pri"
            disabled={busy || dirty}
            title={dirty ? "参数改了还没应用 —— 先点「应用到这 N 条」" : undefined}
            onClick={onConfirm}
          >
            <Send className="ic12" />
            {busy ? "提交中…" : "确认提交"}
          </button>
        </>
      }
    >
      <div style={{ padding: 4 }}>
        {/* 参数条放在最上面：它是这张卡里唯一还能改的东西，而下面那串命令行是它的结果。 */}
        <div className="parambar mb8">
          <span className="fs11 fw6 t3 nowrap">这一批的参数</span>
          <V2vParamPicker
            models={models}
            value={p}
            disabled={busy}
            onChange={(v) => {
              setP(v);
              setDirty(true);
            }}
          />
          <div className="f1" />
          <button
            type="button"
            className={cn("btn xs", dirty && "pri")}
            disabled={busy || !dirty}
            onClick={() => {
              onApplyParams(p);
              setDirty(false);
            }}
          >
            应用到这 {ids.length} 条
          </button>
        </div>
        <div className="costbar mb8">
          <div className="fs12">
            <b>{preview.commands.length}</b> 条 · 预计消耗{" "}
            <b>
              {preview.unpriced.length > 0 ? "≥ " : ""}
              {short}
            </b>{" "}
            额度
            {balance === undefined && <span className="t3">｜余额读取中…</span>}
            {balance !== undefined && balance !== null && (
              <>
                ｜余额 <b>{balance}</b> → 提交后约 <b>{balance - short}</b>
              </>
            )}
          </div>
          {preview.unpriced.length > 0 && (
            <div className="twarn">
              {preview.unpriced.join("、")} 没实测过单价，未计入 —— 实际只会更高。
            </div>
          )}
          {balance != null && balance < short && (
            <div className="terr">
              余额不足：即梦逐条扣费，会提交到一半开始报错，而前面扣掉的退不回来。
            </div>
          )}
        </div>
        <div className="costbar mb8">
          <div className="fs12">
            即梦同一时间只跑得下 <b>{limit}</b> 条
            {(queue?.running ?? 0) > 0 && `（现在已占 ${queue?.running}）`}，所以这一批{" "}
            <b>先发 {goesNow} 条</b>
            {waits > 0 && (
              <>
                ，其余 <b>{waits} 条排在本地</b>，出一条自动补一条 —— 不必再来点一次
              </>
            )}
            。
          </div>
          <div className="fs11 t3">
            排队的那些<b>还没扣费</b>：额度是在真正发出去的那一刻扣的，所以下面那个预估是
            这一批全部跑完的总数，不是现在就要花掉的。
          </div>
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
            排队位次按轮询节拍采样落库，保留 30 天；额度快照一天一条。
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

        {/* 排产用的两块。放在出片统计**之前**：那些是「已经花掉的」，
            而这两块是「下一批什么时候发」——后者才是打开这个面板时要决定的事。 */}
        <div className="vsec mt10">非 VIP 排队观测（近 7 天）</div>
        <div className="mt5">
          <V2vQueueTrend hours={7 * 24} />
        </div>

        <div className="vsec mt10">每日额度（近 14 天）</div>
        <div className="mt5">
          <V2vCreditDaily />
        </div>

        <div className="vsec mt10">额度分账</div>
        <div className="statgrid mt5">
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
