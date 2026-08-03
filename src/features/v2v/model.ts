import type { ChannelStat, ClipView, EffectiveParams, ModelInfo } from "@/lib/ipc";

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

/**
 * 每一档的显示元数据。
 *
 * ## 标签是**流程上的位置**，两个字
 *
 * 异常 / 缺词 / 就绪 / 远端 / 队列 / 验收 —— 一条视频从入队到交付会依次经过它们，
 * 所以侧栏那一列读下来就是这条流水线本身。旧标签（「处理异常」「去改写」「待放行」
 * 「机器手里」）混着动词、状态与拟人，长短还差一倍：在一列两位数计数旁边，
 * 参差的标签会让人以为它们不是同一级的东西。
 *
 * `note` 不再常驻显示（侧栏是全应用最挤的一列），它是那一行的 `title=`：
 * 回答的不是「这是什么」（标签已经说了），而是**「拿它怎么办，代价是什么」**。
 * 所以每一句都必须是恒真的事实：「多半是幽灵单」「出片中位 21 分钟」这类要看数据
 * 才成立的话一律不写进常量，那是行内 `situation` 的活（它读得到那一条的字段）。
 */
export const ACTION_META: Record<
  NextAction,
  { label: string; note: string; fg: string; dot: string }
> = {
  fix: {
    label: "异常",
    // 这一档里混着代价完全相反的两类，而它们长得一模一样 —— 所以这句话只说
    // 「先看花没花钱」，不替其中任何一类下结论。
    note: "无输出任务可恢复；可能已经计费的任务先核对远端",
    fg: "var(--er)",
    dot: "var(--sg-fail)",
  },
  rewrite: {
    // 「缺词」= 缺的是视频提示词。它比「去改写」准：卡住的不是某个动作没做，
    // 而是这一条还没有词可用 —— 而写词的地方不在 GenDesk 里。
    label: "缺词",
    note: "工单已物化，去 Claude Code / Codex 跑 v2v-rewrite",
    fg: "var(--acc2)",
    dot: "var(--sg-rewrite)",
  },
  submit: {
    label: "就绪",
    note: "词齐了、就差提交 —— 还没花钱，派发那一刻才扣",
    fg: "var(--acc2)",
    dot: "var(--sg-ready)",
  },
  review: {
    label: "验收",
    note: "出片了，等你判过还是毙",
    fg: "var(--st-rev)",
    dot: "var(--sg-rev)",
  },
  queued: {
    // 与「就绪」的分别就是这句话：人已经点过了，不必再点第二次。
    label: "队列",
    note: "已放行 · 在等这条通道的空位，出一条自动补一条",
    fg: "var(--t3)",
    dot: "var(--sg-queued)",
  },
  wait: {
    // 「远端」= 东西在即梦那边。这一档存在的意义是与前面几档形成对照 ——
    // 那几档卡在人身上，这一档不用管。
    label: "远端",
    note: "在即梦手上，出片会自动取回",
    fg: "var(--t3)",
    dot: "var(--sg-run)",
  },
  done: { label: "已定案", note: "", fg: "var(--t3)", dot: "var(--sg-pass)" },
};

/**
 * 六档在制动作 + 已定案，**按流程顺序**：异常 → 缺词 → 就绪 → 远端 → 队列 → 验收。
 *
 * 侧栏、摘要卡的动作构成、底坞的作用域判定三处都照这个序，所以它同时是
 * 「默认落在哪一档」的顺序（`enter()` 取第一个不为空的）。
 *
 * 异常排在最前不是流程位置而是优先级：那一档里躺着的是已经花过钱、
 * 或者正准备第二次花钱的条目。
 */
