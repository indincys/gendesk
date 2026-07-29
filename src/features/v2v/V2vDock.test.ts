import { dockScope, reviewScope } from "@/features/v2v/V2vDock";
import { channelGridClass } from "@/features/v2v/V2vList";
import type { Row } from "@/features/v2v/model";
import { describe, expect, it } from "vitest";

function row(id: number, stage: Row["stage"]): Row {
  return { clip: { id }, stage } as Row;
}

describe("视频批量作用域", () => {
  const ready = [row(1, "ready"), row(2, "ready"), row(3, "ready")];

  it("有勾选项时只作用于全部选中任务", () => {
    expect(dockScope(ready[0] ?? null, ready, new Set([2, 3])).map((item) => item.clip.id)).toEqual(
      [2, 3],
    );
  });

  it("已选项即使不在当前筛选里也不会悄悄空转", () => {
    expect(dockScope(ready[0] ?? null, ready, new Set([3])).map((item) => item.clip.id)).toEqual([
      3,
    ]);
  });

  it("没有勾选项时只作用于当前任务", () => {
    expect(dockScope(ready[1] ?? null, ready, new Set()).map((item) => item.clip.id)).toEqual([2]);
  });

  it("全选退回改写会一次得到全部合格 ID", () => {
    const scope = dockScope(ready[0] ?? null, ready, new Set([1, 2, 3]));
    const ids = scope
      .filter((item) => ["ready", "rev", "rej", "fail"].includes(item.stage))
      .map((item) => item.clip.id);
    expect(ids).toEqual([1, 2, 3]);
  });

  it("批量验收固定使用进入时选中的验收任务", () => {
    const rows = [row(1, "rev"), row(2, "rev"), row(3, "ready"), row(4, "rev")];
    expect(reviewScope(rows, new Set([2, 4])).map((item) => item.clip.id)).toEqual([2, 4]);
  });

  it("四个及以上通道使用两行布局", () => {
    expect(channelGridClass(3)).toBe("chq");
    expect(channelGridClass(4)).toBe("chq tworow");
    expect(channelGridClass(8)).toBe("chq tworow");
  });
});
