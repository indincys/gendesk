import { type PackedRow, moveByRow, packJustifiedRows } from "@/features/review/layout";
import { describe, expect, it } from "vitest";

/** 造一张图：`r` 是宽高比（0.5625 = 9:16 竖，1 = 方，1.78 = 16:9 横）。 */
type Img = { id: number; r: number; g?: string | undefined };
const img = (id: number, r: number, g?: string): Img => ({ id, r, g });

const pack = (items: Img[], perRow = 4, width = 1000, gap = 10) =>
  packJustifiedRows(items, {
    width,
    perRow,
    gap,
    ratioOf: (t) => t.r,
    clusterKey: (t) => t.g ?? null,
    counts: new Map(),
  });

const cardRows = <T>(rows: PackedRow<T>[]) => rows.filter((r) => r.kind === "cards");

describe("验收页齐行排版", () => {
  // 这一页存在的全部理由：**按真实比例显示**。同一行里 9:16 与 16:9 必须宽度不同，
  // 否则就退回了「统一正方形」——而那会把竖幅裁掉上下、横幅裁掉左右。
  it("同一行里各张按自己的宽高比分宽度", () => {
    const { rows } = pack([img(1, 0.5625), img(2, 1.78)], 4);
    const row = cardRows(rows)[0];
    if (row?.kind !== "cards") throw new Error("应打出一行卡片");
    const [portrait, landscape] = row.items;
    if (!portrait || !landscape) throw new Error("两张都该在这一行");
    expect(landscape.w).toBeGreaterThan(portrait.w);
    // 宽度比就是宽高比之比（同一行共用行高），允许取整误差。
    expect(landscape.w / portrait.w).toBeCloseTo(1.78 / 0.5625, 1);
  });

  // 行高必须**渲染前就算得出来**且填满容器：虚拟化的 estimateSize 直接用它，
  // 算不准就要回头测量，而测量正是滚动抖动的来源。
  it("整行铺满容器宽度（含间距），不留缝也不溢出", () => {
    const { rows } = pack([img(1, 1), img(2, 1), img(3, 1), img(4, 1)], 4, 1000, 10);
    const row = cardRows(rows)[0];
    if (row?.kind !== "cards") throw new Error("应打出一行卡片");
    const total = row.items.reduce((s, i) => s + i.w, 0) + 10 * (row.items.length - 1);
    expect(Math.abs(total - 1000)).toBeLessThanOrEqual(row.items.length); // 逐张 floor 的累计误差
  });

  // 竖幅一行能塞下更多张 —— 这正是「每行固定列数」做不到、而齐行天然做到的事。
  it("竖幅一行塞得比横幅多", () => {
    const portraits = Array.from({ length: 12 }, (_, i) => img(i, 0.5625));
    const landscapes = Array.from({ length: 12 }, (_, i) => img(i, 1.78));
    const pRow = cardRows(pack(portraits).rows)[0];
    const lRow = cardRows(pack(landscapes).rows)[0];
    if (pRow?.kind !== "cards" || lRow?.kind !== "cards") throw new Error("两边都该有行");
    expect(pRow.items.length).toBeGreaterThan(lRow.items.length);
  });

  // 分段收尾常常只剩两三张。硬拉满会把它们撑成一整行的巨幅，比留白更奇怪。
  it("分组最后一行不被拉伸成巨幅", () => {
    const items = [img(1, 1, "A"), img(2, 1, "A"), img(3, 1, "A"), img(4, 1, "B")];
    const { rows } = pack(items, 4, 1000, 10);
    const last = cardRows(rows).at(-1);
    if (last?.kind !== "cards") throw new Error("应有收尾行");
    expect(last.items).toHaveLength(1);
    // 目标行高 = 1000/4 = 250。不设限的话这一张会被撑到 ~1000。
    expect(last.h).toBeLessThanOrEqual(250 * 1.35);
  });

  // 分组头是整宽行、不含卡片；每换一个分段就要先收掉上一段的不满行。
  it("换分段时先收行再插分组头", () => {
    const items = [img(1, 1, "A"), img(2, 1, "B")];
    const { rows } = pack(items, 4);
    expect(rows.map((r) => r.kind)).toEqual(["header", "cards", "header", "cards"]);
    expect(rows[0]).toMatchObject({ kind: "header", key: "A" });
    expect(rows[2]).toMatchObject({ kind: "header", key: "B" });
  });

  // cardRow 是「卡片全局序号 → 所在行号」，焦点滚动全靠它。
  it("每张卡片都能反查到自己所在的行", () => {
    const items = Array.from({ length: 9 }, (_, i) => img(i, 1));
    const { rows, cardRow } = pack(items, 4);
    for (let i = 0; i < items.length; i++) {
      const ri = cardRow[i];
      expect(ri).toBeDefined();
      const row = rows[ri as number];
      if (row?.kind !== "cards") throw new Error(`第 ${i} 张指向的不是卡片行`);
      expect(row.items.some((c) => c.idx === i)).toBe(true);
    }
  });

  // 尺寸缺失（0027 之前生成、且缩略图也读不出来）不能让整页塌掉。
  it("宽高比缺失时退化成方图而不是塌成 0 宽", () => {
    const { rows } = packJustifiedRows([img(1, 0), img(2, Number.NaN)], {
      width: 1000,
      perRow: 4,
      ratioOf: (t) => t.r,
      clusterKey: () => null,
      counts: new Map(),
    });
    const row = cardRows(rows)[0];
    if (row?.kind !== "cards") throw new Error("应打出一行");
    for (const c of row.items) expect(c.w).toBeGreaterThan(0);
  });
});