export const ACTION_ORDER: NextAction[] = [
  "fix",
  "rewrite",
  "submit",
  "wait",
  "queued",
  "review",
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
 * 即梦对**每条模型通道**各自动态限容。GenDesk 逐条读接单回执，首个明确未计费的
 * `ExceedConcurrencyLimit` 会停止当前通道并把条目放回 FIFO；其余尾部从未离开本地，
 * 而**别的通道不受影响**（0031）。
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

/**
 * 工作台的筛选：**动作 × 通道的交集**，两维各有各的控件。
 *
 * ## 为什么是交集
 *
 * 「2.0Fast 上这 12 条等着放行」是这一页最常问的一句话，而它只有交集答得出来。
 *
 * 单选试过一版，问题不在语义而在**控件长在哪**：那时两维摞在侧栏同一列里，
 * 九行里任何两行的高亮长得一模一样，人读到的是「选了两样东西」，读不出「其中一样
 * 在缩小另一样」。现在动作在侧栏（流程导航）、通道在列表顶上（就贴着被它筛的那张表），
 * 两个控件从位置到形状都不一样，「这一屏是这两个条件叠出来的」不必解释。
 *
 * ## 交集不许把数字变成谎话
 *
 * 交集最容易出的错是「侧栏写 12，点进去 3 条」。所以两处计数都是**分面计数**：
 * 每个数字都按**另一维当前的选择**算，于是它恒等于「点它之后会看到的条数」——
 * 侧栏那六个数按当前通道算，通道片那三个数按当前动作算。判据单点 `matchFilter`。
 */
export interface Filter {
  /** 流程档。**恒有一个** —— 工作台整页回答的就是「这一档还剩多少活」。 */
  action: NextAction;
  /** 通道。`null` = 不按通道筛（看这一档的全部）。 */
  channel: string | null;
}

/**
 * 侧栏动作卡的六行。
 *
 * **没有「已定案」**：`pass` 归成片库那一页，`rej` 的成片已经进了废纸篓 ——
 * 两者都不是「还剩多少活」的一部分，摆进来只会把真正的待办往下挤。
 */
export const WORKBENCH_ACTIONS: NextAction[] = ACTION_ORDER.filter((a) => a !== "done");

/**
 * 信号 = 与阶段正交的**例外**。
 *
 * 单看阶段回答不了「这批里有没有出事」：18 条幽灵单和 18 条正常排队在「已提交」列里
 * 长得一模一样，而它们的处置完全相反（一个直接重跑不花钱，一个必须继续等否则重复扣费）。
 */
/**
 * 信号仍然逐行算，只是**不再有一排筛选片**（v0.24.0，主轴搬进侧栏时一并去掉）。
 *
 * 它们没有变成装饰：`phantom`/`timeout` 决定 `nextAction` 把这一条送进「处理异常」，
 * 六个信号全部参与 `situation` 与详情栏那几句方向性提示，底坞的按钮组也读它们
 * 决定该摆「免费重跑」还是「继续等待」。少的只是「按信号筛一屏」这个入口。
 */
export type SignalKey = "phantom" | "timeout" | "slow" | "vip" | "auto";

/**
 * 列表的排序 —— **只有一种，没有开关**。
 *
 * 从前这里是三档（已等最久 / 额度最高 / 重跑最多）加一个栏头上的循环按钮。那个按钮
 * 去掉了：三档里只有「已等最久」真的被用过，另外两档存在的唯一后果是有人不小心点了
 * 一下、此后一直看着一个自己没意识到的顺序。
 *
 * 现在的顺序是 `rankRows`：**在跑的排最前，然后等得最久的**。它同时解决了另一件事 ——
 * 顶栏点一条通道灯进来时，「这条队上现在正在生成的是哪几条」必须是第一眼看到的东西，
 * 而那正是点那盏灯的理由。按动作筛的一屏里全体动作相同，这一层是恒等式，不改变什么。
 */
const RANK: Record<NextAction, number> = {
  wait: 0,
  queued: 1,
  fix: 2,
  review: 3,
  submit: 4,
  rewrite: 5,
  done: 6,
};

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

/** unix 秒 → 「09:14」。进度条用。 */
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
    // **通道口径只有 `channelOf` 一个**。这里原先自己拿 `??` 回落一遍，而 `??` 只兜
    // null/undefined —— 一个空白型号（`""`）会被它原样留下，于是同一条 clip 分节时按
    // `channelOf` 归到默认通道、`modelFull` 却是空串：筛选（`matchFilter` 比的是
    // `modelFull`）、通道卡与计数三处随后都读 `modelFull`，结果就是后台在默认通道上
    // 执行、界面却把它摆进另一条「CLI 默认」通道里，点哪一节都找不着它。
    const modelFull = key === "" ? null : key;
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
    if (vip) signals.add("vip");
    if (c.autoSubmitted) signals.add("auto");

    // 「情况」这一列必须点名**动作**与**代价**，而不是复述阶段（阶段就在旁边的色点上）。
    // 前四个关键词（疑幽灵单/等待异常/继续等待/免费重跑）保留原词：既有测试断言它们，
    // 而这四处指错方向的代价是真金白银 —— 幽灵单重跑不花钱，超时重跑要再花一份。
    let situation: string;
    let situationTone: Row["situationTone"] = "t3";
    if (phantomLive) {
      situation = "疑幽灵单 · 没入队也没扣费，可恢复";
      situationTone = "er";
    } else if (slow) {
      situation = `等待异常 · 已超同通道中位数 ${SLOW_FACTOR} 倍，别手动催`;
      situationTone = "wr";
    } else if (stage === "run" && c.awaitingDownload) {
      // 即梦已经做完了，卡的是**下载**（`query_result --download_dir` 走 CLI 自己的
      // 30 秒 HTTP 超时，大文件或网络抖动就会失败，下一轮自动恢复）。
      //
      // 没有这一格的话，这一行会掉进下面那条「位次问不到」——而它根本没在跑，
      // `queue_idx` 的 0 是「已出队」不是位次。人看到的会是一条「即梦在跑」挂在
      // 那里好几轮，而真相是片子早就好了、钱也扣完了，只差最后一步落盘。
      situation = "已出片 · 正在取回到本地，失败会自动恢复";
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
      situation = "可恢复 · 从未计费";
      situationTone = "er";
    } else if (stage === "fail" && c.errorType === "submit_timeout") {
      // 与上一条正好相反：幽灵单是「确认没花钱」，这一条是「不知道花没花」。
      // 提交的 CLI 被超时杀掉之前可能已经下过单，而 submit_id 随进程没了。
      // 直接重跑是这一格里最贵的一个误操作，所以这句话必须说「先核对」。
      situation = "提交超时 · 可能已扣费，核对后再恢复";
      situationTone = "er";
    } else if (stage === "fail" && c.errorType === "submit_interrupted") {
      situation = "提交中断 · 扣费未知，已禁止自动重提";
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
 * 筛选是否命中 —— 动作与通道的**交集**。
 *
 * 收 `Row` 而不是 `Stage`：幽灵判定要看队列位次与扣费回执，那是整行的事 ——
 * 一条 `run` 的幽灵单归「异常」，而单看阶段只看得出它在跑。
 *
 * 「已定案」不必在这里额外拦：六档动作里本来就没有那一格（`WORKBENCH_ACTIONS`），
 * 而 `action` 恒是其中之一，故 pass/rej 从定义上就进不来。
 *
 * 这是**唯一**的判据。两处分面计数（侧栏六个数、通道片三个数）也调它，
 * 所以「数字」与「点进去看到的条目」不可能分叉。
 */
export function matchFilter(r: Row, f: Filter): boolean {
  if (r.action !== f.action) return false;
  return f.channel == null || (r.modelFull ?? "") === f.channel;
}

/**
 * 即梦**收下过**这一单没有 —— 「从流水线删掉」这件事唯一要问的问题。
 *
 * 删除是真删行（`repo::remove` 是 `DELETE FROM v2v_clips`，没有废纸篓也没有撤销令牌），
 * 所以这条判据决定的不是能不能删，而是**删掉会丢什么**：
 *
 * - 没收下过 → 丢的只是这一条的改写提示词与尝试记录。人自己刚决定「这张图不做视频了」，
 *   那本来就是要丢的东西。
 * - 收下过 → 即梦那边可能还在跑、钱可能已经扣了，而删掉之后我们再也认不出那一单：
 *   片子取不回来，额度也退不了。这一类必须在确认卡上把话说完。
 *
 * 判据分三层，一层比一层弱：
 * 1. `clip.billed` —— Rust 读五处证据得出的结论，扣过费就一定被收下过。
 * 2. **幽灵单**是反例：即梦给了 submit_id，但从未入队、从未计费 —— 等于没收。
 *    这也正是它能免费重跑的原因，两处判据必须同源。
 * 3. 其余情况看有没有 submit_id：有回执就是收下了。
 *
 * `submit_timeout` / `submit_interrupted` 单列（`removalRisk` 里的 `"unknown"`）：CLI
 * 在回执落库前可能已经下过单，而 submit_id 没拿到或随进程一起没了 —— 这一类我们
 * **不知道**，不能当成「没收下过」顺手删掉。
 */
export function remoteAccepted(r: Row): boolean {
  if (r.clip.billed) return true;
  if (r.phantomLive || (r.stage === "fail" && r.clip.errorType === "phantom")) return false;
  return (r.clip.submitId ?? "").trim() !== "";
}

/** 删这一条的代价：即梦没收过（免费）· 收过（丢单丢钱）· 不知道收没收。 */
export type RemovalRisk = "free" | "held" | "unknown";

export function removalRisk(r: Row): RemovalRisk {
  if (remoteAccepted(r)) return "held";
  if (
    r.stage === "fail" &&
    (r.clip.errorType === "submit_timeout" || r.clip.errorType === "submit_interrupted")
  )
    return "unknown";
  return "free";
}

/** 当前筛选的「脸」—— 标题、一句话、以及它那个颜色。 */
export interface FilterFace {
  /** 「验收」，或叠了通道时的「验收 · 2.0Fast」。 */
  label: string;
  /** 副标题：这一档「拿它怎么办」；叠了通道时前面加一句它被缩到了哪条队上。 */
  sub: string;
  /** 动作的色，取自 `ACTION_META.dot` —— 主轴恒是动作，故颜色永远跟着它走。 */
  color: string;
  /** 语气：决定标题与计数用哪支强调色。 */
  mood: "er" | "rev" | "acc" | "t3";
}

/**
 * 筛选 → 它在三处（列表栏头 / 摘要卡 / 底坞）要显示的同一套身份。
 *
 * 单点在这里，是因为这三处**必须同色同名**：栏头写「验收」而摘要卡写别的颜色时，
 * 人第一反应是自己点错了。
 *
 * 叠了通道时颜色**仍取动作的**：一屏只该有一个主色，而主轴是流程
 * （侧栏那六档才是这一页的骨架）。通道是缩小范围的第二个条件，它的身份由标题里
 * 那半截名字和列表顶上那枚亮着的片子承担。
 */
export function filterFace(f: Filter, channels: Channel[]): FilterFace {
  const m = ACTION_META[f.action];
  const mood: FilterFace["mood"] =
    m.fg === "var(--er)"
      ? "er"
      : m.fg === "var(--st-rev)"
        ? "rev"
        : m.fg === "var(--t3)"
          ? "t3"
          : "acc";
  if (f.channel == null) return { label: m.label, sub: m.note, color: m.dot, mood };
  // 通道会在最后一条走完之后从清单里消失，而筛选还指着它。此时说实话，
  // 不回落到某条别的通道上 —— 那会让人以为自己看的是刚才那条队。
  const c = channels.find((x) => x.key === f.channel);
  const name = c?.label ?? (f.channel === "" ? "CLI 默认" : shortModel(f.channel));
  return {
    label: `${m.label} · ${name}`,
    sub:
      c == null
        ? "这条通道上已经没有条目了 —— 再点一次那枚片子看全部"
        : `只看 ${name} 这条队 · ${m.note}`,
    color: m.dot,
    mood,
  };
}

/** 在跑的排最前，然后等得最久的；再同则按 id 定死（否则每次重渲染顺序会漂）。 */
export function rankRows(rows: Row[]): Row[] {
  return [...rows].sort(
    (a, b) => RANK[a.action] - RANK[b.action] || b.waitSecs - a.waitSecs || a.clip.id - b.clip.id,
  );
}

/** 一条即梦通道。侧栏那张通道卡的一行，也是行左轨与摘要卡堆叠条的配色来源。 */
export interface Channel {
  /** 通道全名（`model_version`）。空串 = 设置里也没写默认型号，走 CLI 默认。 */
  key: string;
  /** 通道简写（`ModelInfo.label`，随模型清单从 Rust 下发）。 */
  label: string;
  vip: boolean;
  /**
   * 配色序号 0..`CHANNEL_TONES-1`。
   *
   * 按**通道名排序后的下标**取，不按显示顺序取 —— 显示顺序会随「哪条还在跑」漂移，
   * 而一条通道在侧栏、行左轨、堆叠条三处必须同色；颜色跟着状态变的话，
   * 「蓝色那条是谁」这个问题每隔几分钟就有一个新答案。
   */
  tone: number;
  /** 这条通道上涉及的提示词组名 —— 回答「这条队上跑的是哪几组」。 */
  title: string;
  /** 本通道全部条目（不受任何筛选影响）—— 含已定案的，`worstGroup` 要数毙掉的那些。 */
  rows: Row[];
  /**
   * 还没走完的条数 = `rows.length - counts.done`。
   *
   * 侧栏那一行显示的就是它，而**不是** `rows.length`：点进去看到的是 `matchFilter`
   * 筛出来的在制条目，两个数字必须是同一个数 —— 侧栏写 83、列表里躺着 61 的话，
   * 没人会认为是「另外 22 条已定案」，只会认为其中一个坏了。
   */
  live: number;
  /** 按下一步动作分桶（含 `done`）。 */
  counts: Record<NextAction, number>;
  /** 侧栏那一行的副行：这条队此刻的占用状况，或它贵在哪。没什么可说时为空串。 */
  note: string;
  /** 一句人话的全貌摘要（挂在侧栏那一行的 `title=` 上）。例外在前。 */
  headline: string;
  headlineTone: "er" | "acc" | "t3";
  /** 已定案 = 全部落在 pass/rej，没有任何一条还需要人或机器动。 */
  done: boolean;
}

/** 通道配色轮转档数。与 `globals.css` 里的 `--chn-0..3` 一一对应。 */
export const CHANNEL_TONES = 4;

/**
 * 摘要（`headline`）。
 *
 * 一句话既自带图例又直接说出数字 —— 它取代过一条无图例的分段条，
 * 而那条分段条唯一的图例是 `title=` tooltip，即：没人答得上来。
 *
 * 「这一批 N 条」那个前缀去掉了（0032）：一条通道里的条目来自若干个批次，
 * 「这一批」会直接把人指错方向。总数由第一句的语义承担。
 */
function headlineOf(all: Row[], counts: Record<NextAction, number>) {
  const parts: string[] = [];
  if (counts.fix > 0) parts.push(`${counts.fix} 条出了异常`);
  if (counts.rewrite > 0) parts.push(`${counts.rewrite} 条等你改写`);
  if (counts.submit > 0) parts.push(`${counts.submit} 条等你放行`);
  if (counts.review > 0) parts.push(`${counts.review} 条等你验收`);
  if (counts.queued > 0) parts.push(`${counts.queued} 条在本地排队`);
  if (counts.wait > 0) parts.push(`${counts.wait} 条在即梦跑`);
  // 已定案的那几条必须报出来，否则这句话与旁边那个计数**加不起来**：摘要卡右边写的是
  // 在制条数（8），这里的「共 13 条」后面却只列得出 8 条的去向 —— 差出来的 5 条
  // 没有任何一处交代，而它们就在这条通道的历史里躺着。
  //
  // `parts` 为空（全定案）时不加：那一支走的是下面那句「已全部定案」，
  // 再补一个「13 条已定案」等于同一件事说两遍。
  if (counts.done > 0 && parts.length > 0) parts.push(`另 ${counts.done} 条已定案`);
  const headlineTone: Channel["headlineTone"] =
    counts.fix > 0 ? "er" : counts.rewrite + counts.submit + counts.review > 0 ? "acc" : "t3";
  const headline =
    parts.length === 0
      ? `共 ${all.length} 条 · 已全部定案`
      : `共 ${all.length} 条 · ${parts.join("，")}`;
  return { headline, headlineTone };
}

/**
 * 按**通道**归拢（0032 起的分组维度，v0.24.0 起从「分节」变成「侧栏筛选器」）。
 *
 * ## 为什么维度是通道
 *
 * 即梦按模型通道各排各的队，一条通道排满与另一条能不能发**毫无关系**。而一个批次的
 * 条目会分散到不同通道上 —— 按批次分组时每一个组内数字都不指向任何真实的队列：
 * 「本批已出 0/49」的分子分母跨着几条互不相干的队，对它们做的任何批量动作
 * （尤其是换通道）都不成立。按通道分之后，一条 = 一条队 = 一个可以整体处置的单位。
 *
 * 没写 `model_version` 的条目归**默认通道**那一条 —— 那本来就是它们会走的通道
 * （`channelOf` 与 Rust 的 `runner::channel_of` 同口径），另设一个「未定」组反而会把
 * 同一条真实队列在界面上劈成两半。
 *
 * ## 与「分节」时代唯一的规则差异：空的通道**不消失**
 *
 * 分节时代有一条规则是「当前筛选下一行都不显示的整节消失」—— 那是对的，因为节是
 * **内容容器**：一个只写着「当前筛选下没有条目」的空壳节头不回答任何问题，只会把真正
 * 命中的那一节挤下去。
 *
 * 但这里产出的是**筛选器**，规则必须反过来：只要这条通道上还有条目，它就得列出来。
 * 一个把「你正要切过去的那条通道」藏起来的筛选器，等于让人没法从「2.0Fast 一条都没有」
 * 走到「2.0Mini 有 9 条」—— 而那正是打开这张卡要做的事。故这里只收 `all`，
 * 不收 `visible`：可见性是调用方按 (动作 × 通道) 自己算的，与「有哪几条通道」无关。
 *
 * ## 排序：还在动的排前面
 *
 * 远端在跑 > 本地压着队 > 其余，同档按条数多的在前。通道之间没有批次那样天然的
 * 时间序，唯一有意义的先后是「哪条还有账要算」。
 */
export function buildChannels(
  all: Row[],
  models: ModelInfo[] = [],
  /** 逐通道的实时占用（`QueueStats.channels`）—— 只用来写那句副行，缺了就不写。 */
  stats: readonly ChannelStat[] = [],
): Channel[] {
  const groups = new Map<string, Row[]>();
  for (const r of all) {
    const key = r.modelFull ?? "";
    const bucket = groups.get(key);
    if (bucket) bucket.push(r);
    else groups.set(key, [r]);
  }

  // 配色按**名字序**定死（见 `Channel.tone` 的注释）：显示顺序会随状态漂移，颜色不能跟着漂。
  const toneOf = new Map(
    [...groups.keys()].sort((a, b) => a.localeCompare(b)).map((k, i) => [k, i % CHANNEL_TONES]),
  );

  const out: Channel[] = [];
  for (const [key, rows] of groups) {
    const info = models.find((m) => m.modelVersion === key);
    const vip = info?.vip ?? false;
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
      vip,
      tone: toneOf.get(key) ?? 0,
      title: channelTitle(rows),
      rows,
      live: rows.length - counts.done,
      counts,
      note: channelNote(
        vip,
        stats.find((s) => s.modelVersion === key),
      ),
      ...headlineOf(rows, counts),
      done: rows.every((r) => !isLive(r.stage)),
    });
  }
  // 还在动的排前面；同档按条数多的在前，再同则按名字定死顺序（否则每次重渲染的
  // 顺序会随 Map 插入序漂移）。
  const rank = (c: Channel) => (c.counts.wait > 0 ? 0 : c.counts.queued > 0 ? 1 : 2);
  out.sort(
    (a, b) =>
      rank(a) - rank(b) ||
      b.counts.wait + b.counts.queued - (a.counts.wait + a.counts.queued) ||
      b.rows.length - a.rows.length ||
      a.key.localeCompare(b.key),
  );
  return out;
}

