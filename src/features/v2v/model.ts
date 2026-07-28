import type { ClipView, EffectiveParams, ModelInfo } from "@/lib/ipc";

/**
 * 视频流水线的**派生模型**（纯函数，不碰 React、不碰 IPC）。
 *
 * 抽出来的理由是一条界面规则：表格里「情况 · 判断依据」那一列必须说出**为什么**，
 * 而那个「为什么」由五六个字段交叉决定（阶段 × 队列位次 × 扣费回执 × 错误分类 × 等待时长）。
 * 把这段判断散在 JSX 里，等于让「幽灵单」的定义在界面、筛选、批量操作三处各写一遍
 * —— 而它们一旦分叉，筛出来的条目和按钮上写的数字就对不上了。
 */

export type Stage = "rewrite" | "ready" | "run" | "rev" | "pass" | "rej" | "fail";

/** 七态的显示元数据。颜色一律取 token（铁律 5：oklch 只许出现在 globals.css）。 */
export const STAGE_META: Record<Stage, { label: string; fg: string; bg: string; seg: string }> = {
  rewrite: { label: "待改写", fg: "var(--t3)", bg: "var(--inset)", seg: "var(--sg-rewrite)" },
  ready: { label: "待提交", fg: "var(--acc2)", bg: "var(--accbg)", seg: "var(--sg-ready)" },
  run: { label: "已提交", fg: "var(--st-run)", bg: "var(--accbg)", seg: "var(--sg-run)" },
  rev: { label: "待验收", fg: "var(--st-rev)", bg: "var(--wrbg)", seg: "var(--sg-rev)" },
  pass: { label: "成片", fg: "var(--ok2)", bg: "var(--okbg)", seg: "var(--sg-pass)" },
  rej: { label: "未通过", fg: "var(--t3)", bg: "var(--inset)", seg: "var(--sg-rej)" },
  fail: { label: "失败", fg: "var(--er)", bg: "var(--erbg)", seg: "var(--sg-fail)" },
};

export function isLive(stage: Stage): boolean {
  return stage !== "pass" && stage !== "rej";
}

/**
 * 下一步该谁动手、动什么手。
 *
 * **阶段回答「它在哪」，动作回答「拿它怎么办」** —— 而后者才是这一页存在的理由。
 * 按阶段组织时，21 条待改写会同时显示「需要我 0」「待改写 21」「无待办」
 * 「等 skill 写回 · 交接已物化」四句互相矛盾的话：阶段名就写在每一条脸上，
 * 是最不缺的信息；真正没人回答的是「所以我现在该干嘛」。
 */
export type NextAction = "rewrite" | "submit" | "review" | "fix" | "queued" | "wait" | "done";

export const ACTION_META: Record<NextAction, { label: string; fg: string; dot: string }> = {
  fix: { label: "处理异常", fg: "var(--er)", dot: "var(--sg-fail)" },
  rewrite: { label: "去改写", fg: "var(--acc2)", dot: "var(--sg-rewrite)" },
  submit: { label: "待放行", fg: "var(--acc2)", dot: "var(--sg-ready)" },
  review: { label: "待验收", fg: "var(--st-rev)", dot: "var(--sg-rev)" },
  queued: { label: "排队中", fg: "var(--t3)", dot: "var(--sg-ready)" },
  wait: { label: "等即梦", fg: "var(--t3)", dot: "var(--sg-run)" },
  done: { label: "已定案", fg: "var(--t3)", dot: "var(--sg-pass)" },
};

/** 六档在制动作 + 已定案。顺序 = 「离人最近的排最前」，节头摘要也照这个序。 */
export const ACTION_ORDER: NextAction[] = [
  "fix",
  "rewrite",
  "submit",
  "review",
  "queued",
  "wait",
  "done",
];

/**
 * 阻在**人**身上的四档 —— 「需要我」就是它们的并集。
 *
 * 待改写在里面，这是与旧口径最大的分歧：那一步虽然在 Claude Code 里做，但它**只可能**
 * 由人推动，GenDesk 这边已经把工单物化好、什么都不缺了。把它排除在「需要我」之外，
 * 等于让全流水线最大的一处阻塞在界面上显示为 0。
 */
export const MINE: NextAction[] = ["fix", "rewrite", "submit", "review"];

/**
 * 阶段 + 幽灵判定 → 下一步动作。
 *
 * `run + 幽灵疑单 → fix` 是这层派生存在的第二个理由：幽灵单只存在于 `run`，
 * 而旧的「需要我」= ready|rev|fail 不含 run —— 于是唯一该**免费**重跑的那一类，
 * 被默认筛选藏了起来。这里结构上不可能再漏。
 */
