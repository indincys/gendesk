/**
 * 时间线分段：**日 → 任务簇** —— 纯函数，与 React 无关，故可单独测。
 *
 * 验收页与废纸篓共用。两页问的是同一个问题：「这些图是什么时候、在哪一次任务里
 * 弄出来的」。放在 `_shared/` 而不是各留一份 —— 「隔多久算另一次任务」这种判断
 * 在两页上分叉之后，同一批图在两页会被切成不同的段，而人恰恰是拿这两页对照着看的。
 *
 * ## 两级，各自有各自的理由
 *
 * 1. **日**（今天 / 昨天 / 前天 / 具体日期）。跨天一定要断开：昨晚那批和今天凌晨
 *    那批在时间轴上可能只隔两小时，但在人的记忆里是两件事。
 * 2. **任务簇**。同一天里，相邻两条只要**隔得够久**或**不属于同一次任务**就断开。
 *    两条判据缺一不可：
 *    - 只按时间切 → 连着跑的两批（只隔几秒）会被并成一段；
 *    - 只按归属切 → 同一批分两次处理（上午看一半、下午回来看另一半）会被并成
 *      一段，而那正是「这批到底出了什么问题」最该看见的分界。
 *    归属未知的（`null`）只落回时间判据：`null !== null` 在 JS 里是 false，但
 *    「两边都不知道」本来就**不构成**切段理由，故显式判掉，否则每条自成一段。
 */

/** 同一天里超过这么久没有下一条，就算另一次任务了。 */
export const CLUSTER_GAP_SECS = 20 * 60;

const DAY_SECS = 86_400;

/** 任务归属键。数字（批次号）或字符串都行；`null` = 不知道。 */
export type ClusterKey = string | number | null;

export interface TimeCluster<T> {
  /** 稳定键（React key 与「选中本簇」用）。 */
  key: string;
  items: T[];
  /** 本簇最早 / 最晚的时刻（unix 秒）。 */
  from: number;
  to: number;
  /** 本簇认得出来的归属键（一簇里混着有归属和没归属时取那个认得出来的）。 */
  groupKey: ClusterKey;
}

export interface TimeDay<T> {
  key: string;
  /** 今天 / 昨天 / 前天 / 「7月26日 周六」。 */
  label: string;
  clusters: TimeCluster<T>[];
  count: number;
}

export interface TimelineOptions<T> {
  /** 取这一条的时刻（unix 秒）。 */
  at: (t: T) => number;
  /** 取这一条的任务归属。返回 null = 不知道。 */
  clusterKey: (t: T) => ClusterKey;
  /** 「今天/昨天」的参照时刻。 */
  now: number;
  /** 段内与段间的排序。`desc`（默认）= 新的在上。 */
  order?: "desc" | "asc";
  gapSecs?: number;
  /** 时刻相同时的稳定次序（返回一个单调 id）。缺省不额外定序。 */
  tieBreak?: (t: T) => number;
}

/**
 * 切成日 → 任务簇。
 *
 * 入参不必已排序：这里自己排一遍，于是调用方筛选之后顺序依然确定。
 */
