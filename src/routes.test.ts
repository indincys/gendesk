import { ROUTES, ROUTE_BY_SHORTCUT } from "@/routes";
import { describe, expect, it } from "vitest";

describe("routes 注册表", () => {
  it("恰好 8 个页面，快捷键 1–8 无重复", () => {
    expect(ROUTES).toHaveLength(8);
    const shortcuts = ROUTES.map((r) => r.shortcut).sort((a, b) => a - b);
    expect(shortcuts).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it("ROUTE_BY_SHORTCUT 与 ⌘1–8 映射一致", () => {
    expect(ROUTE_BY_SHORTCUT[1]).toBe("generate");
    expect(ROUTE_BY_SHORTCUT[8]).toBe("settings");
  });

  it("路由键唯一", () => {
    const keys = new Set(ROUTES.map((r) => r.key));
    expect(keys.size).toBe(ROUTES.length);
  });
});
