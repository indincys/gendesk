import { ROUTES, ROUTE_BY_SHORTCUT } from "@/routes";
import { describe, expect, it } from "vitest";

describe("routes 注册表", () => {
  it("10 个页面（原八页 + 资产库/发布计划），快捷键无重复", () => {
    // 发布模块 P1 新增两项导航：资产库(⌘9) / 发布计划(⌘0)。
    expect(ROUTES).toHaveLength(10);
    const shortcuts = ROUTES.map((r) => r.shortcut).sort((a, b) => a - b);
    expect(shortcuts).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it("ROUTE_BY_SHORTCUT 与快捷键映射一致", () => {
    expect(ROUTE_BY_SHORTCUT[1]).toBe("generate");
    expect(ROUTE_BY_SHORTCUT[8]).toBe("settings");
    expect(ROUTE_BY_SHORTCUT[9]).toBe("assets");
    expect(ROUTE_BY_SHORTCUT[0]).toBe("plan");
  });

  it("路由键唯一", () => {
    const keys = new Set(ROUTES.map((r) => r.key));
    expect(keys.size).toBe(ROUTES.length);
  });
});