export function groupByTimeline<T>(rows: readonly T[], opts: TimelineOptions<T>): TimeDay<T>[] {
  const dir = opts.order === "asc" ? 1 : -1;
  const gap = opts.gapSecs ?? CLUSTER_GAP_SECS;
  const tie = opts.tieBreak;
  const sorted = [...rows].sort((a, b) => {
    const d = (opts.at(a) - opts.at(b)) * dir;
    if (d !== 0) return d;
    return tie ? (tie(a) - tie(b)) * dir : 0;
  });

  const days: TimeDay<T>[] = [];
  for (const it of sorted) {
    const t = opts.at(it);
    const k = opts.clusterKey(it);
    const dayKey = dayKeyOf(t);

    let day = days[days.length - 1];
    if (!day || day.key !== dayKey) {
      day = { key: dayKey, label: dayLabel(t, opts.now), clusters: [], count: 0 };
      days.push(day);
    }
    day.count += 1;

    const cluster = day.clusters[day.clusters.length - 1];
    const prev = cluster?.items[cluster.items.length - 1];
    if (cluster && prev !== undefined && !breaks(opts, gap, prev, it)) {
      cluster.items.push(it);
      // `from`/`to` 与遍历方向无关：一簇的两端就是它的最小与最大时刻。
      cluster.from = Math.min(cluster.from, t);
      cluster.to = Math.max(cluster.to, t);
      // 一簇里混着有归属和没归属的（同一次操作里既处理了图又删了提示词）：
      // 报那个认得出来的，而不是因为末条没有归属就把整簇的身份抹掉。
      cluster.groupKey = cluster.groupKey ?? k;
    } else {
      days[days.length - 1]?.clusters.push({
        key: `${dayKey}#${day.clusters.length}`,
        items: [it],
        from: t,
        to: t,
        groupKey: k,
      });
    }
  }
  return days;
}

/** 相邻两条之间要不要断开。 */
function breaks<T>(opts: TimelineOptions<T>, gap: number, prev: T, cur: T): boolean {
  if (Math.abs(opts.at(prev) - opts.at(cur)) > gap) return true;
  const a = opts.clusterKey(prev);
  const b = opts.clusterKey(cur);
  // 两边都不知道归属 → 这条判据没有话说，别把它读成「两次不同的任务」。
  if (a == null && b == null) return false;
  return a !== b;
}

/** 本地日历日的键（同一天必须同键，跨年也不能撞）。 */
function dayKeyOf(unix: number): string {
  const d = new Date(unix * 1000);
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
}

const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/**
 * 日期标题。最近三天用「今天 / 昨天 / 前天」——那是人真正在用的说法。
 *
 * 差几天按**本地日历日**算，不是 `(now - t) / 86400`：晚上 11:50 弄出来的东西，
 * 到第二天上午就该叫「昨天」，而按秒差算那时它还不满 24 小时。
 */
export function dayLabel(unix: number, now: number): string {
  const diff = calendarDaysBetween(unix, now);
  if (diff === 0) return "今天";
  if (diff === 1) return "昨天";
  if (diff === 2) return "前天";
  const d = new Date(unix * 1000);
  const y = new Date(now * 1000).getFullYear() === d.getFullYear() ? "" : `${d.getFullYear()}年`;
  return `${y}${d.getMonth() + 1}月${d.getDate()}日 ${WEEKDAYS[d.getDay()] ?? ""}`;
}

/** 两个时刻相隔几个本地日历日（同日 = 0）。 */
function calendarDaysBetween(a: number, b: number): number {
  const midnight = (t: number) => {
    const d = new Date(t * 1000);
    d.setHours(0, 0, 0, 0);
    return Math.floor(d.getTime() / 1000);
  };
  return Math.round((midnight(b) - midnight(a)) / DAY_SECS);
}

/** 时刻 → `HH:MM`（本地）。 */
export function hhmm(unix: number): string {
  const d = new Date(unix * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** 一簇的时间跨度：落在同一分钟内就报一个点，否则报区间。 */
export function clusterTime(c: { from: number; to: number }): string {
  const a = hhmm(c.from);
  const b = hhmm(c.to);
  return a === b ? a : `${a} – ${b}`;
}

/**
 * 一簇里都是些什么（「验收未通过 12 · 已删除提示词 2」）。
 *
 * 按条数降序，最多三项 —— 它是标题的一部分，不是统计表。
 */
export function faceOf<T>(items: readonly T[], labelOf: (t: T) => string): string {
  const n = new Map<string, number>();
  for (const it of items) {
    const k = labelOf(it);
    if (k !== "") n.set(k, (n.get(k) ?? 0) + 1);
  }
  return [...n.entries()]
    .sort((x, y) => y[1] - x[1])
    .slice(0, 3)
    .map(([label, k]) => `${label} ${k}`)
    .join(" · ");
}
