import { moveByRow, packGroups, packJustifiedRows } from "@/features/_shared/justified";
import { describe, expect, it } from "vitest";

/**
 * 分段从打包器里搬到了 `packGroups`（见 `justified.ts` 顶部的说明）：打包器只排一组，
 * 几级标题由外层摆。原先那三条「分组头」断言在这里原样保留，只是改成经 `packGroups`
 * 验证 —— 它们说的事没变（换段先收行、标题行不含卡片、方向键跨得过标题）。
 */

/** 造一张图：`r` 是宽高比（0.5625 = 9:16 竖，1 = 方，1.78 = 16:9 横）。 */
type Img = { id: number; r: number };
const img = (id: number, r: number): Img => ({ id, r });

const pack = (items: Img[], perRow = 4, width = 1000, gap = 10) =>
  packJustifiedRows(items, { width, perRow, gap, ratioOf: (t) => t.r });

/** 把若干组打成块（每组一个标题）。 */
const group = (groups: Img[][], perRow = 4, width = 1000, gap = 10) =>
  packGroups(
    groups.map((items, i) => ({ key: `g${i}`, heads: [`H${i}`], items })),
    { width, perRow, gap, ratioOf: (t) => t.r },
  );

describe("齐行排版", () => {
  // 这一页存在的全部理由：**按真实比例显示**。同一行里 9:16 与 16:9 必须宽度不同，
  // 否则就退回了「统一正方形」——而那会把竖幅裁掉上下、横幅裁掉左右。
  it("同一行里各张按自己的宽高比分宽度", () => {
    const row = pack([img(1, 0.5625), img(2, 1.78)], 4)[0];
    if (!row) throw new Error("应打出一行卡片");
    const [portrait, landscape] = row.items;
    if (!portrait || !landscape) throw new Error("两张都该在这一行");
    expect(landscape.w).toBeGreaterThan(portrait.w);
    // 宽度比就是宽高比之比（同一行共用行高），允许取整误差。
    expect(landscape.w / portrait.w).toBeCloseTo(1.78 / 0.5625, 1);
  });

  // 行高必须**渲染前就算得出来**且填满容器：虚拟化的 estimateSize 直接用它，
  // 算不准就要回头测量，而测量正是滚动抖动的来源。
  it("整行铺满容器宽度（含间距），不留缝也不溢出", () => {
    const row = pack([img(1, 1), img(2, 1), img(3, 1), img(4, 1)], 4, 1000, 10)[0];
    if (!row) throw new Error("应打出一行卡片");
    const total = row.items.reduce((s, i) => s + i.w, 0) + 10 * (row.items.length - 1);
    expect(Math.abs(total - 1000)).toBeLessThanOrEqual(row.items.length); // 逐张 floor 的累计误差
  });

  // 竖幅一行能塞下更多张 —— 这正是「每行固定列数」做不到、而齐行天然做到的事。
  it("竖幅一行塞得比横幅多", () => {
    const portraits = Array.from({ length: 12 }, (_, i) => img(i, 0.5625));
    const landscapes = Array.from({ length: 12 }, (_, i) => img(i, 1.78));
    const pRow = pack(portraits)[0];
    const lRow = pack(landscapes)[0];
    if (!pRow || !lRow) throw new Error("两边都该有行");
    expect(pRow.items.length).toBeGreaterThan(lRow.items.length);
  });

  // 一组收尾常常只剩两三张。硬拉满会把它们撑成一整行的巨幅，比留白更奇怪。
  it("一组最后一行不被拉伸成巨幅", () => {
    const rows = pack([img(1, 1), img(2, 1), img(3, 1), img(4, 1), img(5, 1)], 4, 1000, 10);
    const last = rows.at(-1);
    if (!last) throw new Error("应有收尾行");
    expect(last.items).toHaveLength(1);
    // 目标行高 = 1000/4 = 250。不设限的话这一张会被撑到 ~1000。
    expect(last.h).toBeLessThanOrEqual(250 * 1.35);
  });

  // 尺寸缺失（0027 之前生成、且缩略图也读不出来）不能让整页塌掉。
  it("宽高比缺失时退化成方图而不是塌成 0 宽", () => {
    const row = packJustifiedRows([img(1, 0), img(2, Number.NaN)], {
      width: 1000,
      perRow: 4,
      ratioOf: (t) => t.r,
    })[0];
    if (!row) throw new Error("应打出一行");
    for (const c of row.items) expect(c.w).toBeGreaterThan(0);
  });
});