describe("齐行下的上下移动", () => {
  // 每行张数是变的，所以不能「焦点 ± 列数」——那在竖横混排里会跳得毫无规律。
  it("下移一行时保持列位", () => {
    const items = Array.from({ length: 8 }, (_, i) => img(i, 1));
    const { rows, cardRow } = pack(items, 4, 1000, 10);
    const first = cardRows(rows)[0];
    if (first?.kind !== "cards") throw new Error("应有首行");
    const n = first.items.length;
    // 第 0 张（首行第 1 列）下移应落到第二行第 1 列。
    expect(moveByRow(rows, cardRow, 0, 1)).toBe(n);
    expect(moveByRow(rows, cardRow, n, -1)).toBe(0);
  });

  // 下一行更短时取最后一张，绝不越界返回 undefined。
  it("目标行更短时落到该行最后一张", () => {
    // 首行塞满竖幅（每张 0.5625，攒够 sumR ≥ 4 才收行 → 8 张），
    // 第二行只剩一张横幅 —— 于是「下一行比这一行短」。
    const items = [...Array.from({ length: 8 }, (_, i) => img(i, 0.5625, "A")), img(99, 1.78, "A")];
    const { rows, cardRow } = pack(items, 4, 1000, 10);
    const first = cardRows(rows)[0];
    const second = cardRows(rows)[1];
    if (first?.kind !== "cards" || second?.kind !== "cards") throw new Error("应有两行");
    const lastCol = first.items.length - 1;
    const from = first.items[lastCol]?.idx as number;
    const got = moveByRow(rows, cardRow, from, 1);
    expect(second.items.some((c) => c.idx === got)).toBe(true);
  });

  // 分组头行不含卡片，上下移动必须跨过去而不是停在那里。
  it("跨分段移动时跳过分组头行", () => {
    const items = [img(1, 1, "A"), img(2, 1, "B")];
    const { rows, cardRow } = pack(items, 4);
    expect(moveByRow(rows, cardRow, 0, 1)).toBe(1);
    expect(moveByRow(rows, cardRow, 1, -1)).toBe(0);
  });

  it("到顶/到底不动", () => {
    const items = [img(1, 1), img(2, 1)];
    const { rows, cardRow } = pack(items, 4);
    expect(moveByRow(rows, cardRow, 0, -1)).toBe(0);
    expect(moveByRow(rows, cardRow, 1, 1)).toBe(1);
  });
});
