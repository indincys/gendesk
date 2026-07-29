import {
  CLUSTER_GAP_SECS,
  clusterTime,
  dayLabel,
  faceOf,
  groupByTimeline,
  hhmm,
} from "@/features/_shared/timeline";
import { describe, expect, it } from "vitest";

/** 某天某时刻的 unix 秒（本地时区）—— 分段全按本地日历日算，测试也必须按本地造。 */
function at(y: number, mo: number, d: number, h: number, mi = 0): number {
  return Math.floor(new Date(y, mo - 1, d, h, mi, 0, 0).getTime() / 1000);
}

interface Row {
  t: number;
  batch: number | null;
  label?: string;
}
const row = (t: number, batch: number | null = null, label?: string): Row =>
  label === undefined ? { t, batch } : { t, batch, label };

const cut = (rows: Row[], now: number) =>
  groupByTimeline(rows, { at: (r) => r.t, clusterKey: (r) => r.batch, now });

describe("日 → 任务簇", () => {
  it("跨天必断开 —— 深夜与凌晨只隔两小时，在人的记忆里却是两件事", () => {
    const days = cut(
      [row(at(2026, 7, 27, 23, 50)), row(at(2026, 7, 28, 1, 30))],
      at(2026, 7, 28, 12),
    );
    expect(days.map((d) => d.label)).toEqual(["今天", "昨天"]);
    expect(days.map((d) => d.count)).toEqual([1, 1]);
  });

  it("同一天里隔得久就断开，挨着的留在同一簇", () => {
    const t = at(2026, 7, 28, 10);
    const days = cut(
      [row(t), row(t - 60), row(t - 60 - CLUSTER_GAP_SECS - 1)],
      at(2026, 7, 28, 12),
    );
    expect(days).toHaveLength(1);
    expect(days[0]?.clusters.map((c) => c.items.length)).toEqual([2, 1]);
  });

  it("换了归属就断开 —— 连着跑两批只隔几秒，只按时间切会并成一段", () => {
    const t = at(2026, 7, 28, 10);
    const days = cut([row(t, 9), row(t - 5, 9), row(t - 10, 8)], at(2026, 7, 28, 12));
    expect(days[0]?.clusters.map((c) => c.groupKey)).toEqual([9, 8]);
  });

  it("同一批分两次处理也断开 —— 那正是「这批出了什么问题」最该看见的分界", () => {
    const t = at(2026, 7, 28, 16);
    const days = cut([row(t, 9), row(t - CLUSTER_GAP_SECS - 1, 9)], at(2026, 7, 28, 18));
    expect(days[0]?.clusters).toHaveLength(2);
  });

  it("两边都不知道归属不构成切段理由 —— 否则每条都自成一段", () => {
    const t = at(2026, 7, 28, 10);
    const days = cut([row(t), row(t - 30), row(t - 60)], at(2026, 7, 28, 12));
    expect(days[0]?.clusters).toHaveLength(1);
  });

  it("一簇里混着有归属和没归属的，报那个认得出来的", () => {
    const t = at(2026, 7, 28, 10);
    // 同一次操作里先处理了图（有批次），顺手删了条提示词（没有批次概念）。
    const days = cut([row(t, 9), row(t - 5, 9)], t);
    expect(days[0]?.clusters[0]?.groupKey).toBe(9);
  });

  it("入参乱序也照样分段 —— 筛选之后顺序必须依然确定", () => {
    const t = at(2026, 7, 28, 10);
    const days = cut([row(t - 3600), row(t), row(t - 1800)], t);
    expect(days[0]?.clusters.map((c) => c.items.map((i) => i.t))).toEqual([
      [t],
      [t - 1800],
      [t - 3600],
    ]);
  });

  it("正序也成立 —— from/to 是簇的两端，与遍历方向无关", () => {
    const t = at(2026, 7, 28, 10);
    const days = groupByTimeline([row(t), row(t - 300)], {
      at: (r) => r.t,
      clusterKey: (r) => r.batch,
      now: t,
      order: "asc",
    });
    const c = days[0]?.clusters[0];
    expect(c?.items.map((i) => i.t)).toEqual([t - 300, t]);
    expect([c?.from, c?.to]).toEqual([t - 300, t]);
  });

  it("时刻相同的用 tieBreak 定序 —— 否则同一秒里的几条每次渲染排法都可能不同", () => {
    const t = at(2026, 7, 28, 10);
    const rows = [
      { t, batch: null, id: 2 },
      { t, batch: null, id: 1 },
    ];
    const days = groupByTimeline(rows, {
      at: (r) => r.t,
      clusterKey: () => null,
      now: t,
      tieBreak: (r) => r.id,
    });
    expect(days[0]?.clusters[0]?.items.map((r) => r.id)).toEqual([2, 1]);
  });
});

describe("标题文案", () => {
  it("最近三天说人话，再往前给日期与星期", () => {
    const now = at(2026, 7, 28, 12);
    expect(dayLabel(at(2026, 7, 28, 1), now)).toBe("今天");
    expect(dayLabel(at(2026, 7, 27, 23), now)).toBe("昨天");
    expect(dayLabel(at(2026, 7, 26, 9), now)).toBe("前天");
    expect(dayLabel(at(2026, 7, 25, 9), now)).toBe("7月25日 周六");
    // 跨年要带年份，否则「1月3日」到底是哪一年说不清。
    expect(dayLabel(at(2025, 12, 31, 9), now)).toBe("2025年12月31日 周三");
  });

  it("差几天按本地日历日算，不是按满 24 小时", () => {
    // 23:50 弄的，次日 00:10 看：只隔 20 分钟，但那已经是「昨天」了。
    expect(dayLabel(at(2026, 7, 27, 23, 50), at(2026, 7, 28, 0, 10))).toBe("昨天");
  });

  it("一分钟内的一簇报一个时刻，跨时间的报区间", () => {
    const t = at(2026, 7, 28, 10, 5);
    expect(clusterTime({ from: t - 300, to: t })).toBe("10:00 – 10:05");
    expect(clusterTime({ from: t, to: t })).toBe("10:05");
    expect(hhmm(at(2026, 7, 28, 9, 7))).toBe("09:07");
  });

  it("成分按条数降序，最多三项，空标签不占位", () => {
    const items = [
      row(0, null, "已删除提示词"),
      row(0, null, "验收未通过"),
      row(0, null, "验收未通过"),
      row(0, null, ""),
    ];
    expect(faceOf(items, (r) => r.label ?? "")).toBe("验收未通过 2 · 已删除提示词 1");
  });
});
