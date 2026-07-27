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

export const STAGE_ORDER: Stage[] = ["rewrite", "ready", "run", "rev", "pass", "rej", "fail"];

/**
 * 「还在制」的阶段 —— 工作台只管这些。
 *
 * `pass` 与 `rej` 都已经定案：前者去成片库，后者是一个已经做完的决定。把它们留在
 * 工作台上，「这里还剩多少活」这个问题就再也答不准了 —— 实测 18 条验收通过的片子
 * 一直挂在看板上，人得先在心里把它们减掉才能看出真正的待办。
 *
 * `fail` 算在制：它等着人决定重跑还是继续等，那是活。
 */
export const LIVE_STAGES: Stage[] = ["rewrite", "ready", "run", "rev", "fail"];

export function isLive(stage: Stage): boolean {
  return stage !== "pass" && stage !== "rej";
}

/** 阶段筛选片。`need` 是默认值 —— 一进页面该看到的是「等你动手的」，不是全部。 */
export type StageFilter = Stage | "need" | "all";

/**
 * 工作台的阶段筛选片。
 *
 * **没有「成片」**：验收通过的视频归成片库那一页，它们已经不是流水线的事了。
 * 「未通过」留着当逃生舱 —— 那些条目不该变得无处可寻，只是不该默认占位。
 */
export const STAGE_CHIPS: { key: StageFilter; label: string }[] = [
  { key: "need", label: "需要我" },
  { key: "all", label: "全部在制" },
  ...LIVE_STAGES.map((s) => ({ key: s as StageFilter, label: STAGE_META[s].label })),
  { key: "rej", label: STAGE_META.rej.label },
];

/**
 * 信号 = 与阶段正交的**例外**。
 *
 * 单看阶段回答不了「这批里有没有出事」：18 条幽灵单和 18 条正常排队在「已提交」列里
 * 长得一模一样，而它们的处置完全相反（一个直接重跑不花钱，一个必须继续等否则重复扣费）。
 */
export type SignalKey = "phantom" | "timeout" | "slow" | "rerun" | "vip" | "noasset" | "auto";

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
// `noasset` 只对成片有意义，而成片不在工作台 —— 它的筛选片在成片库那一页。

export const SORTS = {
  batch: "批次倒序",
  wait: "已等最久",
  credit: "额度最高",
  attempt: "重跑最多",
} as const;
export type SortKey = keyof typeof SORTS;

/** 「等待异常」的判据倍数。中位数的 3 倍 —— 单条慢不算事，慢出一个数量级才是。 */
const SLOW_FACTOR = 3;

/** 幽灵单的宽限期（秒），与 Rust 侧 `runner::PHANTOM_GRACE_SECS` 同值。 */
export const PHANTOM_GRACE_SECS = 15 * 60;