export function nextAction(stage: Stage, phantom: boolean, queued = false): NextAction {
  if (stage === "pass" || stage === "rej") return "done";
  if (stage === "fail") return "fix";
  if (stage === "run") return phantom ? "fix" : "wait";
  if (stage === "rev") return "review";
  if (stage === "ready") return queued ? "queued" : "submit";
  return "rewrite";
}

/**
 * 这一条是不是「人已放行、正在本地排队等即梦的空位」（0028）。
 *
 * 即梦对**每条模型通道**各有一个并发上限（实测 2.0fast 只跑得下 1 条），超出的部分
 * 会被它逐条以 `ExceedConcurrencyLimit` 弹回来。所以 GenDesk 逐通道只发得下的那几条，
 * 其余留在本地 —— 而**别的通道不受影响**（0031）。
 * 这一格与「等你点确认提交」必须分开显示 —— 在此之前两者长得一模一样，
 * 于是一批放行完的片子看起来像是没人管。
 */
export function isQueued(c: ClipView): boolean {
  return c.stage === "ready" && c.submitQueuedAt != null;
}

/**
 * 这一条走哪条即梦通道：它自己写死的型号优先，没写就落到设置里的默认型号。
 *
 * 与 Rust 的 `runner::channel_of` 和 SQL 的 `repo::CHANNEL_OF` **必须同口径**。
 * 一边按型号分桶、另一边按别的口径数空位，结果就是数着 A 通道的空位往 B 通道发单
 * ——而界面这一侧分叉的症状同样难查：节头写着「2.0Mini」，节里躺着一条实际会走
 * 2.0Fast 的条目，于是「这一节全选换通道」换的不是人以为的那批。
 *
 * 这里是这份口径在前端的**唯一**副本；分节、本地队列位次、等待中位数三处都读它。
 */
export function channelOf(c: ClipView, eff: EffectiveParams | null): string {
  const own = (c.modelVersion ?? "").trim();
  return own !== "" ? own : (eff?.modelVersion ?? "");
}

/** 动作筛选片。`mine` 是默认值 —— 一进页面该看到的是「等你动手的」，不是全部。 */
export type ActionFilter = NextAction | "mine" | "all" | "rej";

/**
 * 工作台的筛选片。
 *
 * **没有「成片」**：验收通过的视频归成片库那一页，它们已经不是流水线的事了。
 * 「未通过」留着当逃生舱 —— 那些条目不该变得无处可寻，只是不该默认占位。
 */
export const ACTION_CHIPS: { key: ActionFilter; label: string }[] = [
  { key: "mine", label: "需要我" },
  { key: "all", label: "全部在制" },
  ...MINE.map((a) => ({ key: a as ActionFilter, label: ACTION_META[a].label })),
  { key: "queued", label: ACTION_META.queued.label },
  { key: "wait", label: ACTION_META.wait.label },
  { key: "rej", label: STAGE_META.rej.label },
];

/**
 * 信号 = 与阶段正交的**例外**。
 *
 * 单看阶段回答不了「这批里有没有出事」：18 条幽灵单和 18 条正常排队在「已提交」列里
 * 长得一模一样，而它们的处置完全相反（一个直接重跑不花钱，一个必须继续等否则重复扣费）。
 */
export type SignalKey = "phantom" | "timeout" | "slow" | "rerun" | "vip" | "auto";

export const SIGNAL_CHIPS: { key: SignalKey; label: string; title: string }[] = [
  {
    key: "phantom",
    label: "幽灵单",
    title: "即梦接了单却从未入队、从未计费（无队列位次 + 无扣费回执）。重跑不花钱。",
  },
  {
    key: "timeout",
    label: "超时",
    title: "只是我们这边不等了 —— 额度已扣、即梦那边还在跑。该点「继续等待」而不是重跑。",
  },
  { key: "slow", label: "等待异常", title: "已超同通道在跑条目中位等待时长的 3 倍。" },
  { key: "rerun", label: "重跑过", title: "尝试次数 > 1，即同一张图已经花过不止一份额度。" },
  { key: "vip", label: "vip 通道", title: "同规格贵 5.5 倍，买到的只是不排队。" },
  { key: "auto", label: "常驻队列", title: "由自动补单放行的条目，不是你手动提交的。" },
];

export const SORTS = {
  wait: "已等最久",
  credit: "额度最高",
  attempt: "重跑最多",
} as const;
export type SortKey = keyof typeof SORTS;

