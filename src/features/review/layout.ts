/**
 * 验收网格的**齐行（justified）排版**——纯函数，与 React 无关，故可单独测。
 *
 * 它不是 UI 壳：这段算的是「每张图占多宽、每行多高」，而验收判的恰恰是构图与边缘，
 * 排错一格就等于给人看了一张裁过的图。同时行高必须在渲染**之前**算得出来，
 * 虚拟化才不需要回头测量——那是「滚动不卡顿」的全部依据。
 *
 * ## 为什么是齐行而不是瀑布流
 *
 * 两个要求同时成立：**按真实比例显示**（统一正方形会把竖幅裁掉上下、横幅裁掉左右）
 * 与**可虚拟化**（渲染前就知道每行多高）。
 *
 * 齐行两条都满足：一行里的图共用行高，各自宽度按自己的宽高比分配，
 * 行高 = 可用宽度 ÷ 该行宽高比之和。没有测量、没有 ResizeObserver 追图片加载。
 *
 * 瀑布流做不到第二条：列高要一张一张往下累加，第 N 张的位置依赖它前面的全部张，
 * 而虚拟化恰恰不渲染前面那些。
 */

/** 行模型：整宽的分组头行，或一行卡片（各自已定宽、共用行高）。 */
export type PackedRow<T> =
  | { kind: "header"; key: string; count: number }
  | { kind: "cards"; items: { it: T; idx: number; w: number }[]; h: number };

export interface PackOptions<T> {
  /** 容器可用宽度（已扣掉内边距）。 */
  width: number;
  /** 「每行大约几张」的目标值（滑块 3–8）；由它推出目标行高。 */
  perRow: number;
  /** 卡片间距，参与宽度分配——不算进去最后一张会被挤出边界。 */
  gap?: number;
  /** 单张的宽高比（宽/高）。 */
  ratioOf: (t: T) => number;
  /** 分段键；返回 null = 不分段（时间序）。 */
  clusterKey: (t: T) => string | null;
  /** 各段的条数（渲染分组头用）。 */
  counts: Map<string, number>;
}

/**
 * 分组最后一行的「不拉伸」阈值。
 *
 * 一段的收尾常常只剩两三张。硬按「填满整行」去算，三张图会被撑成一整行的巨幅，
 * 比留白更奇怪。超过目标行高这么多倍就退回目标行高、右侧留白。
 */
const LAST_ROW_STRETCH_LIMIT = 1.35;

/** 宽高比兜底：非有限数 / 非正数（尺寸缺失或读坏）一律当方图。 */
function safeRatio(r: number): number {
  return Number.isFinite(r) && r > 0 ? Math.max(0.05, r) : 1;
}

export function packJustifiedRows<T>(
  items: T[],
  opts: PackOptions<T>,
): { rows: PackedRow<T>[]; cardRow: number[] } {
  const gap = opts.gap ?? 10;
  const avail = Math.max(120, opts.width);
  const targetH = avail / Math.max(1, opts.perRow);

  const rows: PackedRow<T>[] = [];
  const cardRow: number[] = [];
  let buf: { it: T; idx: number; r: number }[] = [];
  let curKey: string | null = null;

  const flush = (lastOfCluster: boolean) => {
    if (buf.length === 0) return;
    const sumR = buf.reduce((s, b) => s + b.r, 0);
    const inner = avail - gap * (buf.length - 1);
    let h = inner / sumR;
    if (lastOfCluster && h > targetH * LAST_ROW_STRETCH_LIMIT) h = targetH;
    rows.push({
      kind: "cards",
      items: buf.map((b) => ({ it: b.it, idx: b.idx, w: Math.floor(h * b.r) })),
      h: Math.round(h),
    });
    buf = [];
  };

  items.forEach((it, idx) => {
    const ck = opts.clusterKey(it);
    if (ck !== null && ck !== curKey) {
      flush(true);
      curKey = ck;
      rows.push({ kind: "header", key: ck, count: opts.counts.get(ck) ?? 0 });
    }
    // 单张比一整行还宽（极端横幅）时也要能独占一行，故先入 buf 再判收行。
    //
    // 比例必须**先判有限再夹取**：`Math.max(0.05, NaN)` 回的是 NaN，而 NaN 会顺着
    // 「宽高比之和 → 行高 → 每张宽度」一路污染，把整行的宽度全变成 NaN、
    // 那一行当场塌掉。0/负数/NaN 一律退化成方图。
    buf.push({ it, idx, r: safeRatio(opts.ratioOf(it)) });
    const sumR = buf.reduce((s, b) => s + b.r, 0);
    if (sumR * targetH >= avail) flush(false);
  });
  flush(true);

  rows.forEach((r, ri) => {
    if (r.kind === "cards") for (const c of r.items) cardRow[c.idx] = ri;
  });
  return { rows, cardRow };
}

/**
 * 上/下移一行，尽量保持「在这一行里的第几张」。
 *
 * 齐行下每行张数是变的（一行竖幅能塞 5 张，一行横幅只塞 2 张），所以不能像固定网格
 * 那样「焦点 ± 列数」——那会在竖横混排的批次里跳得毫无规律。
 */
export function moveByRow<T>(
  rows: PackedRow<T>[],
  cardRow: number[],
  focus: number,
  dir: 1 | -1,
): number {
  const ri = cardRow[focus];
  if (ri === undefined) return focus;
  const cur = rows[ri];
  const col = cur?.kind === "cards" ? cur.items.findIndex((c) => c.idx === focus) : 0;
  // 跳过分组头行——它们不含卡片。
  for (let i = ri + dir; i >= 0 && i < rows.length; i += dir) {
    const r = rows[i];
    if (r?.kind !== "cards" || r.items.length === 0) continue;
    const target = r.items[Math.min(Math.max(0, col), r.items.length - 1)];
    return target ? target.idx : focus;
  }
  return focus;
}