/** 快捷通道位数。顶栏灯与列表筛选条**同为三格**，且必须是同一批通道。 */
export const TOP_CHANNELS = 3;

/**
 * 用得最多的前三条通道 —— 顶栏那排状态灯与列表顶上那排快捷筛选**共用这一份**。
 *
 * ## 为什么是「常驻前三」而不是「此刻在动的全部」
 *
 * 从前顶栏只显示 `running > 0 || queued > 0` 的通道，于是那排灯的**格数会变**：
 * 一条通道跑完最后一单就整格消失，下一次提交又冒出来。人靠位置记东西 ——
 * 「左边第二格是 2.0Mini」在这种排布下一天要重学好几次，而它每次重学的代价是
 * 看错一条队的占用。固定三格之后位置是稳的，「有没有在动」交给灯本身回答。
 *
 * ## 排序：先看还有多少活，再看历史上用得多不多
 *
 * `live`（还没走完的条数）优先 —— 那是现在能动手的量；同档再按这条通道上的总条数，
 * 于是常用通道即便此刻闲着也留在原位。两级都同时按名字定死，避免每次重渲染漂移。
 */
export function topChannels(channels: Channel[], n: number = TOP_CHANNELS): Channel[] {
  return [...channels]
    .sort((a, b) => b.live - a.live || b.rows.length - a.rows.length || a.key.localeCompare(b.key))
    .slice(0, n);
}