/** 「等待异常」的判据倍数。中位数的 3 倍 —— 单条慢不算事，慢出一个数量级才是。 */
const SLOW_FACTOR = 3;

/** 表格一行所需的全部派生值。 */
export interface Row {
  clip: ClipView;
  stage: Stage;
  /** 下一步该干什么。筛选、节头摘要、行内色点三者同源，不可能各说各话。 */
  action: NextAction;
  /** 归一化后的模型全名（clip 自己的 → 设置里的默认）；两者都没有时为 null。 */
  modelFull: string | null;
  modelShort: string;
  vip: boolean;
  /** 生效的时长与分辨率（同上，逐级回落）。 */
  duration: number | null;
  resolution: string | null;
  /** 这一条按单价表算出来的预估额度；查不到单价为 null（**不猜**）。 */
  estimate: number | null;
  /** 实际额度：回执优先，没有回执时才回落到预估。 */
  credit: number | null;
  creditEstimated: boolean;
  /** 已等待秒数（按**首次**提交算，「继续等待」不清零）。未提交为 0。 */
  waitSecs: number;
  /** 距上次发起查询多少秒；从未查过为 null。 */
  polledAgo: number | null;
  /**
   * 排在第几位。两条队列**语义不同，不能混成一个数字**：
   * - `ready + 已放行` → **本地**队列位次（1 = 下一条发出去的），由这一层算。
   * - `run` → **即梦**队列位次（`clip.queueIdx`，实测能到四千多位），由即梦下发。
   *
   * 前者是「还要等我们发几条」，后者是「即梦那边前面还有多少人」——
   * 把它们显示成同一个「第 N 位」会让人以为本地排第 3 就快了。
   */
  queuePos: number | null;
  /**
   * 在跑但一处计费证据都没有、且过了宽限期 —— **Rust 下发的结论**（`clip.phantomSuspect`）。
   *
   * 这里不再自己算。前端曾抄过一份判据，它只看三个字段、还手抄了一份 15 分钟的宽限期
   * 常量，而 Rust 那侧读的是五处证据（含提交回执与历史队列位次）、按 `submittedAt`
   * 计时。两份判据给同一条不同结论时，指向的是两个相反的动作：幽灵单重跑不花钱，
   * 正在排队的重跑要再花一份。
   */
  phantomLive: boolean;
  slow: boolean;
  signals: Set<SignalKey>;
  /** 「情况 · 判断依据」那一列。 */
  situation: string;
  situationTone: "er" | "wr" | "acc" | "t3";
}

/**
 * 「seedance2.0fast_vip」→「2.0Fast VIP」。表格里那一列只有 86px。
 *
 * **真相在 Rust**（`dreamina::short_label`，随 `ModelInfo.label` 一起下发）——
 * 这里只是拿不到 `ModelInfo` 时的回落（库里存着一个已经从清单里下架的型号）。
 * 有 `ModelInfo` 就读 `label`，别在这里再判一遍。
 */
export function shortModel(full: string): string {
  const vip = full.endsWith("_vip");
  const base = (vip ? full.slice(0, -4) : full).replace(/^seedance/, "");
  return (
    base.replace(/[a-z]+/g, (w) => w.charAt(0).toUpperCase() + w.slice(1)) + (vip ? " VIP" : "")
  );
}

/** 秒 → 「12s / 3m / 2h31m」。等宽列里要短。 */
export function fmtDur(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}

/** 秒 → 「12 秒前 / 3 分钟前 / 2 小时前」。正文里要读得顺。 */
export function fmtAgo(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  if (s < 60) return `${s} 秒前`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  return `${Math.floor(s / 86400)} 天前`;
}

/** 秒 → 「3 小时 12 分」。摘要横幅那句「你离开的 …」。 */
export function fmtSpan(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  if (s < 60) return `${s} 秒`;
  if (s < 3600) return `${Math.floor(s / 60)} 分钟`;
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return m === 0 ? `${h} 小时` : `${h} 小时 ${m} 分`;
}

