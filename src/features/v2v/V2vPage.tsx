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
  carryParams,
  creditPerSec,
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
  type V2vRefresh,
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

/**
 * 换通道要写下去的一套参数。
 *
 * 与 `Params` 的区别是 `duration` **不可为 null** —— 即梦只接受「三者都不给」或
 * 「一套完整组合」，而换通道属于后者。用 `Required<Params>` 顶不了这个：它只去掉
 * 可选性，`number | null` 里的 null 照样留着。
 */
type ChannelParams = { modelVersion: string; duration: number; videoResolution: string };

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
  /**
   * 手动刷新的实时进度。
   *
   * 手动刷新是 O(n) 个 `query_result` 进程，几十条要跑几十秒 —— 没有这份状态，
   * 那段时间里按钮点下去毫无反应，而人点它的场景恰恰是「我怀疑它卡住了」。
   */
  const [refresh, setRefresh] = useState<V2vRefresh | null>(null);
  /** 轮询刚问到的实时进度，按 clip id。库里那份要等下一次 `listV2vClips` 才更新。 */
  const [progress, setProgress] = useState<Record<number, LiveProgress>>({});
  /** 「几秒前」要自己走字，否则一个静止的「12 秒前」比没有还误导。 */
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  // ── 筛选与选择 ───────────────────────────────────────
  const [action, setAction] = useState<ActionFilter>("mine");
  const [signals, setSignals] = useState<SignalKey[]>([]);
  const [sort, setSort] = useState<SortKey>("wait");
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
  /** 换通道面板要处置的行。 */
  const [switching, setSwitching] = useState<Row[] | null>(null);
  /**
   * 重跑前的确认：只在选中里**确实有已扣费的在跑条目**时才出现。
   *
   * 幽灵单与未计费的在跑条目不弹 —— 它们重跑本来就免费，多一次确认只会训练出
   * 盲点头的习惯，等真正要花钱的那次弹出来时就没人读了。
   */
  const [confirmRerun, setConfirmRerun] = useState<{
    ids: number[];
    paid: number;
    credit: number;
    advanceFrom?: number;
  } | null>(null);
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
      // 刷新跑完了才重取列表：中途每查一条就 `listV2vClips` 一次，等于把一次刷新变成
      // n 次全表查询。行内的位次不会因此滞后 —— `onProgress` 已经把它逐条送过来了。
      onRefresh: (e) => {
        setRefresh(e);
        if (!e.active) {
          if (e.error) toast.error(`刷新出错：${e.error}`);
          else toast(e.finished > 0 ? `取回 ${e.finished} 条成片` : "已刷新，暂无新出片");
          void load();
        }
      },
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
  // 上限按通道下发（0031）。依赖的是「通道构成变了」这个信号而不是 queue 对象本身：
  // 后者每次心跳都是新对象，挂上去等于每 6 秒重算一整页。
  const limitKey = (queue?.channels ?? []).map((c) => `${c.modelVersion}:${c.limit}`).join(",");
  // biome-ignore lint/correctness/useExhaustiveDependencies: 见上，依赖的是通道构成这个信号
  const limits = useMemo(
    () => new Map((queue?.channels ?? []).map((c) => [c.modelVersion, c.limit])),
    [limitKey],
  );
  const rows = useMemo(
    () => deriveRows(clips, models, eff, coarseNow, limits),
    [clips, models, eff, coarseNow, limits],
  );
  const byId = useMemo(() => new Map(rows.map((r) => [r.clip.id, r])), [rows]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = rows.filter(
      (r) => matchAction(r, action) && signals.every((s) => r.signals.has(s)) && matchQuery(r, q),
    );
    return sortRows(filtered, sort);
  }, [rows, action, signals, sort, query]);

  const sections = useMemo(() => buildSections(rows, visible, models), [rows, visible, models]);

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

  /**
   * 重跑的入口 —— 已扣费的在跑条目先弹确认。
   *
   * `requeue_for_run` 会把 `submit_id` 与 `credit_count` 一起清掉，此后 `list_running`
   * 再也认不出那一单：即梦还在跑、钱已经扣了，片子却永远取不回来，下次提交是第二份钱。
   * 而底栏按钮与 `R` 键此前对**任何阶段**都直接放行，代价提示是在动作**完成之后**
   * 才弹的 —— 那时已经晚了。
   *
   * 判据用 Rust 下发的 `clip.billed`（`Evidence::billed`，读五处证据），不在这里拿
   * `creditCount != null` 凑：前端只看得见其中两个字段，少读一处的后果是对着一条
   * 已经扣过钱的单子说「重跑不花钱」。没扣过费的（幽灵单、被并发上限弹回的）
   * **不弹** —— 它们重跑本来就免费。
   */
  const rerun = useCallback(
    (ids: number[], advanceFrom?: number) => {
      const paid = ids
        .map((id) => byId.get(id))
        .filter((r): r is Row => r != null && r.stage === "run" && r.clip.billed);
      if (paid.length === 0) {
        void requeue(ids, "run", advanceFrom);
        return;
      }
      setConfirmRerun({
        ids,
        paid: paid.length,
        credit: paid.reduce((a, r) => a + (r.clip.creditCount ?? r.clip.submitCredit ?? 0), 0),
        ...(advanceFrom == null ? {} : { advanceFrom }),
      });
    },
    [byId, requeue],
  );

  /** 换通道。免费的那些与要丢弃提交单的那些，由面板上的复选框分开。 */
  const switchChannel = useCallback(
    (ids: number[], p: ChannelParams, abandon: boolean) =>
      guard(async () => {
        const res = await unwrap(
          commands.switchV2vChannel(ids, p.modelVersion, p.duration, p.videoResolution, abandon),
        );
        setSwitching(null);
        if (res.switched + res.abandoned === 0) {
          toast("这些条目当前阶段不允许换通道");
          return;
        }
        // 撤销令牌只含被丢弃的那些（纯参数改动撤不回来，换回去本来就免费）——
        // 所以没有丢弃时不摆那个「撤销」pill，免得人以为它能把通道换回去。
        if (res.undo.length > 0) setUndo({ label: res.label, entries: res.undo });
        toast(
          res.abandoned > 0
            ? `${res.label}（丢弃 ${res.abandoned} 条已提交单，其中已扣 ${res.abandonedCredit} 额度）`
            : `${res.label}（还没提交出去，未产生额度消耗）`,
        );
        setSel(new Set());
        await load();
      }),
    [guard, load],
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
        // 而确认卡会把它们算进「共 N 条 · 预估 M 额度」，两个数字当场就不准了。
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

  /**
   * 立刻问一遍即梦。
   *
   * **不走 `guard`**：那把锁是给「会改状态、不能连点」的动作用的，而刷新要跑几十秒，
   * 用它锁住整页等于刷新期间什么都干不了。命令本身立刻返回（活儿在 Rust 后台），
   * 重入由 Rust 侧的 `REFRESHING` 闸挡，界面这边只把按钮置灰。
   */
  const pollNow = useCallback(() => {
    void unwrap(commands.pollV2vNow())
      .then((n) => {
        // **进度一律只从事件来**，不在这里乐观地写一个 `active: true`。
        // Rust 在 spawn 之前就发了第一帧，而命令返回值走的是另一条通道 —— 一轮很快的
        // 刷新（在跑 0 条）完全可能先收到终帧、再收到这个 `.then()`，那样写下去就是
        // 把已经结束的那一轮复活成「正在刷新」，按钮从此一直转下去。
        if (n === 0) toast("即梦手上没有在跑的条目 —— 本地队列里那些它还不知道");
      })
      .catch((e) => {
        if (e instanceof Error) toast.error(e.message);
      });
  }, []);

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
        // `R` 键与按钮走同一条路 —— 护栏若只挂在按钮上，最快的那个入口就是敞开的。
        rerun([id], id);
      } else if (kind === "rewrite") {
        void requeue([id], "rewrite", id);
      } else {
        void requeue([id], "wait", id);
      }
    },
    [curRow, review, requeue, rerun],
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
          refresh={refresh}
          now={now}
          balance={credit?.balance ?? null}
          spentDay={credit?.spentDay ?? null}
          passRate={passRate}
          stale={false}
          staleSecs={null}
          queue={queue}
          auto={auto}
          passCount={0}
          onRefresh={pollNow}
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
        refresh={refresh}
        now={now}
        balance={credit?.balance ?? null}
        spentDay={credit?.spentDay ?? null}
        passRate={passRate}
        stale={stale}
        staleSecs={staleSecs}
        queue={queue}
        auto={auto}
        passCount={passN}
        onRefresh={pollNow}
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
            setSort((s) => ks[(ks.indexOf(s) + 1) % ks.length] ?? "wait");
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
                onRerunBatch={(ids) => rerun(ids)}
                onSwitchChannel={(ids) =>
                  setSwitching(ids.map((id) => byId.get(id)).filter((r): r is Row => r != null))
                }
                onRewriteGroup={(ids) => void requeue(ids, "rewrite")}
              />
            ))}
            {sections.length === 0 && (
              <div className="vclear">
                <div className="fs13 fw5 t2">工作台已清空</div>
                <div className="fs11 t3" style={{ lineHeight: 1.8, maxWidth: 460 }}>
                  当前筛选下没有还在制的条目 —— 一条通道上的活全部定案后，那一节会**整节
                  消失**，不再折叠占位。
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
                {/* 换通道排在重跑**前面**：想「换条快队」时该点的是它，而重跑对已提交
                    的条目会丢弃一份已付费的任务。此前这一格只有重跑，于是那件免费的事
                    只能靠一个会花第二份钱的按钮去做。 */}
                {selected.some((r) => r.stage === "ready" || r.stage === "rewrite") && (
                  <button
                    type="button"
                    className="btn xs"
                    disabled={busy}
                    onClick={() => setSwitching(selected)}
                    title="改投到另一条即梦队列。还在本地排队的换起来一分钱不花 —— 即梦对它们一无所知。"
                  >
                    换通道
                  </button>
                )}
                <button
                  type="button"
                  className="btn xs"
                  disabled={busy}
                  onClick={() => rerun([...sel])}
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
              // 「查一次进度」搬到了顶栏（`RefreshButton`）：它原来只在**没有勾选任何
              // 条目**时才出现，而人最想立刻查一遍的时刻，恰恰是手里正攥着一批选中的
              // 条目、拿不准它们跑到哪了的时候。
              <span className="fs11 t3 nowrap ohide">
                {visible.length} 条符合当前筛选 · ←/→ 换条 · 空格 通过 · X 不通过 · ⏎ 全屏看片
              </span>
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

      {switching && switching.length > 0 && (
        <ChannelSwitchModal
          rows={switching}
          models={models}
          autofillOn={auto?.enabled === true}
          busy={busy}
          onClose={() => setSwitching(null)}
          // 「改投并去提交」在这里串，而不是塞进 `switchChannel` 里 ——
          // 它要调 `openSubmit`，而那个 hook 定义在 `switchChannel` 之后（依赖 `byId`）。
          // 接的是**提交确认卡**而不是直接放行：提交即扣费且不可撤回，而这张卡通篇在说
          // 「换通道免费」，在它后面直接放行等于让人在一张写着「不花钱」的卡上花掉钱。
          onConfirm={(p, abandon, andSubmit) => {
            const ids = switching.map((r) => r.clip.id);
            const holding = switching.filter((r) => r.action === "submit").map((r) => r.clip.id);
            void switchChannel(ids, p, abandon).then(() => {
              if (andSubmit && holding.length > 0) void openSubmit(holding);
            });
          }}
        />
      )}

      {confirmRerun && (
        <ConfirmModal
          title={`重跑 ${confirmRerun.ids.length} 条 · 其中 ${confirmRerun.paid} 条已经花过钱`}
          desc={`这 ${confirmRerun.paid} 条即梦已经收下并扣了 ${confirmRerun.credit} 额度，且不可撤回。重跑会丢弃原提交单 —— 那几条视频即梦还在跑，但我们此后再也取不回来，下次确认提交是第二份钱。想换条队而不是重抽的话，用「换通道」。`}
          confirmLabel="仍要重跑"
          danger
          onConfirm={() => {
            const c = confirmRerun;
            setConfirmRerun(null);
            void requeue(c.ids, "run", c.advanceFrom);
          }}
          onClose={() => setConfirmRerun(null)}
        />
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
 * 顶栏那个「刷新」—— 同一个控件回答两件事：**数据有多新** 与 **现在就去问一遍**。
 *
 * ## 它取代的那颗胶囊为什么必须消失
 *
 * 原来这里写的是「轮询中 · 2 在跑 · 3 秒前」。那个「3 秒前」是**心跳**（6 秒一次、
 * 纯内存读），不是「3 秒前问过即梦」—— 真正的查询是 5/10 分钟一次。两个时刻差着一个
 * 数量级，而胶囊把慢的那个藏起来、把快的那个摆出来，于是它最擅长的事就是让人相信
 * 屏幕上的位次和状态是新鲜的。这一格现在读 `tick.lastSweepAt`（`runner::last_sweep_at`，
 * 真实查询时刻），并且可以点。
 *
 * 「N 在跑」也不在脸上了：旁边那排通道状态灯已经**逐通道**答了同一个问题，而求和成
 * 一个数恰恰是 0031 刚拆掉的那种表达。它挪进了 tooltip。
 *
 * ## 四种状态
 *
 * 刷新中（转圈，写「正在查 k/n」）· 后台轮询已关 · 循环卡住/上一轮出错（红）· 空闲。
 * 「循环卡住」仍按心跳判（超过 30 秒没心跳），因为那问的是**后台循环还活着吗**，
 * 与「上次查询多久前」是两件事 —— 关掉轮询开关时前者正常、后者会一直老下去。
 */
function RefreshButton({
  busy,
  bad,
  off,
  sweptAgo,
  done,
  total,
  running,
  beat,
  error,
  onClick,
}: {
  busy: boolean;
  bad: boolean;
  off: boolean;
  sweptAgo: number | null;
  done: number;
  total: number;
  running: number;
  beat: number | null;
  error: string | null;
  onClick: () => void;
}) {
  const label = busy
    ? total > 0
      ? `正在查 ${done}/${total}`
      : "正在刷新"
    : sweptAgo == null
      ? "刷新 · 还没查过"
      : `刷新 · 上次查询 ${fmtAgo(sweptAgo)}`;
  return (
    <button
      type="button"
      className={cn("refbtn", busy && "busy", off && "off", bad && "bad")}
      disabled={busy}
      onClick={onClick}
      title={[
        "点一下立刻逐条问一遍即梦：队列位次、生成状态、扣费额度、已出的片，全部现取。",
        `即梦手上 ${running} 条${running > 0 ? "（本地队列里那些即梦还不知道，问不到）" : ""}`,
        off
          ? "后台轮询开关是关的 —— 不影响手动刷新，也不影响已扣额度的任务"
          : "后台自己也在扫（含 VIP 5 分钟一次、全非 VIP 10 分钟一次）",
        beat != null && beat > 30 ? `后台已 ${fmtAgo(beat)}没有心跳` : null,
        error ? `上一轮出错：${error}` : null,
      ]
        .filter(Boolean)
        .join("\n")}
    >
      <span className="dot" />
      {label}
    </button>
  );
}

/**
 * 页头。刷新按钮、余额、通过率、三个面板入口。
 */
function V2vHeader({
  tick,
  refresh,
  now,
  balance,
  spentDay,
  passRate,
  stale,
  staleSecs,
  queue,
  auto,
  passCount,
  onRefresh,
  onObserve,
  onLog,
  onParams,
}: {
  tick: V2vTick | null;
  refresh: V2vRefresh | null;
  now: number;
  balance: number | null;
  spentDay: number | null;
  passRate: number | null;
  stale: boolean;
  staleSecs: number | null;
  queue: QueueStats | null;
  auto: AutofillStatus | null;
  passCount: number;
  onRefresh: () => void;
  onObserve: () => void;
  onLog: () => void;
  onParams: () => void;
}) {
  const beat = tick == null ? null : Math.max(0, now - tick.at);
  // 心跳每 6 秒一次；超过 30 秒没心跳说明循环卡住或应用被挂起了。
  const bad = tick != null && (tick.error != null || (beat ?? 0) > 30);
  const busy = refresh?.active === true;
  const swept = tick?.lastSweepAt ?? null;
  return (
    <div className="vhd">
      <span className="ptt">视频流水线</span>
      <RefreshButton
        busy={busy}
        bad={bad}
        off={tick != null && !tick.enabled}
        sweptAgo={swept == null ? null : Math.max(0, now - swept)}
        done={refresh?.done ?? 0}
        total={refresh?.total ?? 0}
        running={tick?.running ?? 0}
        beat={beat}
        error={tick?.error ?? refresh?.error ?? null}
        onClick={onRefresh}
      />
      {stale && staleSecs != null && (
        <span
          className="vstalepill"
          title="超 2 小时没有新片落盘 —— 任务不会丢（额度已扣、即梦照跑），但值得看一眼执行日志"
        >
          {fmtDur(staleSecs)} 未出片
        </span>
      )}
      <ChannelPills queue={queue} auto={auto} onOpen={onParams} />
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
 * 通道状态灯（0031）—— 顶部那排 pill，一条通道一格。
 *
 * ## 为什么不能再是「即梦 1/1 · 本地排队 78」
 *
 * 那个写法把六条互不相干的队列压成了一个数，而**每一位都是错的**。即梦按模型通道
 * 各排各的队 —— `query_result` 回体里 `queue_info.debug_info.dreamina_matrix_queue_name`
 * 逐通道不同（1.5pro `..._video35_pro_i2v_720p` / 2.0 `..._video40_pro` /
 * 2.0mini `..._video40_mini` / 2.0_vip `..._video40_pro_vision`），
 * 2026-07-27 五条不同通道的单子同时下出去全部被收下并计费，一条 `ExceedConcurrencyLimit`
 * 都没有。于是「1/1」既不是任何一条通道的真实占用，也答不出那 6 条 2.0mini 为什么不走
 * —— 而真相是它们本来就该走，是我们自己按一个账户级的假上限把它们锁住了。
 *
 * ## 每格要答的两个问题
 *
 * 1. **远端此刻在替我做什么**。有排队位次就报位次（「前方排队 6233」—— 非 VIP 通道
 *    实测能排到六千多位，那才是「还要等多久」唯一有意义的信号）；问不到位次而确实
 *    有在跑的，就报「任务中 N」。**绝不把两者混成一个数**，也绝不拿 0 冒充位次
 *    （回体里的 0 意思是「已出队」）。
 * 2. **本地还压着多少条同通道的**。「本地队列」只数已放行、随时会自己发出去的那些；
 *    还等着人点确认的另算（写在 title 里）—— 两者的下一步动作完全不同。
 *
 * ## 一条通道一个胶囊，闲着的一律不出现
 *
 * 显示判据是「这条通道上还有没有没走完的事」= 远端在跑 **或** 本地压着队。两者都为 0
 * 的通道在日常里纯是噪音：它没在动，也没有需要人处理的东西。顶栏位置有限，
 * 留给真的还有账要算的那几条。
 *
 * ## 排版：数字带色，标签不带
 *
 * 这一格里三类信息的重要性差着量级：**数字**（6233 / 78 / 1）是要读的，**通道名**是要
 * 认的，**「前方排队」这些词**只是给数字贴个名。整条染成一个颜色、一个字号，等于把
 * 三者压成同一层 —— 扫一眼什么都抓不住。
 *
 * 所以颜色只落在数字上，且**三个数各是一个颜色**，因为它们是三件不同的事：
 *
 * | 数字 | 颜色 | 它在说什么 |
 * |---|---|---|
 * | 前方排队 | 蓝 | 队还没轮到我，这个数要往下掉 |
 * | 任务中   | 绿 | 即梦此刻真的在生成 |
 * | 本地队列 | 黄 | 还没发出去、也还没花钱的存量 |
 *
 * 标签词一律淡灰、通道名深色粗体 —— 三层字重加三个色，一眼就能在六个胶囊里找到
 * 「哪条在跑、哪条还压着货」。
 */
function ChannelPills({
  queue,
  auto,
  onOpen,
}: {
  queue: QueueStats | null;
  auto: AutofillStatus | null;
  onOpen: () => void;
}) {
  // 远端没在跑、本地也没压着队 = 这条通道没有任何还没走完的事，不显示（见上）。
  const channels = (queue?.channels ?? []).filter((c) => c.running > 0 || c.queued > 0);
  if (channels.length === 0) return null;

  return (
    <>
      {channels.map((c) => {
        const queueing = c.frontQueueIdx != null && c.frontQueueIdx > 0;
        const live = c.running > 0;
        // 常驻队列只写进悬停说明，**不在顶栏另占一格**：它是「谁放行的」这条元信息，
        // 与「这条通道现在什么状况」不是一个问题，挤在同一排会把后者稀释掉。
        const mine = auto?.enabled === true && c.autofill;
        return (
          <button
            key={c.modelVersion || "(default)"}
            type="button"
            className={cn("chpill", !live && "idle")}
            onClick={onOpen}
            title={[
              `${c.label} 通道（${c.modelVersion || "设置里没指定型号，实际通道由 CLI 挑"}）。`,
              "即梦按模型通道各排各的队 —— 这条排满了，别的通道照样发得出去。",
              queueing
                ? `\n远端：最靠前那一单排在第 ${c.frontQueueIdx} 位。`
                : live
                  ? `\n远端：${c.running} 条在生成中（还没问到排队位次）。`
                  : "\n远端：这条通道上暂时没有在跑的任务。",
              c.oldestWait > 0 ? `最久那条已等 ${fmtSpan(c.oldestWait)}。` : "",
              `\n本地：${c.queued} 条已放行、正等这条通道的空位（出一条自动补一条，不必再点提交）`,
              c.ready > 0 ? `；另有 ${c.ready} 条还等着你点「确认提交」。` : "。",
              `\n同时在跑上限 ${c.limit} 条`,
              c.observedLimit != null
                ? "（本次运行实测出来的：再多发即梦会以 ExceedConcurrencyLimit 拒收）。"
                : "（可在参数面板里调整）。",
              mine
                ? `\n常驻队列配在这条通道上：目标 ${auto?.depth} 条在跑，其中 ${c.autoRunning} 条是它放的。${
                    auto?.blocked ? `当前停在「${auto.blocked}」。` : ""
                  }`
                : "",
            ].join("")}
          >
            <span className="dot" />
            {/* VIP 不另挂标签：`short_label` 已经把它写进名字里（「2.0Fast VIP」），
                再挂一个「VIP」小牌子就是同一件事说两遍。 */}
            <span className="nm">{c.label}</span>
            {queueing ? (
              <>
                <span className="k">前方排队</span>
                <span className="n nque">{c.frontQueueIdx}</span>
              </>
            ) : (
              live && (
                <>
                  <span className="k">任务中</span>
                  <span className="n nrun">{c.running}</span>
                </>
              )
            )}
            {c.queued > 0 && (
              <>
                <span className="k">本地队列</span>
                <span className="n nloc">{c.queued}</span>
              </>
            )}
          </button>
        );
      })}
    </>
  );
}

/** 一条即梦通道一节。待办摘要 + 就地的整条通道级动作。 */
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
  onSwitchChannel,
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
  onSwitchChannel: (ids: number[]) => void;
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
        {/* 一节 = 一条即梦队列。这个 chip 是它的身份，也是「全选本节 → 换通道」
            那个动作的对象 —— 所以它必须是通道名，不能是一个跨着几条队的批次号。 */}
        <span className="pid" title={s.key === "" ? "设置里没写默认型号，走 CLI 默认" : s.key}>
          {s.label}
        </span>
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
            本地排队 {queuedRows.length}
          </span>
        )}
        {/* 「整条通道改投」—— 一节现在就是一条队，所以这是一个完整动作。
            只算本地那些：它们即梦一无所知，改起来一分钱不花；已提交的那几条要不要
            一起丢，在面板里单独勾。 */}
        {queuedRows.length + ready.length > 0 && (
          <button
            type="button"
            className="btn xs gho"
            disabled={busy}
            onClick={(e) => {
              e.stopPropagation();
              onSwitchChannel([...queuedRows, ...ready].map((r) => r.clip.id));
            }}
            title="把这条通道上还没发出去的条目改投到别的队 —— 它们还在本地，换通道不花钱"
          >
            改投 {queuedRows.length + ready.length} 条
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
            status={progress[r.clip.id]?.genStatus ?? r.clip.genStatus ?? ""}
            onPick={() => onPick(r.clip.id)}
            onCheck={() => onCheck(r.clip.id)}
          />
        ))}
      {/* 「当前筛选下这一节没有条目」那句提示没了，因为这一节现在根本不会出现 ——
          `buildSections` 里空节整节消失。留个空壳节头不回答任何问题，只会把真正
          命中的那一节挤下去。 */}
    </div>
  );
}

/** 这条通道上毙得最狠的那个组（≥3 条不通过才报）。返回可退回改写的条目 id。 */
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
 * 也是唯一一个改了还来得及的时刻 —— 提交即扣费，之后再改只影响下一次。
 * 改完当场重算这一下要花多少：模型之间差 5.5 倍，那个数字必须随选择一起变，
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
  busy,
  onApplyParams,
  onClose,
  onConfirm,
}: {
  preview: SubmitPreview;
  ids: number[];
  models: ModelInfo[];
  busy: boolean;
  /** 把参数写进这些条目并重取预览。 */
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
  //
  // 由 Rust 逐通道算好（`SubmitPreview.lanes`）：空位是按通道的，前端拿一个全局上限减
  // 一个全局在跑数，会在一批横跨两条通道时给出一个谁也不对的数（0031）。
  const goesNow = preview.lanes.reduce((a, l) => a + l.goesNow, 0);
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
          <span className="fs11 fw6 t3 nowrap">这些条目的参数</span>
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
            即梦<b>按模型通道各排各的队</b>，每条通道各有一个在跑上限，所以现在{" "}
            <b>先发 {goesNow} 条</b>
            {waits > 0 && (
              <>
                ，其余 <b>{waits} 条排在本地</b>，出一条自动补一条 —— 不必再来点一次
              </>
            )}
            。
          </div>
          {/* 逐通道分账。这一次提交横跨两条通道时，一个合计数答不出「卡的是哪条」——
              而那正是「我这 6 条 mini 为什么不走」当初没人答得上来的原因。 */}
          <div className="fs11 t3" style={{ lineHeight: 1.8 }}>
            {preview.lanes.map((l) => (
              <span key={l.label} style={{ marginRight: 12 }}>
                <b className="t1">{l.label}</b> 共 {l.total} 条 · 先发 {l.goesNow} 条（该通道上限{" "}
                {l.limit}）
              </span>
            ))}
          </div>
          <div className="fs11 t3">
            排队的那些<b>还没扣费</b>：额度是在真正发出去的那一刻扣的，所以下面那个预估是
            这些条目全部跑完的总数，不是现在就要花掉的。
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

/**
 * 换通道面板 —— 把选中的条目改投到另一条即梦队列。
 *
 * ## 这张卡存在的全部理由：两种「排队中」的代价差着一整份额度
 *
 * 一条通道同时只有上限那么几条真在即梦手上（2.0fast 非 VIP 实测 = 1），**其余同通道的
 * 全压在本地**（`stage='ready' AND submit_queued_at IS NOT NULL`）—— 即梦对它们一无所知，
 * `submit_id` 为空、一分钱没扣。所以「排在慢通道上的那 78 条」换通道是**免费**的。
 *
 * 而人此前找得到的唯一按钮是「重跑」，那个按钮对已提交的条目会丢弃一份已付费的任务。
 * 一个免费、一个是第二份钱，长得却一模一样 —— 这张卡的职责就是把这两件事**分开说**，
 * 并且在按钮之前把数字摆出来。
 *
 * 已提交的那些默认**不动**：那个复选框不勾，它们就原样留在原通道上继续跑。
 */
function ChannelSwitchModal({
  rows,
  models,
  autofillOn,
  busy,
  onClose,
  onConfirm,
}: {
  rows: Row[];
  models: ModelInfo[];
  /** 常驻队列开着没有 —— 决定要不要提示「这一换就退出候选池」。 */
  autofillOn: boolean;
  busy: boolean;
  onClose: () => void;
  onConfirm: (p: ChannelParams, abandon: boolean, andSubmit: boolean) => void;
}) {
  // ── 按「换完之后会怎样」分堆 ────────────────────────────
  //
  // 这张卡原来只按「花不花钱」分，于是它答得出「换通道免费」，却答不出人真正在问的
  // 那句：**换完它就自己跑起来了吗**。而答案逐堆完全不同 —— 已放行的换完自动接着排、
  // 待放行的还要点确认提交、待改写的连提示词都还没有，怎么换都发不出去。
  //
  // 少了这一层，一个人选中 6 条待改写换了通道，会得到一个「已改投 6 条」的成功提示，
  // 然后盯着一个永远不会自己动的列表。
  const queued = rows.filter((r) => r.action === "queued"); // 已放行，在本地排队
  const holding = rows.filter((r) => r.action === "submit"); // 待放行，等人点确认
  const noPrompt = rows.filter((r) => r.stage === "rewrite"); // 还没有视频提示词
  const free = rows.filter((r) => r.stage === "ready" || r.stage === "rewrite");
  const live = rows.filter((r) => r.stage === "run");
  const paid = live.filter((r) => r.clip.billed);
  const locked = rows.length - free.length - live.length;
  const paidCredit = paid.reduce((a, r) => a + (r.clip.creditCount ?? r.clip.submitCredit ?? 0), 0);
  // 换通道 = 把型号写死，而 `AUTOFILL_POOL` 只捡型号为空的（「指定过参数的不给补单器
  // 捡走」）。于是一个叫「换通道」的动作会顺带把这些条目**永久移出常驻队列的候选池**
  // —— 补单器开着、水位线又快见底时，这是个查半天查不出原因的静默后果。
  const leavingPool = rows.filter((r) => (r.clip.modelVersion ?? "").trim() === "").length;

  const [abandon, setAbandon] = useState(false);
  // 带过去的原值取第一条 —— 混选时下面会标「多条不一致」，而一个「保持不变」的选项在
  // 这里不存在：即梦只接受一套完整组合。
  const first = rows[0];
  const wantDur = first?.duration ?? null;
  const wantRes = first?.resolution ?? "";
  const mixed = rows.some((r) => r.duration !== wantDur || r.resolution !== wantRes);

  const [p, setP] = useState<Params>({ modelVersion: "", duration: null, videoResolution: "" });
  const target = models.find((m) => m.modelVersion === p.modelVersion);
  const carried = carryParams(target, wantDur, wantRes);

  const willMove = free.length + (abandon ? live.length : 0);
  const perSec = creditPerSec(models, p.modelVersion, p.videoResolution);
  const after = perSec != null && p.duration != null ? perSec * p.duration * willMove : null;
  const before = estimateOf(rows.filter((r) => free.includes(r) || (abandon && live.includes(r))));

  return (
    <Modal
      title={`换通道 · ${rows.length} 条`}
      width="w700"
      onClose={onClose}
      footer={
        <>
          <span className="fs11 t3">
            {willMove === 0
              ? "当前选择下没有可改投的条目"
              : `将改投 ${willMove} 条${abandon && paid.length > 0 ? `，其中 ${paid.length} 条要丢弃已付费的提交单` : ""}`}
          </span>
          <div className="f1" />
          <button type="button" className="btn sm gho" onClick={onClose}>
            取消
          </button>
          <button
            type="button"
            className={cn("btn sm", abandon && paid.length > 0 ? "dngo" : "pri")}
            disabled={busy || target == null || willMove === 0}
            onClick={() => {
              if (!target || p.duration == null) return;
              onConfirm(
                {
                  modelVersion: p.modelVersion,
                  duration: p.duration,
                  videoResolution: p.videoResolution,
                },
                abandon,
                false,
              );
            }}
          >
            改投 {willMove} 条
          </button>
          {/* 「换完就发」——人换通道多半就是这个意思。但它接的是**提交确认卡**而不是
              直接放行：提交即扣费且不可撤回，而这张卡通篇在说「换通道免费」，
              在它上面挂一个会安静扣钱的按钮是最坏的一种连贯。
              只在确实有东西可发时出现（待放行、且已经有视频提示词）。 */}
          {holding.length > 0 && (
            <button
              type="button"
              className="btn sm pri"
              disabled={busy || target == null}
              onClick={() => {
                if (!target || p.duration == null) return;
                onConfirm(
                  {
                    modelVersion: p.modelVersion,
                    duration: p.duration,
                    videoResolution: p.videoResolution,
                  },
                  abandon,
                  true,
                );
              }}
            >
              改投并去提交 {holding.length} 条
            </button>
          )}
        </>
      }
    >
      <div style={{ padding: 4 }}>
        <div className="parambar mb8">
          <span className="fs11 fw6 t3 nowrap">改投到</span>
          <V2vParamPicker
            models={models}
            value={p}
            disabled={busy}
            onChange={(next) => {
              // 换模型时**把原值带过去**，而不是像别处那样清空 —— 这张卡的语义是
              // 「换条队」，不是「重设参数」。夹过的会在下面报出来。
              if (next.modelVersion !== p.modelVersion) {
                const c = carryParams(
                  models.find((m) => m.modelVersion === next.modelVersion),
                  wantDur,
                  wantRes,
                );
                setP({
                  modelVersion: next.modelVersion,
                  duration: c.duration,
                  videoResolution: c.resolution,
                });
              } else {
                setP(next);
              }
            }}
          />
        </div>

        {target && (carried.durationChanged || carried.resolutionChanged) && (
          <div className="fs11 wr2 mb8" style={{ lineHeight: 1.8 }}>
            这条通道接不了原来的规格，已经夹到最近的合法值：
            {carried.durationChanged && ` 时长 ${wantDur}s → ${carried.duration}s`}
            {carried.resolutionChanged && ` 分辨率 ${wantRes} → ${carried.resolution}`}
            。上面还能再改。
          </div>
        )}
        {mixed && (
          <div className="fs11 t3 mb8">
            选中的条目原参数不一致 —— 上面填的这一套会**整体覆盖**它们，不是逐条保持。
          </div>
        )}

        {/* 「换完之后会怎样」——这是人点这个按钮时真正在问的问题，而它逐堆答案完全不同。
            放在花钱那一段**之前**：先答「它会不会跑起来」，再答「要花多少」。 */}
        <div className="costbar mb8">
          <div className="fs11 fw6 t3">换完之后</div>
          {queued.length > 0 && (
            <div className="fs12">
              <b>{queued.length} 条已放行的 → 直接排到新通道上，什么都不用再点</b>
              ，出空位自动发。它们带着原来的放行时刻插队，不会被罚到队尾。
            </div>
          )}
          {holding.length > 0 && (
            <div className="fs12">
              <b>{holding.length} 条待放行的 → 仍然停在「等你点确认提交」</b>
              。用下面那个「改投并去提交」可以换完直接进确认卡（那一步才扣费）。
            </div>
          )}
          {noPrompt.length > 0 && (
            <div className="fs12 wr2" style={{ lineHeight: 1.8 }}>
              <b>{noPrompt.length} 条待改写的 → 换完还是发不出去</b>
              ：它们连视频提示词都还没有，而即梦要的就是提示词。得先去 Claude Code / Codex 里跑
              v2v-rewrite 把提示词写回来，那之后才谈得上提交。 换通道这一步对它们仍然有效 ——
              只是生效要等到提交那一刻。
            </div>
          )}
        </div>

        <div className="costbar mb8">
          {free.length > 0 && (
            <div className="fs12">
              <b>{free.length} 条还在本地队列 / 待放行 / 待改写</b> —— 即梦对它们一无所知，
              <b>一分钱没扣</b>，换通道免费。
            </div>
          )}
          {live.length > 0 && (
            <div className={cn("fs12", paid.length > 0 && "terr")} style={{ lineHeight: 1.8 }}>
              <b>{live.length} 条已经提交给即梦</b>
              {paid.length > 0 ? (
                <>
                  ，其中 {paid.length} 条<b>确实扣了 {paidCredit} 额度且不可撤回</b>
                  。改投等于丢弃它们 —— 那几条视频即梦还在跑，但我们此后再也取不回来， 新通道上要
                  <b>再花一份钱</b>。
                </>
              ) : (
                <>，但一处计费证据都没有（幽灵单 / 被并发上限弹回的），改投不花钱。</>
              )}
              <label className="fx ac gap6 mt5" style={{ cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={abandon}
                  disabled={busy}
                  onChange={(e) => setAbandon(e.target.checked)}
                />
                <span className="fs11">连这 {live.length} 条一起改投</span>
              </label>
            </div>
          )}
          {locked > 0 && (
            <div className="fs11 t3">
              另有 {locked} 条已出片或已定案，换通道对它们没有意义，会跳过。
            </div>
          )}
          {autofillOn && leavingPool > 0 && (
            <div className="fs11 wr2" style={{ lineHeight: 1.8 }}>
              其中 {leavingPool} 条现在跟随全局默认通道，换过去等于**给它们写死型号** ——
              常驻队列只捡没写死型号的，所以这 {leavingPool} 条从此不会再被自动补单捡走。
              要放回去：底栏参数条把模型选回「跟随全局默认」。
            </div>
          )}
          {target && (
            <div className="fs11 t3">
              额度预估：{before == null ? "?" : before} → <b className="t1">{after ?? "?"}</b>
              {target.vip && " · vip 同规格贵 5.5 倍，买到的只是不排队"}
            </div>
          )}
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
        // 「上次查询」读 `lastSweepAt`（真实问过即梦的时刻），不是心跳时刻 ——
        // 同顶栏那个按钮的理由：两者差一个数量级，混用就是在说数据比实际新鲜。
        <span className="chip">
          {tick == null
            ? "等待首轮心跳"
            : `${tick.running} 在跑 · 上次查询 ${
                tick.lastSweepAt == null ? "还没查过" : fmtAgo(Math.max(0, now - tick.lastSweepAt))
              }`}
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
