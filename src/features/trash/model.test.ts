import {
  buildBlocks,
  clusterFace,
  clusterSkill,
  groupTrash,
  ratioOf,
} from "@/features/trash/model";
import type { TrashItemView } from "@/lib/ipc";
import { describe, expect, it } from "vitest";

/** 某天某时刻的 unix 秒（本地时区）。 */
function at(y: number, mo: number, d: number, h: number, mi = 0): number {
  return Math.floor(new Date(y, mo - 1, d, h, mi, 0, 0).getTime() / 1000);
}

let nextId = 1;
function item(deletedAt: number, over: Partial<TrashItemView> = {}): TrashItemView {
  return {
    id: nextId++,
    entityType: "task",
    code: null,
    title: null,
    refName: null,
    thumbPath: "/t.jpg",
    imagePath: null,
    promptText: null,
    sourceLabel: "验收未通过",
    deletedAt,
    restorable: true,
    width: 3,
    height: 4,
    batchId: null,
    skill: null,
    ...over,
  };
}

describe("废纸篓分段", () => {
  it("接的是删除时刻与批次 —— 五类实体唯一共有的时间戳就是它", () => {
    const t = at(2026, 7, 28, 10);
    const days = groupTrash([item(t, { batchId: 9 }), item(t - 5, { batchId: 8 })], t);
    expect(days).toHaveLength(1);
    expect(days[0]?.clusters.map((c) => c.groupKey)).toEqual([9, 8]);
  });

  it("簇的成分按条数降序", () => {
    const t = at(2026, 7, 28, 10);
    const [day] = groupTrash(
      [
        item(t, { sourceLabel: "已删除提示词" }),
        item(t - 1, { sourceLabel: "验收未通过" }),
        item(t - 2, { sourceLabel: "验收未通过" }),
      ],
      t,
    );
    expect(clusterFace(day?.clusters[0] as never)).toBe("验收未通过 2 · 已删除提示词 1");
  });
});

describe("簇的 skill 归属", () => {
  const t = at(2026, 7, 28, 10);
  const clusterOf = (rows: TrashItemView[]) => groupTrash(rows, t)[0]?.clusters[0] as never;

  it("一簇一个 skill 就报它", () => {
    expect(
      clusterSkill(
        clusterOf([
          item(t, { skill: "prompts-ugc-real" }),
          item(t - 1, { skill: "prompts-ugc-real" }),
        ]),
      ),
    ).toBe("prompts-ugc-real");
  });

  it("混了就报「几个」而不是挑第一个 —— 挑第一个会把整簇的锅算到一个 skill 头上", () => {
    expect(clusterSkill(clusterOf([item(t, { skill: "a" }), item(t - 1, { skill: "b" })]))).toBe(
      "2 个 skill",
    );
  });

  it("一条都没有就不报 —— 手动导入的提示词本来就没有 skill", () => {
    expect(clusterSkill(clusterOf([item(t), item(t - 1, { skill: "" })]))).toBeNull();
  });
});

describe("网格打包", () => {
  it("逐簇分别打包 —— 一行绝不横跨两个任务簇，否则区隔在等高图上根本读不到", () => {
    const t = at(2026, 7, 28, 10);
    const rows = [
      item(t, { batchId: 9 }),
      item(t - 1, { batchId: 9 }),
      item(t - 2, { batchId: 8 }),
      item(t - 3, { batchId: 8 }),
    ];
    const { blocks, cardRow, flat } = buildBlocks(groupTrash(rows, t), {
      width: 1000,
      perRow: 4,
      gap: 12,
    });
    expect(flat).toHaveLength(4);
    // 第一簇顶「日期头 + 簇头」，其余只顶簇头；每簇各自一行卡片。
    expect(blocks.map((b) => b.kind)).toEqual(["head", "head", "cards", "head", "cards"]);
    expect(blocks.filter((b) => b.kind === "head").map((b) => b.head.kind)).toEqual([
      "day",
      "cluster",
      "cluster",
    ]);
    // 每张图都落在某一行上，且同簇两张同行、异簇两张不同行。
    expect(cardRow).toHaveLength(4);
    expect(cardRow[0]).toBe(cardRow[1]);
    expect(cardRow[2]).toBe(cardRow[3]);
    expect(cardRow[1]).not.toBe(cardRow[2]);
  });

  it("卡片序号搬成全局序号 —— 打包器给的是簇内序号，照抄会让第二簇选中第一簇的图", () => {
    const t = at(2026, 7, 28, 10);
    const { blocks } = buildBlocks(
      groupTrash([item(t, { batchId: 9 }), item(t - 1, { batchId: 8 })], t),
      { width: 1000, perRow: 4, gap: 12 },
    );
    const idxs = blocks.flatMap((b) => (b.kind === "cards" ? b.items.map((c) => c.idx) : []));
    expect(idxs).toEqual([0, 1]);
  });

  it("缺尺寸退回竖幅而不是 NaN —— 比例会一路污染到整行宽度", () => {
    expect(ratioOf(item(0, { width: null, height: null }))).toBe(3 / 4);
    expect(ratioOf(item(0, { width: 0, height: 0 }))).toBe(3 / 4);
    expect(ratioOf(item(0, { width: 1920, height: 1080 }))).toBeCloseTo(16 / 9);
  });
});