describe("分段打包", () => {
  // 标题是整宽行、不含卡片；每换一段就要先收掉上一段的不满行 —— 一行绝不横跨两段。
  it("换分段时先收行再插标题", () => {
    const { blocks } = group([[img(1, 1)], [img(2, 1)]]);
    expect(blocks.map((b) => b.kind)).toEqual(["head", "cards", "head", "cards"]);
    expect(blocks[0]).toMatchObject({ kind: "head", head: "H0" });
    expect(blocks[2]).toMatchObject({ kind: "head", head: "H1" });
  });

  it("一段可以摆多级标题（日 + 任务簇）", () => {
    const { blocks } = packGroups([{ key: "d", heads: ["日", "簇"], items: [img(1, 1)] }], {
      width: 1000,
      perRow: 4,
      ratioOf: (t) => t.r,
    });
    expect(blocks.map((b) => b.kind)).toEqual(["head", "head", "cards"]);
  });

  // 组内序号必须搬成全局序号，否则第二段会选中第一段的图。
  it("卡片序号是全局序号，跨段连续", () => {
    const { blocks, flat } = group([[img(1, 1), img(2, 1)], [img(3, 1)]]);
    const idxs = blocks.flatMap((b) => (b.kind === "cards" ? b.items.map((c) => c.idx) : []));
    expect(idxs).toEqual([0, 1, 2]);
    expect(flat.map((i) => i.id)).toEqual([1, 2, 3]);
  });

  // cardRow 是「卡片全局序号 → 所在块号」，焦点滚动全靠它。
  it("每张卡片都能反查到自己所在的行", () => {
    const items = Array.from({ length: 9 }, (_, i) => img(i, 1));
    const { blocks, cardRow } = group([items]);
    for (let i = 0; i < items.length; i++) {
      const ri = cardRow[i];
      expect(ri).toBeDefined();
      const row = blocks[ri as number];
      if (row?.kind !== "cards") throw new Error(`第 ${i} 张指向的不是卡片行`);
      expect(row.items.some((c) => c.idx === i)).toBe(true);
    }
  });
});

describe("齐行下的上下移动", () => {
  // 每行张数是变的，所以不能「焦点 ± 列数」——那在竖横混排里会跳得毫无规律。
  it("下移一行时保持列位", () => {
    const items = Array.from({ length: 8 }, (_, i) => img(i, 1));
    const { blocks, cardRow } = group([items]);
    const first = blocks.find((b) => b.kind === "cards");
    if (first?.kind !== "cards") throw new Error("应有首行");
    const n = first.items.length;
    // 第 0 张（首行第 1 列）下移应落到第二行第 1 列。
    expect(moveByRow(blocks, cardRow, 0, 1)).toBe(n);
    expect(moveByRow(blocks, cardRow, n, -1)).toBe(0);
  });

  // 下一行更短时取最后一张，绝不越界返回 undefined。
  it("目标行更短时落到该行最后一张", () => {
    // 首行塞满竖幅（每张 0.5625，攒够 sumR ≥ 4 才收行 → 8 张），
    // 第二行只剩一张横幅 —— 于是「下一行比这一行短」。
    const items = [...Array.from({ length: 8 }, (_, i) => img(i, 0.5625)), img(99, 1.78)];
    const { blocks, cardRow } = group([items]);
    const cards = blocks.filter((b) => b.kind === "cards");
    const first = cards[0];
    const second = cards[1];
    if (first?.kind !== "cards" || second?.kind !== "cards") throw new Error("应有两行");
    const lastCol = first.items.length - 1;
    const from = first.items[lastCol]?.idx as number;
    const got = moveByRow(blocks, cardRow, from, 1);
    expect(second.items.some((c) => c.idx === got)).toBe(true);
  });

  // 标题行不含卡片，上下移动必须跨过去而不是停在那里。
  it("跨分段移动时跳过标题行", () => {
    const { blocks, cardRow } = group([[img(1, 1)], [img(2, 1)]]);
    expect(moveByRow(blocks, cardRow, 0, 1)).toBe(1);
    expect(moveByRow(blocks, cardRow, 1, -1)).toBe(0);
  });

  it("到顶/到底不动", () => {
    const { blocks, cardRow } = group([[img(1, 1), img(2, 1)]]);
    expect(moveByRow(blocks, cardRow, 0, -1)).toBe(0);
    expect(moveByRow(blocks, cardRow, 1, 1)).toBe(1);
  });
});