/**
 * 顶栏通道：活跃通道一条都不截断；少于三条时再用常用空闲通道补位。
 */
export function statusChannels(
  channels: Channel[],
  stats: readonly ChannelStat[],
  minimum: number = TOP_CHANNELS,
): Channel[] {
  const activeKeys = new Set(
    stats.filter((s) => s.running > 0 || s.queued > 0).map((s) => s.modelVersion),
  );
  const active = channels.filter((c) => activeKeys.has(c.key));
  if (active.length >= minimum) return active;
  const fillers = topChannels(
    channels.filter((c) => !activeKeys.has(c.key)),
    minimum - active.length,
  );
  return [...active, ...fillers];
}

/**
 * 侧栏通道行的副行 —— 只说**这条队此刻堵没堵**，或**它贵在哪**。
 *
 * 「并发已满」排在 vip 前面：前者是此刻会改变决策的事（新单发不出去，该换条队），
 * 后者是一条恒真的成本提醒。恒真的那句什么时候看都还在，会变的那句错过就没了。
 */
function channelNote(vip: boolean, s: ChannelStat | undefined): string {
  if (s && s.queued > 0 && s.running >= s.limit) {
    return `并发已满 · ${s.queued} 条在本地等空位`;
  }
  if (vip) return "贵 5.5 倍 · 买到的只是不排队";
  if (s && s.running > 0) return `并发 ${s.running} / ${s.limit}`;
  if (s && s.queued > 0) return `${s.queued} 条在本地排队`;
  return "";
}

