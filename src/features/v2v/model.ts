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
 * 即梦的并发上限是账户级的（实测非 VIP 只跑得下 1 条），超出的部分会被它逐条
 * 以 `ExceedConcurrencyLimit` 弹回来。所以 GenDesk 只发得下的那几条，其余留在本地。
 * 这一格与「等你点确认提交」必须分开显示 —— 在此之前两者长得一模一样，
 * 于是一批放行完的片子看起来像是没人管。
 */
export function isQueued(c: ClipView): boolean {
  return c.stage === "ready" && c.submitQueuedAt != null;
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
  { key: "slow", label: "等待异常", title: "已超同批在跑条目中位等待时长的 3 倍。" },
  { key: "rerun", label: "重跑过", title: "尝试次数 > 1，即同一张图已经花过不止一份额度。" },
  { key: "vip", label: "vip 通道", title: "同规格贵 5.5 倍，买到的只是不排队。" },
  { key: "auto", label: "常驻队列", title: "由自动补单放行的条目，不是你手动提交的。" },
];

export const SORTS = {
  batch: "批次倒序",
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

/** 「seedance2.0fast_vip」→「2.0fast_vip」。表格里那一列只有 86px。 */
export function shortModel(full: string): string {
  return full.replace(/^seedance/, "");
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
  /** 即梦同时跑得下几条（`QueueStats.inFlightLimit`）。只用于文案，算不出来时按 1。 */
  inFlightLimit = 1,
): Row[] {
  // 本地队列位次：严格照后端取用的顺序（放行时刻，其次 id）算一遍。
  // 两边分叉的代价是界面说「你排第 1」而实际先发的是另一条 —— 那种错没人查得出来。
  const queueOrder = clips
    .filter(isQueued)
    .sort((a, b) => (a.submitQueuedAt ?? 0) - (b.submitQueuedAt ?? 0) || a.id - b.id)
    .map((c) => c.id);
  const queuePosOf = new Map(queueOrder.map((id, i) => [id, i + 1]));
  // 「等待异常」是相对判据：跟同一批的其它在跑条目比，而不是跟一个拍脑袋的绝对秒数比。
  // 按批分组取中位数 —— 不同批次的提交时刻差着几小时，混在一起算出来的中位数谁也不代表。
  const runWaits = new Map<string, number[]>();
  for (const c of clips) {
    if (c.stage !== "run") continue;
    const key = String(c.batchId ?? "none");
    const w = c.firstSubmittedAt == null ? 0 : Math.max(0, now - c.firstSubmittedAt);
    const bucket = runWaits.get(key);
    if (bucket) bucket.push(w);
    else runWaits.set(key, [w]);
  }
  const medians = new Map<string, number>();
  for (const [k, v] of runWaits) medians.set(k, median(v));

  // 「本批已出 X/Y」：排队中那一行要给的是**进度**，而不是又一句「排队中」。
  const batchTotal = new Map<string, number>();
  const batchDone = new Map<string, number>();
  for (const c of clips) {
    const key = String(c.batchId ?? "none");
    batchTotal.set(key, (batchTotal.get(key) ?? 0) + 1);
    if (c.stage === "rev" || c.stage === "pass" || c.stage === "rej") {
      batchDone.set(key, (batchDone.get(key) ?? 0) + 1);
    }
  }

  return clips.map((c) => {
    const stage = c.stage as Stage;
    const key = String(c.batchId ?? "none");
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
      situation = `等待异常 · 已超本批中位数 ${SLOW_FACTOR} 倍，别手动催`;
      situationTone = "wr";
    } else if (stage === "run") {
      // 位次是排队几小时里**唯一**有意义的进度：「第 4485 位」和「第 12 位」是两件
      // 完全不同的事。问不到时才回落到本批进度 —— 绝不编一个位次出来。
      situation =
        c.queueIdx != null && c.queueIdx > 0
          ? `即梦在排队 · 前面还有 ${c.queueIdx} 个`
          : `即梦在跑 · 本批已出 ${batchDone.get(key) ?? 0}/${batchTotal.get(key) ?? 0}`;
    } else if (isTimeout) {
      situation = "继续等待 · 额度已扣，即梦还在跑";
      situationTone = "er";
    } else if (stage === "fail" && c.errorType === "phantom") {
      situation = "免费重跑 · 从未计费";
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
      situation =
        queuePos != null && queuePos > 1
          ? `已放行 · 本地排第 ${queuePos}，即梦同时只跑 ${inFlightLimit} 条`
          : `已放行 · 下一个就发它（即梦同时只跑 ${inFlightLimit} 条）`;
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
      modelShort: modelFull ? shortModel(modelFull) : "CLI 默认",
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

/** 搜索：编号 / 组名 / 视频提示词 / 生图提示词 / submit_id 一次覆盖。 */
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
    String(c.batchId ?? "").includes(needle)
  );
}

export function sortRows(rows: Row[], sort: SortKey): Row[] {
  const out = [...rows];
  if (sort === "wait") out.sort((a, b) => b.waitSecs - a.waitSecs);
  else if (sort === "credit") out.sort((a, b) => (b.credit ?? -1) - (a.credit ?? -1));
  else if (sort === "attempt") out.sort((a, b) => b.clip.attempt - a.clip.attempt);
  // batch：后端已按 group_id, id 返回；分节本身按批次倒序，节内保持验收序。
  return out;
}

/** 一节（= 一个批次）。 */
export interface Section {
  key: string;
  batchId: number | null;
  title: string;
  /** 本批全部条目（不受筛选影响）—— 摘要与「已定案」判定要看全貌。 */
  all: Row[];
  /** 当前筛选下这一批还剩哪些行。 */
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
 */
function headlineOf(all: Row[], counts: Record<NextAction, number>) {
  const parts: string[] = [];
  if (counts.fix > 0) parts.push(`${counts.fix} 条出了异常`);
  if (counts.rewrite > 0) parts.push(`${counts.rewrite} 条等你改写`);
  if (counts.submit > 0) parts.push(`${counts.submit} 条等你放行`);
  if (counts.review > 0) parts.push(`${counts.review} 条等你验收`);
  if (counts.queued > 0) parts.push(`${counts.queued} 条排队等发`);
  if (counts.wait > 0) parts.push(`${counts.wait} 条在即梦跑`);
  const headlineTone: Section["headlineTone"] =
    counts.fix > 0 ? "er" : counts.rewrite + counts.submit + counts.review > 0 ? "acc" : "t3";
  const headline =
    parts.length === 0
      ? `这一批 ${all.length} 条 · 已全部定案`
      : `这一批 ${all.length} 条 · ${parts.join("，")}`;
  return { headline, headlineTone };
}

/**
 * 按批次分节。批次倒序（最近一批在最上），无批次的历史条目垫底。
 *
 * **当前筛选下一条都不显示的批次整节消失**，无论它是否还有活。
 *
 * 旧规则只砍「已定案」的空节，于是筛「处理异常」时几十个还在跑的批次会留下几十个
 * 只写着「当前筛选下这一批没有条目」的空壳节头，把真正的三条待办推到屏幕外面 ——
 * 用户那句「筛选项随便选一个都会保留每一个分组」说的就是这个。旧规则的理由是
 * 「分段条正是这一批做到哪了的答案，所以空节也该留着」；分段条没了，理由也就没了。
 *
 * 条目不会因此变得无处可寻：成片在成片库，毙掉的选「未通过」筛选片还能翻出来
 * （那时 `rows` 非空，这一节就会重新出现）。
 */
export function buildSections(all: Row[], visible: Row[]): Section[] {
  const groups = new Map<string, Row[]>();
  for (const r of all) {
    const key = String(r.clip.batchId ?? "none");
    const bucket = groups.get(key);
    if (bucket) bucket.push(r);
    else groups.set(key, [r]);
  }
  const visByKey = new Map<string, Row[]>();
  for (const r of visible) {
    const key = String(r.clip.batchId ?? "none");
    const bucket = visByKey.get(key);
    if (bucket) bucket.push(r);
    else visByKey.set(key, [r]);
  }

  const out: Section[] = [];
  for (const [key, rows] of groups) {
    const visRows = visByKey.get(key) ?? [];
    if (visRows.length === 0) continue;
    const batchId = key === "none" ? null : Number(key);
    const counts = Object.fromEntries(ACTION_ORDER.map((a) => [a, 0])) as Record<
      NextAction,
      number
    >;
    for (const r of rows) counts[r.action] += 1;
    out.push({
      key,
      batchId,
      title: sectionTitle(rows),
      all: rows,
      rows: visRows,
      counts,
      ...headlineOf(rows, counts),
      done: rows.every((r) => !isLive(r.stage)),
      createdAt: Math.max(...rows.map((r) => r.clip.createdAt)),
    });
  }
  // 批次倒序；无批次的（历史）永远垫底。
  out.sort((a, b) => {
    if (a.batchId == null) return 1;
    if (b.batchId == null) return -1;
    return b.batchId - a.batchId;
  });
  return out;
}

/**
 * 分节标题 = 本批涉及的提示词组名。
 *
 * 批次本身没有名字（`batches` 表只有 id 与备注），而组名恰好就是人给这批片子起的名
 * （一份 txt = 一个组）。一批混多个组时只列前两个 —— 铺满整行的组名反而什么都读不出来
 * （同作品库分节的处置）。
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
