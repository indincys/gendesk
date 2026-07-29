/**
 * 图片网格的**齐行（justified）排版**——纯函数，与 React 无关，故可单独测。
 *
 * 验收页与废纸篓共用（放在 `_shared/` 而不是任一页里：两页排的是同一件事，
 * 各留一份的话，「一行放几张」这类判断迟早会在其中一页上悄悄改掉）。
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
 *
 * ## 分段是 `packGroups` 的事，不是打包器的事
 *
 * 早期版本把「分段键」塞进打包器里（`clusterKey` + `counts`），于是它同时管着
 * 「怎么排一行」与「哪些图算一段」。废纸篓要两级标题（日 → 任务簇）时这套就不够用了，
 * 而再加一个层级参数只会让打包器继续膨胀。现在切开：打包器只排**一组**，
 * 分段与标题由 `packGroups` 摆，几级标题都摆得下。
 */

/** 一行卡片：各自已定宽、共用行高。 */
export interface CardsRow<T> {
  kind: "cards";
  items: { it: T; idx: number; w: number }[];
  h: number;
}

export interface PackOptions<T> {
  /** 容器可用宽度（已扣掉内边距）。 */
  width: number;
  /** 「每行大约几张」的目标值（大小滑块）；由它推出目标行高。 */
  perRow: number;
  /** 卡片间距，参与宽度分配——不算进去最后一张会被挤出边界。 */
  gap?: number;
  /** 单张的宽高比（宽/高）。 */
  ratioOf: (t: T) => number;
}

/**
 * 一组的最后一行的「不拉伸」阈值。
 *
 * 一组的收尾常常只剩两三张。硬按「填满整行」去算，三张图会被撑成一整行的巨幅，
 * 比留白更奇怪。超过目标行高这么多倍就退回目标行高、右侧留白。
 */
const LAST_ROW_STRETCH_LIMIT = 1.35;

/** 宽高比兜底：非有限数 / 非正数（尺寸缺失或读坏）一律当方图。 */
function safeRatio(r: number): number {
  return Number.isFinite(r) && r > 0 ? Math.max(0.05, r) : 1;
}

/**
 * 把**一组**图打成若干齐行。序号是组内序号（0 起），由 `packGroups` 搬成全局序号。
 */
export function packJustifiedRows<T>(items: readonly T[], opts: PackOptions<T>): CardsRow<T>[] {
  const gap = opts.gap ?? 10;
  const avail = Math.max(120, opts.width);
  const targetH = avail / Math.max(1, opts.perRow);

  const rows: CardsRow<T>[] = [];
  let buf: { it: T; idx: number; r: number }[] = [];

  const flush = (last: boolean) => {
    if (buf.length === 0) return;
    const sumR = buf.reduce((s, b) => s + b.r, 0);
    const inner = avail - gap * (buf.length - 1);
    let h = inner / sumR;
    if (last && h > targetH * LAST_ROW_STRETCH_LIMIT) h = targetH;
    rows.push({
      kind: "cards",
      items: buf.map((b) => ({ it: b.it, idx: b.idx, w: Math.floor(h * b.r) })),
      h: Math.round(h),
    });
    buf = [];
  };

  items.forEach((it, idx) => {
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
  return rows;
}

/** 一段：它前面要摆的标题行（0..n 个，两级分段就是 2 个），加上要打包的图。 */
export interface PackGroup<T, H> {
  key: string;
  heads: H[];
  items: readonly T[];
}

/** 渲染块：整宽的标题行，或一行卡片。 */
export type PackedBlock<T, H> =
  | { kind: "head"; key: string; head: H }
  | ({ key: string } & CardsRow<T>);

/**
 * 逐段分别打包，产出可直接渲染、也可直接用方向键导航的扁平块数组。
 *
 * **逐段分别打包**是分段能看得见的前提：整页一次打包的话，一行会横跨两段，
 * 于是「区隔」只剩一条画在图与图之间的线，而人眼在一整行等高图上根本读不到它。
 * 逐段打包的代价是每段末行右侧留白 —— 那正是要的呼吸感。
 *
 * `cardRow[全局序号] = 块下标`，与 `moveByRow` 的约定一致；`flat` 是全局序。
 */
export function packGroups<T, H>(
  groups: readonly PackGroup<T, H>[],
  opts: PackOptions<T>,
): { blocks: PackedBlock<T, H>[]; cardRow: number[]; flat: T[] } {
  const blocks: PackedBlock<T, H>[] = [];
  const cardRow: number[] = [];
  const flat: T[] = [];

  for (const g of groups) {
    g.heads.forEach((head, i) => {
      blocks.push({ kind: "head", key: `${g.key}h${i}`, head });
    });
    const base = flat.length;
    flat.push(...g.items);
    for (const row of packJustifiedRows(g.items, opts)) {
      // 打包器给的是**组内**序号，这里搬成全局序号 —— 键盘导航与选区都按全局走。
      const shifted = row.items.map((c) => ({ ...c, idx: base + c.idx }));
      const at = blocks.length;
      blocks.push({ kind: "cards", key: `${g.key}r${at}`, items: shifted, h: row.h });
      for (const c of shifted) cardRow[c.idx] = at;
    }
  }
  return { blocks, cardRow, flat };
}

/**
 * 上/下移一行，尽量保持「在这一行里的第几张」。
 *
 * 齐行下每行张数是变的（一行竖幅能塞 5 张，一行横幅只塞 2 张），所以不能像固定网格
 * 那样「焦点 ± 列数」——那会在竖横混排的批次里跳得毫无规律。
 *
 * 行的类型只按**结构**要求（卡片行有 `items`，标题行没有），不写死成某一页的块类型：
 * 「跳过所有非卡片行」这条规则对几级标题都一字不差地成立 —— 两边各写一份的话，
 * 其中一份迟早会漏掉一种标题行，症状是方向键在某种分隔上莫名其妙地卡住。
 */
export function moveByRow(
  // `key` 只为让这个结构类型有个两种块都有的必需属性 —— 否则 TS 会把它判成弱类型，
  // 而弱类型对「一个属性都不重合」的联合分支直接报错。
  rows: readonly { key: string; items?: readonly { idx: number }[] }[],
  cardRow: number[],
  focus: number,
  dir: 1 | -1,
): number {
  const ri = cardRow[focus];
  if (ri === undefined) return focus;
  const col = rows[ri]?.items?.findIndex((c) => c.idx === focus) ?? 0;
  // 跳过标题行——它们不含卡片。
  for (let i = ri + dir; i >= 0 && i < rows.length; i += dir) {
    const r = rows[i];
    if (r?.items === undefined || r.items.length === 0) continue;
    const target = r.items[Math.min(Math.max(0, col), r.items.length - 1)];
    return target ? target.idx : focus;
  }
  return focus;
}