/**
 * 通道标题 = 这条通道上涉及的提示词组名。
 *
 * 组名是人给这批片子起的名（一份 txt = 一个组），而通道本身只是一条队 ——
 * 光看「2.0Fast」答不出「这条队上跑的是什么」。混多个组时只列前两个 ——
 * 铺满整行的组名反而什么都读不出来（同作品库分节的处置）。
 */
function channelTitle(rows: Row[]): string {
  const names: string[] = [];
  for (const r of rows) {
    const n = r.clip.groupName.trim();
    if (n !== "" && !names.includes(n)) names.push(n);
  }
  if (names.length === 0) return "未分组";
  if (names.length <= 2) return names.join(" · ");
  return `${names.slice(0, 2).join(" · ")} 等 ${names.length} 组`;
}

/** 当前 (动作 × 通道) 这一屏的账 —— 摘要卡那三个小格与通道构成条。 */
export interface SliceSummary {
  count: number;
  /** 已经真的扣掉的额度合计（`clip.billed` 为准，与「重跑要不要再花一份钱」同源）。 */
  billed: number;
  /** 还没花的那部分（预估）。 */
  unbilled: number;
  /** 两者都不为 0 —— 此时摆一个合计数会把「已经花掉的」和「打算花的」混成一笔糊涂账。 */
  mixed: boolean;
  /** 查不到单价、没能计入的条数。界面据此显示「≥」，绝不摆一个编出来的数字。 */
  unpriced: number;
  /** 这一屏里等得最久的那条（秒）。0 = 没有一条在等。 */
  oldestWait: number;
  /**
   * 通道构成（堆叠条 + chips）。顺序与 `buildChannels` 一致。
   *
   * **只有这一种构成**：筛选改成交集之后，一屏恒是**一个动作**，所以「这一屏分成哪几种
   * 动作」是个恒等式，画出来是一整条纯色。这里唯一还值得问的是「这一档分散在哪几条队上」。
   * （叠了通道时它同样退化成一条，故调用方那时整块不画。）
   */
  channels: { key: string; label: string; tone: number; n: number }[];
}

