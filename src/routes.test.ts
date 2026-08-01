import { ROUTES, ROUTE_BY_SHORTCUT } from "@/routes";
import { describe, expect, it } from "vitest";

describe("routes 注册表", () => {
  it("13 个页面，数字快捷键无重复", () => {
    // 图文发布重构新增商品资料，并将 ⌘9/⌘0 固定给图片素材库与图文任务单。
    // v0.15.0 新增「视频流水线」、v0.20.0 新增「视频成片」——十个数字已用尽，
    // 两页都没有数字快捷键，只能从侧栏进（v0.24.0 去掉了 ⌘K 命令面板）。
    // v0.21.0 移除「提示词库」：提示词是消耗品，没有可长期浏览的库。
    // v0.31.0 新增「文案库」（标题/正文/话题标签三池），同样无数字快捷键。
    expect(ROUTES).toHaveLength(13);
    const digits = ROUTES.map((r) => r.shortcut)
      .filter((s): s is number => s !== null)
      .sort((a, b) => a - b);
    expect(digits).toEqual([0, 1, 2, 3, 4, 6, 7, 8, 9]);
  });

  it("既有页面的数字快捷键一个都没变（不重排，肌肉记忆的代价更大）", () => {
    expect(ROUTE_BY_SHORTCUT[1]).toBe("generate");
    expect(ROUTE_BY_SHORTCUT[2]).toBe("tasks");
    expect(ROUTE_BY_SHORTCUT[3]).toBe("review");
    expect(ROUTE_BY_SHORTCUT[4]).toBe("library");
    expect(ROUTE_BY_SHORTCUT[6]).toBe("refs");
    expect(ROUTE_BY_SHORTCUT[8]).toBe("settings");
    expect(ROUTE_BY_SHORTCUT[9]).toBe("images");
    expect(ROUTE_BY_SHORTCUT[0]).toBe("sheets");
  });

  // 提示词库整页移除后，⌘5 **空着**而不是被后面的页顶上来。
  // 重排会把每个人按了几百次的 ⌘6/⌘7 一次性作废，代价远大于少一个快捷键。
  it("⌘5 留空，不把后面的页往前挪", () => {
    expect(ROUTE_BY_SHORTCUT[5]).toBeUndefined();
    expect(ROUTES.some((r) => r.key === ("prompts" as string))).toBe(false);
  });

  it("无数字快捷键的页面不进快捷键映射表", () => {
    expect(Object.values(ROUTE_BY_SHORTCUT)).not.toContain("v2v");
    expect(ROUTES.find((r) => r.key === "v2v")?.shortcut).toBeNull();
    expect(Object.values(ROUTE_BY_SHORTCUT)).not.toContain("clips");
    expect(ROUTES.find((r) => r.key === "clips")?.shortcut).toBeNull();
    expect(Object.values(ROUTE_BY_SHORTCUT)).not.toContain("copy");
    expect(ROUTES.find((r) => r.key === "copy")?.shortcut).toBeNull();
    expect(Object.values(ROUTE_BY_SHORTCUT)).not.toContain("products");
    expect(ROUTES.find((r) => r.key === "products")?.shortcut).toBeNull();
  });

  it("路由键唯一", () => {
    const keys = new Set(ROUTES.map((r) => r.key));
    expect(keys.size).toBe(ROUTES.length);
  });
});
