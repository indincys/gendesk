import { ROUTES, ROUTE_BY_SHORTCUT } from "@/routes";
import { describe, expect, it } from "vitest";

describe("routes 注册表", () => {
  it("8 个页面，数字快捷键无重复", () => {
    // 发布、商品资料、图文任务单、文案库、参考图库、图片素材库已整页移除；
    // 视频两页没有数字快捷键，只能从侧栏进。
    expect(ROUTES).toHaveLength(8);
    const digits = ROUTES.map((r) => r.shortcut)
      .filter((s): s is number => s !== null)
      .sort((a, b) => a - b);
    expect(digits).toEqual([1, 2, 3, 4, 7, 8]);
  });

  it("既有页面的数字快捷键一个都没变（不重排，肌肉记忆的代价更大）", () => {
    expect(ROUTE_BY_SHORTCUT[1]).toBe("generate");
    expect(ROUTE_BY_SHORTCUT[2]).toBe("tasks");
    expect(ROUTE_BY_SHORTCUT[3]).toBe("review");
    expect(ROUTE_BY_SHORTCUT[4]).toBe("library");
    expect(ROUTE_BY_SHORTCUT[7]).toBe("trash");
    expect(ROUTE_BY_SHORTCUT[8]).toBe("settings");
  });

  // 页面移除后，它们的数字**空着**而不是被后面的页顶上来。
  // 重排会把每个人按了几百次的 ⌘7/⌘8 一次性作废，代价远大于少一个快捷键。
  it("已移除页面的数字留空，不把后面的页往前挪", () => {
    for (const n of [5, 6, 9, 0]) expect(ROUTE_BY_SHORTCUT[n]).toBeUndefined();
  });

  it("无数字快捷键的页面不进快捷键映射表", () => {
    expect(Object.values(ROUTE_BY_SHORTCUT)).not.toContain("v2v");
    expect(ROUTES.find((r) => r.key === "v2v")?.shortcut).toBeNull();
    expect(Object.values(ROUTE_BY_SHORTCUT)).not.toContain("clips");
    expect(ROUTES.find((r) => r.key === "clips")?.shortcut).toBeNull();
  });

  it("路由键唯一", () => {
    const keys = new Set(ROUTES.map((r) => r.key));
    expect(keys.size).toBe(ROUTES.length);
  });
});