export function sliceSummary(rows: Row[], channels: Channel[]): SliceSummary {
  let billed = 0;
  let unbilled = 0;
  let unpriced = 0;
  let oldestWait = 0;
  for (const r of rows) {
    const amount = r.credit ?? r.estimate;
    if (amount == null) unpriced += 1;
    else if (r.clip.billed) billed += amount;
    else unbilled += amount;
    if (r.waitSecs > oldestWait) oldestWait = r.waitSecs;
  }
  const n = new Map<string, number>();
  for (const r of rows) {
    const k = r.modelFull ?? "";
    n.set(k, (n.get(k) ?? 0) + 1);
  }
  return {
    count: rows.length,
    billed,
    unbilled,
    mixed: billed > 0 && unbilled > 0,
    unpriced,
    oldestWait,
    channels: channels
      .filter((c) => (n.get(c.key) ?? 0) > 0)
      .map((c) => ({ key: c.key, label: c.label, tone: c.tone, n: n.get(c.key) ?? 0 })),
  };
}

/** 进度上的一步。`now` 恒有且只有一步 —— 它就是「现在卡在哪」。 */
export type TrailState = "done" | "now" | "soon";

export interface TrailStep {
  key: string;
  /** 「09:14」，没发生过就是「—」。**绝不编时间**。 */
  at: string;
  what: string;
  /** 副行：`now` 那一步写「所以现在该干嘛」，`done` 那一步写回执一类的证据。 */
  sub: string;
  state: TrailState;
  /**
   * 这一步的颜色。**走完的一步一律 `ok`**（界面上是一枚绿勾）——
   * 「走到过这里」是一件只有成立与不成立的事，给它分四种颜色等于让人去猜
   * 蓝勾和琥珀勾差在哪。剩下三种色只描述例外：`er` 出事了、`rev` 等你判、
   * `dim` 还没轮到（或这条路已经不会再走下去，比如判了不通过）。
   */
  tone: "ok" | "acc" | "rev" | "er" | "dim";
}