/** 表格一行所需的全部派生值。 */
export interface Row {
  clip: ClipView;
  stage: Stage;
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
  /** 在跑但两个信号双双缺席且过了宽限期 —— 与 Rust 判据同构。 */
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
): Row[] {
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

    const waitSecs = c.firstSubmittedAt == null ? 0 : Math.max(0, now - c.firstSubmittedAt);
    const polledAgo = c.polledAt == null ? null : Math.max(0, now - c.polledAt);

    // 与 Rust 侧 `runner::is_phantom` 同构：两个信号**同时**缺席才算，且过了宽限期。
    // 只看队列位次会在即梦哪天不下发 queue_info 时把已扣费的任务误判成没花钱。
    const phantomLive =
      stage === "run" &&
      c.submitCredit == null &&
      c.queueIdx == null &&
      c.creditCount == null &&
      waitSecs > PHANTOM_GRACE_SECS;

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

    const signals = new Set<SignalKey>();
    if (isPhantom) signals.add("phantom");
    if (isTimeout) signals.add("timeout");
    if (slow) signals.add("slow");
    if (c.attempt > 1) signals.add("rerun");
    if (modelFull?.endsWith("_vip")) signals.add("vip");
    if (c.autoSubmitted) signals.add("auto");
    if (stage === "pass" && !c.inAssetLib) signals.add("noasset");

    let situation: string;
    let situationTone: Row["situationTone"] = "t3";
    if (phantomLive) {
      situation = "疑幽灵单 · 无位次、无计费";
      situationTone = "er";
    } else if (slow) {
      situation = `等待异常 · 已超本批中位数 ${SLOW_FACTOR} 倍`;
      situationTone = "wr";
    } else if (stage === "run") {
      situation = `排队中 · 本批已出 ${batchDone.get(key) ?? 0}/${batchTotal.get(key) ?? 0}`;
    } else if (isTimeout) {
      situation = "继续等待 · 额度已扣";
      situationTone = "er";
    } else if (stage === "fail" && c.errorType === "phantom") {
      situation = "免费重跑 · 从未计费";
      situationTone = "er";
    } else if (stage === "fail") {
      situation = `失败 · ${c.errorType ?? "原因见执行日志"}`;
      situationTone = "er";
    } else if (stage === "rev") {
      situation = "等你判定 · 已落盘";
    } else if (stage === "pass") {
      situation = c.inAssetLib ? "已入资产库" : "可入资产库 · 尚未入库";
    } else if (stage === "rej") {
      situation = "已毙 · 成片进废纸篓";
    } else if (stage === "ready") {
      situation = "等你放行 · 改写完成";
      situationTone = "acc";
    } else {
      situation = "等 skill 写回 · 交接已物化";
    }

    return {
      clip: c,
      stage,
      modelFull,
      modelShort: modelFull ? shortModel(modelFull) : "CLI 默认",
      vip: modelFull?.endsWith("_vip") ?? false,
      duration,
      resolution,
      estimate,
      credit,
      creditEstimated,
      waitSecs,
      polledAgo,
      phantomLive,
      slow,
      signals,
      situation,
      situationTone,
    };
  });
}

/**
 * 阶段筛选是否命中。
 *
 * - `need` = 人能立刻动手的三种（放行 / 判定 / 处置异常）。
 * - `all` = **全部在制**，不含已定案的 pass / rej。工作台回答的是「还剩多少活」，
 *   把做完的算进去，这个数就再也不准了。要看成片去成片库，要看毙掉的选「未通过」。
 */
export function matchStage(stage: Stage, filter: StageFilter): boolean {
  if (filter === "all") return isLive(stage);
  if (filter === "need") return stage === "ready" || stage === "rev" || stage === "fail";
  return stage === filter;
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
  /** 本批全部条目（不受筛选影响）—— 分段条与「已定案」判定要看全貌。 */
  all: Row[];
  /** 当前筛选下这一批还剩哪些行。 */
  rows: Row[];
  /** 阶段混合分段条。 */
  seg: { stage: Stage; pct: number }[];
  legend: string;
  /** 已定案 = 全部落在 pass/rej，没有任何一条还需要人或机器动。 */
  done: boolean;
  createdAt: number;
}

/**
 * 按批次分节。批次倒序（最近一批在最上），无批次的历史条目垫底。
 *
 * **全部定案的批次整节消失**，不是折叠成一行 —— 一行也是行，几十批做完之后
 * 那些「已定案」的条目会把真正在跑的那两批挤到屏幕外面去。它们不会丢：
 * 成片在成片库，毙掉的选「未通过」筛选片还能翻出来（那时 `rows` 非空，这一节
 * 就会重新出现）。
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
    const done = rows.every((r) => !isLive(r.stage));
    // 定案了、当前筛选下又一条都不显示 → 整节不出现。
    // 第二个条件是逃生舱：显式筛「未通过」时这一节会重新出现，条目不会变得无处可寻。
    if (done && visRows.length === 0) continue;
    const batchId = key === "none" ? null : Number(key);
    const mix = new Map<Stage, number>();
    for (const r of rows) mix.set(r.stage, (mix.get(r.stage) ?? 0) + 1);
    const total = rows.length || 1;
    // 分段条按七态固定顺序画，而不是按 Map 的插入序 —— 否则同一批的色块会
    // 因为某一条状态变了而整条重排，看着像换了一批。
    const seg = STAGE_ORDER.filter((s) => mix.has(s)).map((s) => ({
      stage: s,
      pct: ((mix.get(s) ?? 0) / total) * 100,
    }));
    out.push({
      key,
      batchId,
      title: sectionTitle(rows),
      all: rows,
      rows: visRows,
      seg,
      legend: STAGE_ORDER.filter((s) => mix.has(s))
        .map((s) => `${STAGE_META[s].label} ${mix.get(s)}`)
        .join(" · "),
      done,
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