/** unix 秒 → 「09:14」。历程条用。 */
export function fmtClock(t: number): string {
  const d = new Date(t * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function median(xs: number[]): number {
  if (xs.length === 0) return 0;
  const s = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 === 1 ? (s[mid] ?? 0) : ((s[mid - 1] ?? 0) + (s[mid] ?? 0)) / 2;
}

/**
 * 这一条有没有交付到输出目录。
 *
 * `export_path` 是验收通过那一刻拷进输出目录的那份的路径（0027）。拷贝失败**不会**
 * 回滚验收（判定是人做的，文件可以补），所以「pass 但 export_path 为空」是一个
 * 真实会出现、且今天界面上完全不提的状态 —— 片子做出来了，却没人知道它没落地。
 */
export function delivered(c: ClipView): boolean {
  return (c.exportPath ?? "").trim() !== "";
}

/** 换通道时把当前参数带过去的结果。`*Changed` 为真即「原值这条通道接不了，夹过了」。 */
export interface CarriedParams {
  duration: number | null;
  resolution: string;
  durationChanged: boolean;
  resolutionChanged: boolean;
}

/**
 * 换通道时，把当前生效的时长/分辨率**尽量原样**带到目标通道。
 *
 * 为什么不能直接交给 Rust 的 `normalize_opts`：它在只给模型时会把时长补成该模型的
 * **最小值**、分辨率补成**第一档**（`dreamina.rs`）。于是一条配了 1080p/10s 的条目
 * 换个通道就被悄悄降成 720p/5s —— 人要的是「换条队」，拿到的是「顺便降了规格」，
 * 而这件事在界面上一个字都不会说。
 *
 * 目标通道接不了原值时才夹，并且**把夹过的事实报出去**（`*Changed`），由面板写成
 * 「10s → 8s（该通道上限）」摆在确认按钮之前。静默改值正是「我明明选了 1080p
 * 却不生效」的成因。
 */
export function carryParams(
  target: ModelInfo | undefined,
  duration: number | null,
  resolution: string,
): CarriedParams {
  if (!target) {
    return { duration: null, resolution: "", durationChanged: false, resolutionChanged: false };
  }
  const dur =
    duration == null
      ? target.minDuration
      : Math.min(target.maxDuration, Math.max(target.minDuration, duration));
  const res =
    resolution !== "" && target.resolutions.includes(resolution)
      ? resolution
      : (target.resolutions[0] ?? "");
  return {
    duration: dur,
    resolution: res,
    durationChanged: duration != null && dur !== duration,
    resolutionChanged: resolution !== "" && res !== resolution,
  };
}

/** 单价查询：查不到返回 null（界面据此显示「≥」，绝不摆一个编出来的数字）。 */
export function creditPerSec(
  models: ModelInfo[],
  modelFull: string | null,
  resolution: string | null,
): number | null {
  if (!modelFull || !resolution) return null;
  const m = models.find((x) => x.modelVersion === modelFull);
  return m?.resPrices.find((p) => p.resolution === resolution)?.creditPerSec ?? null;
}

/**
 * 把一批 clip 派生成行模型。
 *
 * `now` 是外部传进来的秒表 —— 「已等 3h12m」要自己走字，而组件每秒重渲染时
 * 这里必须给出新值，不能在函数内部读 `Date.now()`（那样纯度没了，也没法测）。
 */
export function deriveRows(
  clips: ClipView[],
  models: ModelInfo[],
  eff: EffectiveParams | null,
  now: number,
  /**
   * 每条通道同时跑得下几条（`ChannelStat.limit`，按 `modelVersion` 索引）。
   *
   * **按通道而不是一个全局数字**（0031）：即梦按模型通道各排各的队，2.0fast 排满了
   * 与 2.0mini 能不能发毫无关系。拿一个全局上限去写文案，会对着一条明明能立刻发出去的
   * mini 说「即梦同时只跑 1 条」—— 而那句话正是人放弃排查的地方。查不到就按 1。
   */
  limits: ReadonlyMap<string, number> = new Map(),
): Row[] {
  // 本地队列位次：严格照后端取用的顺序（放行时刻，其次 id）算一遍，**并且按通道各排各的**
  // —— 后端补位是逐通道取队首的（`pick_submit_queued_on`），这边若按全局排，
  // 界面会对一条马上就要发出去的 mini 说「本地排第 79」。
  // 两边分叉的代价是界面说「你排第 1」而实际先发的是另一条 —— 那种错没人查得出来。
  const queuePosOf = new Map<number, number>();
  const seen = new Map<string, number>();
  for (const c of clips
    .filter(isQueued)
    .sort((a, b) => (a.submitQueuedAt ?? 0) - (b.submitQueuedAt ?? 0) || a.id - b.id)) {
    const ch = channelOf(c, eff);
    const pos = (seen.get(ch) ?? 0) + 1;
    seen.set(ch, pos);
    queuePosOf.set(c.id, pos);
  }
  // 「等待异常」是相对判据：跟**同一条通道**上的其它在跑条目比，而不是跟一个拍脑袋的
  // 绝对秒数比。
  //
  // 分组维度从批次改成了通道（0032）：一个批次的条目会分散到不同通道，而通道之间的
  // 等待时长差着数量级（VIP 1–3 分钟出片，非 VIP 排到四千多位要等几小时）。把它们
  // 混在一起取中位数，得到的是一个谁也不代表的数 —— 拿它当判据，要么把正常排队的
  // 非 VIP 全标成「等待异常」，要么把真的卡住的 VIP 全放过去。
  const runWaits = new Map<string, number[]>();
  for (const c of clips) {
    if (c.stage !== "run") continue;
    const key = channelOf(c, eff);
    const w = c.firstSubmittedAt == null ? 0 : Math.max(0, now - c.firstSubmittedAt);
    const bucket = runWaits.get(key);
    if (bucket) bucket.push(w);
    else runWaits.set(key, [w]);
  }
  const medians = new Map<string, number>();
  for (const [k, v] of runWaits) medians.set(k, median(v));

  return clips.map((c) => {
    const stage = c.stage as Stage;
    const key = channelOf(c, eff);
    const modelFull = c.modelVersion ?? eff?.modelVersion ?? null;
    const info = models.find((m) => m.modelVersion === modelFull);
    const resolution = c.videoResolution ?? eff?.videoResolution ?? info?.resolutions[0] ?? null;
    const duration = c.duration ?? eff?.duration ?? info?.minDuration ?? null;
    const perSec = creditPerSec(models, modelFull, resolution);
    const estimate = perSec != null && duration != null ? perSec * duration : null;

    const queued = isQueued(c);
    const queuePos = queued ? (queuePosOf.get(c.id) ?? null) : c.queueIdx;
    const waitSecs = c.firstSubmittedAt == null ? 0 : Math.max(0, now - c.firstSubmittedAt);
    const polledAgo = c.polledAt == null ? null : Math.max(0, now - c.polledAt);

    // 幽灵判定**只有一个来源**：Rust。它读的是全部五处计费证据（本次回体两处 +
    // 已落库三处），而且就是真正会去 `fail(phantom)` 那条路径用的同一个函数。
    const phantomLive = c.phantomSuspect;

    const med = medians.get(key) ?? 0;
    const slow = stage === "run" && med > 0 && waitSecs > med * SLOW_FACTOR && !phantomLive;

    const isPhantom = phantomLive || (stage === "fail" && c.errorType === "phantom");
    const isTimeout = stage === "fail" && c.errorType === "timeout";

    const receipt = c.creditCount ?? c.submitCredit ?? null;
    const settled = stage === "rev" || stage === "pass" || stage === "rej";
    const credit =
      stage === "rewrite" || stage === "ready"
        ? null
        : isPhantom
          ? 0
          : (receipt ?? (settled || stage === "run" ? estimate : null));
    const creditEstimated = receipt == null && credit != null && !isPhantom;

    // vip 由后端下发（`ModelInfo.vip`），不在这里判后缀 —— 即梦哪天出一个不带
    // `_vip` 后缀的付费加急档，抄在前端的这条规则会安静地漏掉它。
    const vip = info?.vip ?? false;

    const signals = new Set<SignalKey>();
    if (isPhantom) signals.add("phantom");
    if (isTimeout) signals.add("timeout");
    if (slow) signals.add("slow");
    if (c.attempt > 1) signals.add("rerun");
    if (vip) signals.add("vip");
    if (c.autoSubmitted) signals.add("auto");

    // 「情况」这一列必须点名**动作**与**代价**，而不是复述阶段（阶段就在旁边的色点上）。
    // 前四个关键词（疑幽灵单/等待异常/继续等待/免费重跑）保留原词：既有测试断言它们，
    // 而这四处指错方向的代价是真金白银 —— 幽灵单重跑不花钱，超时重跑要再花一份。
    let situation: string;
    let situationTone: Row["situationTone"] = "t3";
    if (phantomLive) {
      situation = "疑幽灵单 · 没入队也没扣费，重跑不花钱";
      situationTone = "er";
    } else if (slow) {
      situation = `等待异常 · 已超同通道中位数 ${SLOW_FACTOR} 倍，别手动催`;
      situationTone = "wr";
    } else if (stage === "run") {
      // 位次是排队几小时里**唯一**有意义的进度：「第 4485 位」和「第 12 位」是两件
      // 完全不同的事。绝不编一个位次出来。
      //
      // 问不到时**说「问不到」，并给出上次问的时刻**（0032）。这里原来回落到
      // 「本批已出 X/Y」，而那个分数的分子分母都不指向任何真实的队列：同一批次的条目
      // 分散在不同通道上，各排各的队、上限也各不相同，加起来的进度谁也不是。更糟的是
      // 它长得像个答案，于是没人再去追问「这条到底跑完了没有」。
      //
      // 现在这句话答的是「不知道 + 什么时候问的」，而刷新按钮就在顶栏 —— 位次与状态
      // 都只能靠逐条 `query_result` 拿到，那正是那个按钮做的事。
      situation =
        c.queueIdx != null && c.queueIdx > 0
          ? `即梦在排队 · 前面还有 ${c.queueIdx} 个`
          : polledAgo == null
            ? "即梦在跑 · 还没问到过位次"
            : `即梦在跑 · 位次问不到（${fmtAgo(polledAgo)}问过）`;
    } else if (isTimeout) {
      situation = "继续等待 · 额度已扣，即梦还在跑";
      situationTone = "er";
    } else if (stage === "fail" && c.errorType === "phantom") {
      situation = "免费重跑 · 从未计费";
      situationTone = "er";
    } else if (stage === "fail" && c.errorType === "submit_timeout") {
      // 与上一条正好相反：幽灵单是「确认没花钱」，这一条是「不知道花没花」。
      // 提交的 CLI 被超时杀掉之前可能已经下过单，而 submit_id 随进程没了。
      // 直接重跑是这一格里最贵的一个误操作，所以这句话必须说「先核对」。
      situation = "提交超时 · 可能已扣费，核对后再决定重跑";
      situationTone = "er";
    } else if (stage === "fail") {
      situation = `失败 · ${c.errorType ?? "原因见执行日志"}`;
      situationTone = "er";
    } else if (stage === "rev") {
      situation = "等你判通过还是不通过";
      situationTone = "wr";
    } else if (stage === "pass") {
      // 读交付路径而不是资产库 —— 成片的下游现在是本地文件夹，不是发布链。
      situation = delivered(c) ? "已成片 · 已交付" : "已成片 · 未交付到输出目录";
      if (!delivered(c)) situationTone = "er";
    } else if (stage === "rej") {
      situation = "你判了不通过 · 成片已进废纸篓";
    } else if (queued) {
      // 这一格要同时答出「为什么还没发出去」和「还要多久轮到我」。只说「排队中」
      // 会立刻引出「排谁的队、卡在哪」——而那正是这次事故里没人答得上来的问题。
      //
      // 位次与上限都**只在本通道内**成立，所以句子里必须点名通道：否则「本地排第 79」
      // 会被读成「整条流水线前面有 78 条」，而那 78 条全在另一条队上，与它无关。
      const lane = info?.label ?? (modelFull ? shortModel(modelFull) : "默认通道");
      const cap = limits.get(modelFull ?? "") ?? 1;
      situation =
        queuePos != null && queuePos > 1
          ? `已放行 · ${lane} 本地排第 ${queuePos}，该通道同时只跑 ${cap} 条`
          : `已放行 · 下一个就发它（${lane} 通道同时只跑 ${cap} 条）`;
    } else if (stage === "ready") {
      situation = "等你点确认提交 · 提交即扣费";
      situationTone = "acc";
    } else {
      // 全流水线最大的一处阻塞。**点名工具**（可直接搜、可直接打字）并说明工单已经
      // 备好，免得有人回头去找一个并不存在的「生成工单」按钮。
      situation = "等你跑 v2v-rewrite · 工单已就绪";
      situationTone = "acc";
    }

    return {
      clip: c,
      stage,
      action: nextAction(stage, phantomLive, queued),
      modelFull,
      modelShort: info?.label ?? (modelFull ? shortModel(modelFull) : "CLI 默认"),
      vip,
      duration,
      resolution,
      estimate,
      credit,
      creditEstimated,
      waitSecs,
      polledAgo,
      queuePos,
      phantomLive,
      slow,
      signals,
      situation,
      situationTone,
    };
  });
}

/**
 * 动作筛选是否命中。
 *
 * - `mine` = 阻在人身上的四档（改写 / 放行 / 判定 / 处置异常）。
 * - `all` = **全部在制**，不含已定案的 pass / rej。工作台回答的是「还剩多少活」，
 *   把做完的算进去，这个数就再也不准了。要看成片去成片库，要看毙掉的选「未通过」。
 * - `rej` 不是动作，是逃生舱。
 *
 * 收 `Row` 而不是 `Stage`：幽灵判定要看队列位次与扣费回执，那是整行的事。
 */
export function matchAction(r: Row, filter: ActionFilter): boolean {
  if (filter === "all") return isLive(r.stage);
  if (filter === "mine") return MINE.includes(r.action);
  if (filter === "rej") return r.stage === "rej";
  return r.action === filter;
}

/**
 * 搜索：编号 / 组名 / 视频提示词 / 生图提示词 / submit_id / 通道 一次覆盖。
 *
 * 批次号不在里面（0032）：它在界面上已经一处都不显示了，留一个搜得到却看不见的
 * 维度，只会让「搜 46 出来一堆不相干的东西」变成一个查不出原因的怪事。
 */
export function matchQuery(r: Row, q: string): boolean {
  if (q === "") return true;
  const needle = q.toLowerCase();
  const c = r.clip;
  return (
    c.promptCode.toLowerCase().includes(needle) ||
    c.groupName.toLowerCase().includes(needle) ||
    (c.videoPrompt ?? "").toLowerCase().includes(needle) ||
    c.sourcePrompt.toLowerCase().includes(needle) ||
    (c.submitId ?? "").toLowerCase().includes(needle) ||
    r.modelShort.toLowerCase().includes(needle)
  );
}

export function sortRows(rows: Row[], sort: SortKey): Row[] {
  const out = [...rows];
  if (sort === "wait") out.sort((a, b) => b.waitSecs - a.waitSecs);
  else if (sort === "credit") out.sort((a, b) => (b.credit ?? -1) - (a.credit ?? -1));
  else if (sort === "attempt") out.sort((a, b) => b.clip.attempt - a.clip.attempt);
  return out;
}

/** 一节（= 一条即梦通道）。 */
export interface Section {
  /** 通道全名（`model_version`）。空串 = 设置里也没写默认型号，走 CLI 默认。 */
  key: string;
  /** 通道简写（`ModelInfo.label`，随模型清单从 Rust 下发）。节头那个 chip 显示它。 */
  label: string;
  vip: boolean;
  /** 这条通道上涉及的提示词组名 —— 回答「这条队上跑的是哪几组」。 */
  title: string;
  /** 本通道全部条目（不受筛选影响）—— 摘要与「已定案」判定要看全貌。 */
  all: Row[];
  /** 当前筛选下这条通道还剩哪些行。 */
  rows: Row[];
  /** 按下一步动作分桶。节内动作按钮据此显示，组件不必再数一遍。 */
  counts: Record<NextAction, number>;
  /** 一句人话的节头摘要。例外在前 —— 被截断时先没的必须是常态。 */
  headline: string;
  headlineTone: "er" | "acc" | "t3";
  /** 已定案 = 全部落在 pass/rej，没有任何一条还需要人或机器动。 */
  done: boolean;
  createdAt: number;
}

/**
 * 节头摘要。
 *
 * 取代原来那条 104px 的阶段混合分段条 —— 它唯一的图例是 `title=` tooltip，
 * 于是「这些进度条是什么意思」成了一个没人答得上来的问题。一句话既自带图例，
 * 又能直接说出数字。
 *
 * 「这一批 N 条」那个前缀去掉了（0032）：一节现在是一条通道，而通道里的条目来自
 * 若干个批次，「这一批」会直接把人指错方向。总数由第一句的语义承担。
 */
function headlineOf(all: Row[], counts: Record<NextAction, number>) {
  const parts: string[] = [];
  if (counts.fix > 0) parts.push(`${counts.fix} 条出了异常`);
  if (counts.rewrite > 0) parts.push(`${counts.rewrite} 条等你改写`);
  if (counts.submit > 0) parts.push(`${counts.submit} 条等你放行`);
  if (counts.review > 0) parts.push(`${counts.review} 条等你验收`);
  if (counts.queued > 0) parts.push(`${counts.queued} 条在本地排队`);
  if (counts.wait > 0) parts.push(`${counts.wait} 条在即梦跑`);
  const headlineTone: Section["headlineTone"] =
    counts.fix > 0 ? "er" : counts.rewrite + counts.submit + counts.review > 0 ? "acc" : "t3";
  const headline =
    parts.length === 0
      ? `共 ${all.length} 条 · 已全部定案`
      : `共 ${all.length} 条 · ${parts.join("，")}`;
  return { headline, headlineTone };
}

/**
 * 按**通道**分节（0032）。
 *
 * ## 为什么不再按批次
 *
 * 即梦按模型通道各排各的队，一条通道排满与另一条能不能发**毫无关系**。而一个批次的
 * 条目会分散到不同通道上 —— 于是「按批次分节」下的每一个节内数字都不指向任何真实的
 * 队列：「本批已出 0/49」的分子分母跨着几条互不相干的队，「全选本节」选出来的是一堆
 * 跨通道的条目，对它们做的任何批量动作（尤其是换通道）都不成立。
 *
 * 按通道分之后，一节 = 一条队 = 一个可以整体处置的单位：节内的「还剩多少活」是真的，
 * 「全选本节 → 换通道」也是一个完整动作。
 *
 * 没写 `model_version` 的条目归**默认通道**那一节 —— 那本来就是它们会走的通道
 * （`channelOf` 与 Rust 的 `runner::channel_of` 同口径），另设一个「未定」节反而会把
 * 同一条队拆成两半。
 *
 * ## 排序：还在动的排前面
 *
 * 远端在跑 > 本地压着队 > 其余，同档按条数多的在前。批次曾经有一个天然的时间序
 * （id 倒序），通道没有 —— 通道之间唯一有意义的先后是「哪条还有账要算」。
 *
 * **当前筛选下一条都不显示的通道整节消失**，无论它是否还有活。旧规则只砍「已定案」的
 * 空节，于是筛「处理异常」时会留下一排只写着「当前筛选下没有条目」的空壳节头，
 * 把真正的三条待办推到屏幕外面。
 */
export function buildSections(all: Row[], visible: Row[], models: ModelInfo[] = []): Section[] {
  const groups = new Map<string, Row[]>();
  for (const r of all) {
    const key = r.modelFull ?? "";
    const bucket = groups.get(key);
    if (bucket) bucket.push(r);
    else groups.set(key, [r]);
  }
  const visByKey = new Map<string, Row[]>();
  for (const r of visible) {
    const key = r.modelFull ?? "";
    const bucket = visByKey.get(key);
    if (bucket) bucket.push(r);
    else visByKey.set(key, [r]);
  }

  const out: Section[] = [];
  for (const [key, rows] of groups) {
    const visRows = visByKey.get(key) ?? [];
    if (visRows.length === 0) continue;
    const info = models.find((m) => m.modelVersion === key);
    const counts = Object.fromEntries(ACTION_ORDER.map((a) => [a, 0])) as Record<
      NextAction,
      number
    >;
    for (const r of rows) counts[r.action] += 1;
    out.push({
      key,
      // 简写的真相在 Rust（`dreamina::short_label`）。查不到 `ModelInfo` 才回落 ——
      // 那说明库里存着一个已经从清单里下架的型号，此时行内的 `modelShort` 用的也是
      // 同一条回落，两处必须一致。
      label: info?.label ?? (key === "" ? "CLI 默认" : shortModel(key)),
      vip: info?.vip ?? false,
      title: sectionTitle(rows),
      all: rows,
      rows: visRows,
      counts,
      ...headlineOf(rows, counts),
      done: rows.every((r) => !isLive(r.stage)),
      createdAt: Math.max(...rows.map((r) => r.clip.createdAt)),
    });
  }
  // 还在动的排前面：远端在跑 > 本地压着队 > 其余；同档按条数多的在前，再同则按名字
  // 定死顺序（否则每次重渲染的节序会随 Map 插入序漂移）。
  const rank = (s: Section) => (s.counts.wait > 0 ? 0 : s.counts.queued > 0 ? 1 : 2);
  out.sort(
    (a, b) =>
      rank(a) - rank(b) ||
      b.counts.wait + b.counts.queued - (a.counts.wait + a.counts.queued) ||
      b.all.length - a.all.length ||
      a.key.localeCompare(b.key),
  );
  return out;
}

/**
 * 分节标题 = 这条通道上涉及的提示词组名。
 *
 * 组名是人给这批片子起的名（一份 txt = 一个组），而通道本身只是一条队 ——
 * 光看「2.0Fast」答不出「这条队上跑的是什么」。混多个组时只列前两个 ——
 * 铺满整行的组名反而什么都读不出来（同作品库分节的处置）。
 */
function sectionTitle(rows: Row[]): string {
  const names: string[] = [];
  for (const r of rows) {
    const n = r.clip.groupName.trim();
    if (n !== "" && !names.includes(n)) names.push(n);
  }
  if (names.length === 0) return "未分组";
  if (names.length <= 2) return names.join(" · ");
  return `${names.slice(0, 2).join(" · ")} 等 ${names.length} 组`;
}