/**
 * 这一条走过与将要走的路。
 *
 * 与旧版（只有四个时刻、发生过才有颜色）的差别是**把未来也画出来**：
 * 一条缺词的条目，光看「入队 09:14」答不出「后面还有几步、下一步归谁」。
 * 三态里 `now` 那一步带一句「所以现在该干嘛」，其余留白 —— 没发生的事不编时间。
 */
export function trailOf(row: Row): TrailStep[] {
  const c = row.clip;
  const st = row.stage;
  const out: TrailStep[] = [
    {
      key: "accept",
      at: fmtClock(c.createdAt),
      what: "图片验收通过 · 自动入队待改写",
      sub: "",
      state: "done",
      tone: "ok",
    },
  ];

  out.push(
    c.rewroteAt != null
      ? {
          key: "rewrite",
          at: fmtClock(c.rewroteAt),
          what: "改写结果写回 · 进就绪",
          sub: "",
          state: "done",
          tone: "ok",
        }
      : {
          key: "rewrite",
          at: "—",
          what: "等 skill 写回改写结果",
          // 「去 Claude Code 跑 v2v-rewrite」不写在这儿：那句话在这一屏上已经由
          // 摘要卡的副行说过一次，而底坞那两个按钮就是它的动作。同一条指令在
          // 一屏里出现四遍，读到第二遍时人就不再读了。
          sub: "",
          state: st === "rewrite" ? "now" : "soon",
          tone: st === "rewrite" ? "acc" : "dim",
        },
  );

  out.push(
    c.firstSubmittedAt != null
      ? {
          key: "submit",
          at: fmtClock(c.firstSubmittedAt),
          what: `提交到即梦${c.submitCredit != null ? ` · 回执计费 ${c.submitCredit}` : " · 回执未带计费"}`,
          // 常驻队列放行的条目此前只能靠一个筛选片才看得出来 —— 而「这一单是谁下的」
          // 恰恰是对账时第一个要问的。写进回执这一行，不必再有那个筛选片。
          sub: `${c.submitId ?? "回执里没有 submit_id"}${c.autoSubmitted ? " · 常驻队列放行" : ""}`,
          state: "done",
          tone: "ok",
        }
      : {
          key: "submit",
          at: "—",
          what: row.action === "queued" ? "已放行 · 等这条通道的空位" : "等你放行提交",
          sub:
            st !== "ready"
              ? ""
              : row.action === "queued"
                ? "出一条自动补一条，不必再点确认"
                : "选好通道按 ⌘⏎ 确认提交 —— 那一刻才扣费",
          state: st === "ready" ? "now" : "soon",
          tone: st === "ready" ? "acc" : "dim",
        },
  );

  if (st === "fail") {
    // 判死之后没有「等你判定」那一步 —— 画一个永远不会走到的未来，比不画更误导。
    out.push({
      key: "finish",
      at: c.finishedAt == null ? "—" : fmtClock(c.finishedAt),
      what: c.errorType === "timeout" ? "判超时 · 提交单仍有效" : `判死 · ${c.errorType ?? "失败"}`,
      sub: row.situation,
      state: "now",
      tone: "er",
    });
    return out;
  }

  out.push(
    c.finishedAt != null
      ? {
          key: "finish",
          at: fmtClock(c.finishedAt),
          what: `出片落盘${c.width != null ? ` ${c.width}×${c.height}` : ""} · 进验收`,
          sub: "",
          state: "done",
          tone: "ok",
        }
      : {
          key: "finish",
          at: "—",
          what: "等即梦出片",
          sub: st === "run" ? row.situation : "",
          state: st === "run" ? "now" : "soon",
          tone: st === "run" ? "acc" : "dim",
        },
  );

  out.push(
    c.reviewedAt != null
      ? {
          key: "review",
          at: fmtClock(c.reviewedAt),
          what: st === "pass" ? "验收通过" : "验收不通过 · 成片进废纸篓",
          // 拷贝失败不回滚验收，所以「通过了却没落地」是一个真实会出现的状态。
          sub: st === "pass" && !delivered(c) ? "未交付到输出目录" : "",
          state: st === "pass" && !delivered(c) ? "now" : "done",
          tone: st === "pass" ? (delivered(c) ? "ok" : "er") : "dim",
        }
      : {
          key: "review",
          at: "—",
          what: "等你判定",
          sub: "",
          state: st === "rev" ? "now" : "soon",
          tone: st === "rev" ? "rev" : "dim",
        },
  );
  return out;
}
